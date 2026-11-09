pub mod account;
pub mod batches;
pub mod chat;
pub mod files;
pub mod models;
pub mod ops;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::{catalog::Preferences, error::ApiError};

pub fn request_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

const MAX_CHOICES: u64 = 16;

/// A validated chat request: the body to forward, its model, and routing preferences.
pub struct ChatRequest {
    pub body: Value,
    pub model: String,
    pub prefs: Preferences,
    pub stream: bool,
}

/// Checks the parts of a chat body the gateway relies on and strips the `provider` object.
pub fn parse_chat(mut body: Value) -> Result<ChatRequest, ApiError> {
    let obj = body.as_object_mut().ok_or_else(|| ApiError::bad_request("The request body must be a JSON object."))?;
    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("`model` is required."))?
        .to_string();
    match obj.get("messages") {
        Some(Value::Array(messages)) if !messages.is_empty() => {}
        _ => return Err(ApiError::bad_request("`messages` must be a non-empty array.")),
    }
    for key in ["max_tokens", "max_completion_tokens", "n"] {
        if let Some(v) = obj.get(key)
            && !v.is_null()
            && v.as_u64().is_none_or(|n| n == 0)
        {
            return Err(ApiError::bad_request(format!("`{key}` must be a positive integer.")));
        }
    }
    if obj.get("n").and_then(Value::as_u64).is_some_and(|n| n > MAX_CHOICES) {
        return Err(ApiError::bad_request(format!("`n` can be at most {MAX_CHOICES}.")));
    }
    let prefs = match obj.remove("provider") {
        None | Some(Value::Null) => Preferences::default(),
        Some(v) => {
            serde_json::from_value(v).map_err(|e| ApiError::bad_request(format!("Invalid `provider` object: {e}")))?
        }
    };
    let stream = obj.get("stream").and_then(Value::as_bool).unwrap_or(false);
    Ok(ChatRequest { body, model, prefs, stream })
}

pub fn unix(ts: chrono::DateTime<chrono::Utc>) -> i64 {
    ts.timestamp()
}
