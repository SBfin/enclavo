//! Per-API-key and per-IP rate limits.

use std::{
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::{DefaultKeyedRateLimiter, Quota};

use crate::{Shared, billing, config::LimitsConfig, error::ApiError};

pub struct RateLimits {
    keys: DefaultKeyedRateLimiter<String>,
    ips: DefaultKeyedRateLimiter<IpAddr>,
    client_ip_header: Option<String>,
}

fn quota(rps: u32, burst: u32) -> Quota {
    let rps = NonZeroU32::new(rps.max(1)).expect("non-zero");
    Quota::per_second(rps).allow_burst(NonZeroU32::new(burst.max(1)).expect("non-zero"))
}

impl RateLimits {
    pub fn new(cfg: &LimitsConfig, client_ip_header: Option<String>) -> Self {
        Self {
            keys: DefaultKeyedRateLimiter::keyed(quota(cfg.per_key_rps, cfg.per_key_burst)),
            ips: DefaultKeyedRateLimiter::keyed(quota(cfg.per_ip_rps, cfg.per_ip_burst)),
            client_ip_header,
        }
    }

    /// Drops idle entries so the key maps don't grow without bound.
    pub fn prune(&self) {
        self.keys.retain_recent();
        self.ips.retain_recent();
    }

    pub fn check_customer(&self, user: &str) -> bool {
        self.keys.check_key(&format!("customer:{user}")).is_ok()
    }

    fn client_ip(&self, req: &Request) -> Option<IpAddr> {
        if let Some(header) = &self.client_ip_header {
            let from_proxy =
                req.headers().get(header).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
            if from_proxy.is_some() {
                return from_proxy;
            }
        }
        req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip())
    }
}

/// Keyed by API key when there is one, otherwise by client IP.
pub async fn middleware(State(state): State<Shared>, req: Request, next: Next) -> Response {
    let limits = &state.limits;
    let allowed = match billing::api_key(req.headers()) {
        Some(key) => limits.keys.check_key(&key.to_string()).is_ok(),
        None => match limits.client_ip(&req) {
            Some(ip) => limits.ips.check_key(&ip).is_ok(),
            None => true,
        },
    };
    if !allowed {
        metrics::counter!("enclavo_rate_limited_total").increment(1);
        return ApiError::rate_limited().into_response();
    }
    next.run(req).await
}
