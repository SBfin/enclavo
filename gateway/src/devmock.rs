//! Stand-ins for a provider and an x402 facilitator, for running the whole stack locally without
//! provider keys or a wallet. Never used by the gateway itself.
//!
//! Provider: `POST /{kind}/chat/completions` where `kind` is `good` (answers), `slow` (streams
//! slowly), or `broken` (503s). Facilitator: `POST /facilitator/{verify,settle}`, approving every
//! payment except one whose `payload.signature` is `bad`.

use std::{convert::Infallible, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::pricing::estimate_tokens;

pub fn router() -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/healthz") }))
        .route("/healthz", get(|| async { "ok" }))
        .route("/{kind}/chat/completions", post(chat))
        .route("/facilitator/verify", post(verify))
        .route("/facilitator/settle", post(settle))
        .route("/facilitator/supported", get(supported))
}

fn reply_for(body: &Value) -> String {
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.iter().rev().find(|m| m["role"] == "user"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or("…");
    let quoted: String = last.chars().take(80).collect();
    format!("This is a local mock answer from {}. You said: {quoted}", body["model"].as_str().unwrap_or("a mock"))
}

async fn chat(Path(kind): Path<String>, Json(body): Json<Value>) -> Response {
    if kind == "broken" {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": { "message": "mock provider is down" } })))
            .into_response();
    }
    let model = body["model"].as_str().unwrap_or("mock").to_string();
    let text = reply_for(&body);
    let usage = json!({
        "prompt_tokens": estimate_tokens(&body["messages"].to_string()),
        "completion_tokens": estimate_tokens(&text),
    });
    if body["stream"] != true {
        return Json(json!({
            "id": "chatcmpl-mock", "object": "chat.completion", "model": model,
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }],
            "usage": usage,
        }))
        .into_response();
    }

    let delay = Duration::from_millis(if kind == "slow" { 150 } else { 20 });
    let mut lines: Vec<String> = text
        .split_inclusive(' ')
        .map(|word| {
            json!({ "id": "chatcmpl-mock", "object": "chat.completion.chunk", "model": model,
                    "choices": [{ "index": 0, "delta": { "content": word } }] })
            .to_string()
        })
        .collect();
    lines.push(json!({ "id": "chatcmpl-mock", "object": "chat.completion.chunk", "model": model, "choices": [], "usage": usage }).to_string());
    lines.push("[DONE]".into());
    let events = futures::stream::iter(lines).then(move |line| async move {
        tokio::time::sleep(delay).await;
        Ok::<_, Infallible>(format!("data: {line}\n\n"))
    });
    ([("content-type", "text/event-stream")], Body::from_stream(events)).into_response()
}

async fn verify(Json(body): Json<Value>) -> Json<Value> {
    let payload = &body["paymentPayload"]["payload"];
    let valid = payload["signature"] != "bad";
    let payer = payload["authorization"]["from"].as_str().unwrap_or("0x000000000000000000000000000000000000dEaD");
    Json(
        json!({ "isValid": valid, "invalidReason": if valid { Value::Null } else { "invalid_signature".into() }, "payer": payer }),
    )
}

async fn settle(Json(body): Json<Value>) -> Json<Value> {
    let req = &body["paymentRequirements"];
    let tx = format!("0x{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    Json(json!({
        "success": true,
        "transaction": tx,
        "network": req["network"],
        "amount": req["amount"],
        "payer": "0x000000000000000000000000000000000000dEaD",
    }))
}

async fn supported() -> Json<Value> {
    Json(json!({ "kinds": [
        { "x402Version": 2, "scheme": "upto", "network": "eip155:84532" },
        { "x402Version": 2, "scheme": "exact", "network": "eip155:84532" },
    ] }))
}
