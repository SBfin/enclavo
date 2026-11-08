//! Calls to OpenAI-compatible providers, with fallback across candidate routes.

use std::{pin::Pin, sync::Arc, time::Duration};

use axum::http::StatusCode;
use eventsource_stream::{Event, Eventsource};
use futures::{Stream, StreamExt};
use serde_json::{Value, json};

use crate::{
    catalog::Route,
    error::ApiError,
    router::{Lease, Router},
};

pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event, String>> + Send>>;

/// Why one attempt failed, and whether another route may be tried.
#[derive(Debug)]
pub struct AttemptError {
    pub retryable: bool,
    /// Counts against the route's health (a client mistake does not).
    pub route_fault: bool,
    pub retry_after: Option<Duration>,
    pub status: Option<StatusCode>,
    pub body: Option<Value>,
    pub message: String,
}

pub struct JsonAnswer {
    pub route: Arc<Route>,
    pub body: Value,
}

pub struct StreamAnswer {
    pub route: Arc<Route>,
    pub first: Event,
    pub rest: EventStream,
    /// Keeps the provider slot taken until the stream ends.
    pub lease: Lease,
}

#[derive(Clone)]
pub struct Upstream {
    http: reqwest::Client,
}

impl Upstream {
    pub fn new() -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .build()?;
        Ok(Self { http })
    }

    /// The request body as this route should receive it.
    fn body_for(route: &Route, body: &Value, completion_cap: u64, stream: bool) -> Value {
        let mut body = body.clone();
        let obj = body.as_object_mut().expect("validated as an object");
        obj.remove("provider");
        obj.insert("model".into(), Value::String(route.upstream_model.clone()));
        // Hold the provider to the budget the payment ceiling was computed with.
        if obj.contains_key("max_completion_tokens") {
            obj.insert("max_completion_tokens".into(), completion_cap.into());
            obj.remove("max_tokens");
        } else {
            obj.insert("max_tokens".into(), completion_cap.into());
        }
        obj.insert("stream".into(), Value::Bool(stream));
        if stream {
            obj.insert("stream_options".into(), json!({ "include_usage": true }));
        } else {
            obj.remove("stream_options");
        }
        body
    }

    async fn send(
        &self,
        route: &Route,
        body: &Value,
        first_byte_timeout: Duration,
    ) -> Result<reqwest::Response, AttemptError> {
        let url = format!("{}/chat/completions", route.provider.base_url);
        let mut request = self.http.post(url).json(body).timeout(route.provider.timeout);
        if let Some(key) = &route.provider.api_key {
            request = request.bearer_auth(key);
        }
        let response = match tokio::time::timeout(first_byte_timeout, request.send()).await {
            Err(_) => return Err(transport_error("timed out waiting for the provider")),
            Ok(Err(err)) => return Err(transport_error(&err.to_string())),
            Ok(Ok(response)) => response,
        };
        let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        if status.is_success() {
            return Ok(response);
        }
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = response.json::<Value>().await.ok();
        let retryable = status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::REQUEST_TIMEOUT
            || status.is_server_error()
            // Our own key or account at the provider is the problem; another provider may work.
            || status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || status == StatusCode::PAYMENT_REQUIRED;
        Err(AttemptError {
            retryable,
            route_fault: retryable,
            retry_after: if status == StatusCode::TOO_MANY_REQUESTS {
                retry_after.or(Some(Duration::from_secs(5)))
            } else {
                None
            },
            status: Some(status),
            body,
            message: format!("provider answered {status}"),
        })
    }

    /// Tries candidates in order until one answers with a complete JSON body.
    pub async fn complete(
        &self,
        router: &Arc<Router>,
        routes: &[Arc<Route>],
        caps: &[u64],
        body: &Value,
    ) -> Result<JsonAnswer, ApiError> {
        let first_byte = Duration::from_secs(router.config().first_byte_timeout_secs);
        let mut last: Option<AttemptError> = None;
        for (route, mut lease, cap) in leases(router, routes, caps) {
            let attempt = async {
                let response = self.send(&route, &Self::body_for(&route, body, cap, false), first_byte).await?;
                response.json::<Value>().await.map_err(|e| transport_error(&format!("unreadable provider body: {e}")))
            };
            match attempt.await {
                Ok(answer) => {
                    lease.succeed();
                    record_attempt(&route, "ok");
                    return Ok(JsonAnswer { route, body: answer });
                }
                Err(err) => {
                    if err.route_fault {
                        lease.fail(err.retry_after);
                    }
                    record_attempt(&route, "error");
                    tracing::warn!(route = %route.id, error = %err.message, "provider attempt failed");
                    if !err.retryable {
                        return Err(client_error(err));
                    }
                    last = Some(err);
                }
            }
        }
        Err(exhausted(last))
    }

    /// Tries candidates in order until one sends its first stream event; the route is then fixed.
    pub async fn stream(
        &self,
        router: &Arc<Router>,
        routes: &[Arc<Route>],
        caps: &[u64],
        body: &Value,
    ) -> Result<StreamAnswer, ApiError> {
        let first_byte = Duration::from_secs(router.config().first_byte_timeout_secs);
        let mut last: Option<AttemptError> = None;
        for (route, mut lease, cap) in leases(router, routes, caps) {
            let attempt = async {
                let response = self.send(&route, &Self::body_for(&route, body, cap, true), first_byte).await?;
                let mut events: EventStream =
                    Box::pin(response.bytes_stream().eventsource().map(|r| r.map_err(|e| e.to_string())));
                match tokio::time::timeout(first_byte, events.next()).await {
                    Err(_) => Err(transport_error("timed out waiting for the first stream event")),
                    Ok(None) => Err(transport_error("stream ended before any event")),
                    Ok(Some(Err(e))) => Err(transport_error(&e)),
                    Ok(Some(Ok(first))) => Ok((first, events)),
                }
            };
            match attempt.await {
                Ok((first, rest)) => {
                    lease.succeed();
                    record_attempt(&route, "ok");
                    return Ok(StreamAnswer { route, first, rest, lease });
                }
                Err(err) => {
                    if err.route_fault {
                        lease.fail(err.retry_after);
                    }
                    record_attempt(&route, "error");
                    tracing::warn!(route = %route.id, error = %err.message, "provider attempt failed");
                    if !err.retryable {
                        return Err(client_error(err));
                    }
                    last = Some(err);
                }
            }
        }
        Err(exhausted(last))
    }
}

