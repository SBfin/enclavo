use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Shared, billing, error::ApiError, x402::types::PAYMENT_RESPONSE_HEADER};

#[derive(Deserialize)]
struct TopUpRequest {
    /// USD as a decimal string ("10.50") or number.
    amount_usd: Value,
}

/// Parses a USD amount into micro-USD without going through floating point.
pub fn parse_usd(value: &Value) -> Option<u64> {
    let text = match value {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let (whole, frac) = text.split_once('.').unwrap_or((&text, ""));
    if whole.is_empty() && frac.is_empty() || frac.len() > 6 {
        return None;
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let whole: u64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let frac: u64 = format!("{frac:0<6}").parse().ok()?;
    whole.checked_mul(1_000_000)?.checked_add(frac)
}

/// Buys prepaid credit with an `exact` x402 payment. Without a key, creates an account.
pub async fn top_up(State(state): State<Shared>, headers: HeaderMap, bytes: Bytes) -> Result<Response, ApiError> {
    let req: TopUpRequest =
        serde_json::from_slice(&bytes).map_err(|e| ApiError::bad_request(format!("Invalid body: {e}")))?;
    let amount = parse_usd(&req.amount_usd)
        .ok_or_else(|| ApiError::bad_request("`amount_usd` must be a positive amount with at most 6 decimals."))?;
    // Detached, like chat, so a client that hangs up can't interrupt a settlement half-way.
    let worker = state.clone();
    let done = state
        .tasks
        .spawn(async move { billing::top_up(&worker, &headers, "/v1/credits/topup", amount).await })
        .await
        .map_err(ApiError::internal)??;
    let account = sqlx::query_scalar::<_, i64>("SELECT balance_micro FROM accounts WHERE id = $1")
        .bind(done.account_id)
        .fetch_one(&state.db)
        .await?;
    let mut body = json!({
        "account_id": done.account_id,
        "status": if done.settled { "settled" } else { "pending" },
        "credited_micro_usd": done.credited_micro,
        "balance_micro_usd": account,
    });
    if !done.settled {
        body["detail"] = Value::String(
            "The transfer was sent but is not confirmed yet; the credit is applied automatically once it settles."
                .into(),
        );
    }
    if let Some(key) = done.api_key {
        body["api_key"] = Value::String(key);
        body["note"] = Value::String("Store this API key now; it is not shown again.".into());
    }
    let status = if done.settled { StatusCode::OK } else { StatusCode::ACCEPTED };
    let mut response = (status, Json(body)).into_response();
    if let Ok(v) = HeaderValue::from_str(&done.payment_response)
        && !done.payment_response.is_empty()
    {
        response.headers_mut().insert(PAYMENT_RESPONSE_HEADER, v);
    }
    Ok(response)
}

pub async fn show(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    let (requests, spent): (i64, Option<i64>) =
        sqlx::query_as("SELECT count(*), sum(cost_micro)::bigint FROM usage_events WHERE account_id = $1")
            .bind(account.id)
            .fetch_one(&state.db)
            .await?;
    Ok(Json(json!({
        "id": account.id,
        "object": "account",
        "balance_micro_usd": account.balance_micro,
        "held_micro_usd": account.held_micro,
        "requests": requests,
        "spent_micro_usd": spent.unwrap_or(0),
        "created_at": super::unix(account.created_at),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usd_exactly() {
        assert_eq!(parse_usd(&json!("10")), Some(10_000_000));
        assert_eq!(parse_usd(&json!("0.000001")), Some(1));
        assert_eq!(parse_usd(&json!("2.5")), Some(2_500_000));
        assert_eq!(parse_usd(&json!(3.25)), Some(3_250_000));
        assert_eq!(parse_usd(&json!(".5")), Some(500_000));
        assert_eq!(parse_usd(&json!("1.0000001")), None);
        assert_eq!(parse_usd(&json!("-1")), None);
        assert_eq!(parse_usd(&json!("1e3")), None);
        assert_eq!(parse_usd(&json!(".")), None);
    }
}
