use std::{convert::Infallible, time::Duration};

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue},
    response::{IntoResponse, Response, Sse, sse::Event, sse::KeepAlive},
};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::{
    Shared,
    billing::{self, Charge, Payer, Receipt},
    catalog::Route,
    error::ApiError,
    pricing::{Tier, Usage, estimate_tokens},
    router::LiveGuard,
    upstream::StreamAnswer,
    x402::types::PAYMENT_RESPONSE_HEADER,
};

const PATH: &str = "/v1/chat/completions";

/// How long a client may stop reading a stream before it counts as gone.
const CLIENT_STALL: Duration = Duration::from_secs(30);
/// After a client leaves, how long to keep reading the provider for its usage report.
const USAGE_DRAIN: Duration = Duration::from_secs(30);

pub async fn completions(State(state): State<Shared>, headers: HeaderMap, bytes: Bytes) -> Result<Response, ApiError> {
    // Runs detached so a client that disconnects mid-request can't skip finalizing its payment.
    let tasks = state.tasks.clone();
    tasks.spawn(handle(state, headers, bytes, None)).await.map_err(ApiError::internal)?
}

pub async fn completions_for_account(
    state: Shared,
    headers: HeaderMap,
    bytes: Bytes,
    account: crate::ledger::Account,
) -> Result<Response, ApiError> {
    let tasks = state.tasks.clone();
    tasks.spawn(handle(state, headers, bytes, Some(account))).await.map_err(ApiError::internal)?
}

async fn handle(
    state: Shared,
    headers: HeaderMap,
    bytes: Bytes,
    account: Option<crate::ledger::Account>,
) -> Result<Response, ApiError> {
    let request_id = super::request_id(&headers);
    let body: Value =
        serde_json::from_slice(&bytes).map_err(|e| ApiError::bad_request(format!("Invalid JSON body: {e}")))?;
    let mut req = super::parse_chat(body)?;
    let contract = req
        .body
        .as_object_mut()
        .and_then(|o| o.remove("capacity_contract_id"))
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .and_then(|s| s.parse::<uuid::Uuid>().ok())
                .ok_or_else(|| ApiError::bad_request("capacity_contract_id must be a UUID."))
        })
        .transpose()?;
    let mut routes = state.catalog.candidates(&req.model, &req.prefs, |r| state.router.latency_ms(r))?;
    let mut customer = account;
    if let Some(id) = contract {
        let owned = match customer.take() {
            Some(a) => a,
            None => billing::require_account(&state, &headers).await?,
        };
        let route = crate::market::contract_route(&state, owned.id, id).await?;
        routes.retain(|r| r.id == route);
        if routes.is_empty() {
            return Err(ApiError::bad_request("This model does not match the selected capacity contract."));
        }
        customer = Some(owned);
    }
    let ceiling = state.pricing.ceiling(&routes, &req.body, Tier::Live);
    let payer = if let Some(id) = contract {
        let owner = customer.as_ref().unwrap();
        let choices = req.body["n"].as_u64().unwrap_or(1);
        let hold = crate::market::reserve_capacity(
            &state,
            owner.id,
            id,
            ceiling.prompt_bound,
            ceiling.completion_caps[0].saturating_mul(choices),
        )
        .await?;
        Payer::Capacity { account_id: owner.id, hold_id: hold }
    } else {
        match customer {
            Some(account) => {
                billing::authorize_for_account(&state, &headers, PATH, ceiling.micro, Some(account)).await?
            }
            None => billing::authorize(&state, &headers, PATH, ceiling.micro).await?,
        }
    };
    let live = state.router.live_request();

    if !req.stream {
        let answer = match state.upstream.complete(&state.router, &routes, &ceiling.completion_caps, &req.body).await {
            Ok(answer) => answer,
            Err(err) => {
                billing::finalize(&state, payer, &request_id, None).await;
                return Err(err);
            }
        };
        let usage = answer
            .body
            .get("usage")
            .and_then(Usage::from_openai)
            .unwrap_or_else(|| estimate_json_usage(ceiling.prompt_bound, &answer.body));
        let cost = state.pricing.cost(&answer.route, usage, Tier::Live).min(ceiling.micro);
        let charge = Charge { route_id: &answer.route.id, usage, cost_micro: cost, tier: Tier::Live };
        let receipt = billing::finalize(&state, payer, &request_id, Some(charge)).await;
        drop(live);

        let mut body = answer.body;
        if let Some(obj) = body.as_object_mut() {
            obj.insert("model".into(), Value::String(answer.route.id.clone()));
        }
        let mut response = Json(body).into_response();
        route_headers(response.headers_mut(), &answer.route, Some(&receipt));
        if let Some(id) = contract {
            response.headers_mut().insert("x-enclavo-capacity-contract", id.to_string().parse().unwrap());
        }
        return Ok(response);
    }

    let answer = match state.upstream.stream(&state.router, &routes, &ceiling.completion_caps, &req.body).await {
        Ok(answer) => answer,
        Err(err) => {
            billing::finalize(&state, payer, &request_id, None).await;
            return Err(err);
        }
    };
    let route = answer.route.clone();
    let (tx, mut rx) = mpsc::channel::<Event>(64);
    // The pump owns the upstream and the payment, so it finishes even if the client goes away.
    state.tasks.spawn(pump(state.clone(), answer, payer, ceiling.micro, request_id, ceiling.prompt_bound, live, tx));

    let events = async_stream::stream! {
        while let Some(event) = rx.recv().await {
            yield Ok::<_, Infallible>(event);
        }
    };
    let mut response = Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))).into_response();
    route_headers(response.headers_mut(), &route, None);
    if let Some(id) = contract {
        response.headers_mut().insert("x-enclavo-capacity-contract", id.to_string().parse().unwrap());
    }
    Ok(response)
}