/// Leases candidates lazily in order, skipping routes whose circuit is open or provider is full.
fn leases<'a>(
    router: &'a Arc<Router>,
    routes: &'a [Arc<Route>],
    caps: &'a [u64],
) -> impl Iterator<Item = (Arc<Route>, Lease, u64)> + 'a {
    routes
        .iter()
        .zip(caps.iter().copied())
        .filter_map(|(route, cap)| router.try_lease(route).map(|lease| (route.clone(), lease, cap)))
        .take(router.config().max_attempts)
}

fn record_attempt(route: &Route, outcome: &'static str) {
    metrics::counter!("enclavo_upstream_attempts_total", "route" => route.id.clone(), "outcome" => outcome)
        .increment(1);
}

fn transport_error(message: &str) -> AttemptError {
    AttemptError {
        retryable: true,
        route_fault: true,
        retry_after: None,
        status: None,
        body: None,
        message: message.to_string(),
    }
}

fn client_error(err: AttemptError) -> ApiError {
    match (err.status, err.body) {
        (Some(status), Some(body)) => ApiError::Upstream { status, body },
        _ => ApiError::upstream_failed(err.message),
    }
}

fn exhausted(last: Option<AttemptError>) -> ApiError {
    match last {
        Some(err) => ApiError::upstream_failed(format!("Every route failed; last error: {}", err.message)),
        None => ApiError::no_route("Every matching route is busy or cooling down; retry shortly."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Provider;

    #[test]
    fn body_is_rewritten_for_the_route() {
        let route = Route {
            id: "r".into(),
            model: "m".into(),
            display_name: "m".into(),
            provider: Arc::new(Provider {
                id: "p".into(),
                base_url: String::new(),
                api_key: None,
                max_concurrency: 1,
                timeout: Duration::from_secs(1),
            }),
            upstream_model: "org/m-fp8".into(),
            attestation: String::new(),
            attested: true,
            score: 1,
            tokens_per_iu: 1,
            micro_usd_per_iu: 1,
            max_output_tokens: 1,
            prompt_weight_bps: 10_000,
        };
        let body = json!({ "model": "m", "messages": [], "max_completion_tokens": 9000, "max_tokens": 9000,
            "provider": { "only": ["p"], "require_attestation": true } });
        let out = Upstream::body_for(&route, &body, 50, true);
        assert_eq!(out["model"], "org/m-fp8");
        assert_eq!(out["max_completion_tokens"], 50);
        assert!(out.get("max_tokens").is_none());
        assert!(out.get("provider").is_none());
        assert_eq!(out["stream_options"]["include_usage"], true);
    }
}
