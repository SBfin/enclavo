use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;

use crate::Shared;

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn status(State(state): State<Shared>) -> Json<serde_json::Value> {
    let mut providers: Vec<_> = state.catalog.providers.keys().cloned().collect();
    providers.sort();
    Json(json!({
        "mode": state.cfg.server.mode,
        "providers": providers,
        "routes": state.catalog.routes.len(),
        "funding": if state.cfg.payments.networks.is_empty() { "operator_credit" } else { "x402" },
        "attestation_verification": "not_implemented",
        "chain": crate::chain::Chain::public_info(),
    }))
}

pub async fn readyz(State(state): State<Shared>) -> impl IntoResponse {
    if state.shutdown.is_cancelled() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "draining" })));
    }
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(err) => {
            tracing::error!(error = %err, "database not reachable");
            (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "database unavailable" })))
        }
    }
}

pub async fn metrics(State(state): State<Shared>) -> impl IntoResponse {
    match &state.metrics {
        Some(handle) => (StatusCode::OK, handle.render()),
        None => (StatusCode::NOT_FOUND, String::from("metrics are disabled")),
    }
}