fn route_headers(headers: &mut HeaderMap, route: &Route, receipt: Option<&Receipt>) {
    let mut set = |name: &'static str, value: &str| {
        if let Ok(v) = HeaderValue::from_str(value) {
            headers.insert(HeaderName::from_static(name), v);
        }
    };
    set("x-enclavo-route", &route.id);
    set("x-enclavo-provider", &route.provider.id);
    set("x-enclavo-attestation", &route.attestation);
    if let Some(receipt) = receipt {
        set("x-enclavo-cost-micro-usd", &receipt.charged_micro.to_string());
        if let Some(pr) = &receipt.payment_response {
            set(PAYMENT_RESPONSE_HEADER, pr);
        }
    }
}

/// Usage when the provider reported none: the prompt at its upper bound, the completion counted.
fn estimate_json_usage(prompt_bound: u64, response: &Value) -> Usage {
    let prompt = prompt_bound;
    let completion = response
        .get("choices")
        .and_then(Value::as_array)
        .map(|choices| choices.iter().map(|c| estimate_tokens(&c["message"].to_string())).sum())
        .unwrap_or(0);
    Usage { prompt_tokens: prompt, completion_tokens: completion, estimated: true }
}

/// Bytes of generated text in a stream chunk, for estimating usage when the provider sends none.
fn chunk_text_len(chunk: &Value) -> usize {
    chunk
        .get("choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .map(|c| {
                    let delta = &c["delta"];
                    ["content", "reasoning_content", "reasoning"]
                        .iter()
                        .filter_map(|k| delta.get(*k).and_then(Value::as_str))
                        .map(str::len)
                        .sum::<usize>()
                        + delta.get("tool_calls").map(|t| t.to_string().len()).unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
async fn pump(
    state: Shared,
    answer: StreamAnswer,
    payer: Payer,
    ceiling: u64,
    request_id: String,
    prompt_bound: u64,
    live: LiveGuard,
    tx: mpsc::Sender<Event>,
) {
    let StreamAnswer { route, first, mut rest, mut lease } = answer;
    let idle = route.provider.timeout;
    let mut usage: Option<Usage> = None;
    let mut generated_bytes = 0usize;
    let mut client_gone: Option<tokio::time::Instant> = None;
    let mut failure: Option<String> = None;

    let mut next = Some(first);
    loop {
        // Once the client has left, keep reading only long enough to learn what was generated.
        let wait = match client_gone {
            Some(since) => match USAGE_DRAIN.checked_sub(since.elapsed()) {
                Some(left) => left,
                None => break,
            },
            None => idle,
        };
        let event = match next.take() {
            Some(event) => event,
            None => match tokio::time::timeout(wait, rest.next()).await {
                Ok(Some(Ok(event))) => event,
                Ok(None) => break,
                Ok(Some(Err(err))) => {
                    failure = Some(err);
                    break;
                }
                Err(_) if client_gone.is_some() => break,
                Err(_) => {
                    failure = Some("provider stopped sending".into());
                    break;
                }
            },
        };
        if event.data.trim() == "[DONE]" {
            break;
        }
        let data = match serde_json::from_str::<Value>(&event.data) {
            Ok(mut chunk) => {
                if let Some(u) = chunk.get("usage").and_then(Usage::from_openai) {
                    usage = Some(u);
                }
                generated_bytes += chunk_text_len(&chunk);
                if let Some(obj) = chunk.as_object_mut() {
                    obj.insert("model".into(), Value::String(route.id.clone()));
                }
                chunk.to_string()
            }
            Err(_) => event.data,
        };
        if client_gone.is_none() && tx.send_timeout(Event::default().data(data), CLIENT_STALL).await.is_err() {
            client_gone = Some(tokio::time::Instant::now());
        }
    }
    drop(rest); // closes the provider connection

    if let Some(err) = &failure {
        tracing::warn!(route = %route.id, %request_id, error = %err, "stream broke mid-answer");
        lease.fail(None);
    }
    let usage = usage.unwrap_or(Usage {
        prompt_tokens: prompt_bound,
        completion_tokens: (generated_bytes as u64).div_ceil(4),
        estimated: true,
    });
    let cost = state.pricing.cost(&route, usage, Tier::Live).min(ceiling);
    let charge = Charge { route_id: &route.id, usage, cost_micro: cost, tier: Tier::Live };
    let receipt = billing::finalize(&state, payer, &request_id, Some(charge)).await;
    drop(lease);
    drop(live);

    if client_gone.is_some() {
        return;
    }
    let mut tail = Vec::new();
    if let Some(err) = failure {
        let body = json!({ "error": { "message": format!("The provider stream broke: {err}"), "type": "server_error", "code": "upstream_failed" } });
        tail.push(Event::default().data(body.to_string()));
    }
    // Headers are long gone, so the receipt travels as SSE comments, which SDKs ignore.
    tail.push(Event::default().comment(format!("enclavo-cost-micro-usd {}", receipt.charged_micro)));
    if let Some(pr) = receipt.payment_response {
        tail.push(Event::default().comment(format!("payment-response {pr}")));
    }
    tail.push(Event::default().data("[DONE]"));
    for event in tail {
        if tx.send_timeout(event, CLIENT_STALL).await.is_err() {
            break;
        }
    }
}
