use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;

use crate::Shared;

pub async fn healthz() -> &'static str {
    "ok"
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
