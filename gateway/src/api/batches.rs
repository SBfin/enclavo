use std::collections::HashSet;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{Shared, batch, billing, error::ApiError, ledger, pricing::Tier};

const ENDPOINT: &str = "/v1/chat/completions";

#[derive(Deserialize)]
struct CreateBatch {
    input_file_id: String,
    endpoint: String,
    completion_window: String,
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Deserialize)]
struct Line {
    custom_id: String,
    method: String,
    url: String,
    body: Value,
}

fn usd(micro: i64) -> String {
    format!("${}.{:06}", micro / 1_000_000, micro % 1_000_000)
}

pub async fn create(State(state): State<Shared>, headers: HeaderMap, bytes: Bytes) -> Result<Json<Value>, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    let req: CreateBatch =
        serde_json::from_slice(&bytes).map_err(|e| ApiError::bad_request(format!("Invalid body: {e}")))?;
    if req.endpoint != ENDPOINT {
        return Err(ApiError::bad_request(format!("Only `{ENDPOINT}` batches are supported.")));
    }
    if req.completion_window != "24h" {
        return Err(ApiError::bad_request("`completion_window` must be `24h`."));
    }
    let content: Vec<u8> =
        sqlx::query_scalar("SELECT content FROM files WHERE id = $1 AND account_id = $2 AND purpose = 'batch'")
            .bind(&req.input_file_id)
            .bind(account.id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("No batch file with id {}.", req.input_file_id)))?;
    let text = String::from_utf8(content).map_err(|_| ApiError::bad_request("The input file is not UTF-8."))?;

    // Validate and price every line before anything is stored.
    let mut lines = Vec::new();
    let mut ids = HashSet::new();
    let mut ceiling: u64 = 0;
    for (n, raw) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let at = |msg: String| ApiError::bad_request(format!("Line {}: {msg}", n + 1));
        let line: Line = serde_json::from_str(raw).map_err(|e| at(format!("not a valid request: {e}")))?;
        if line.method != "POST" || line.url != ENDPOINT {
            return Err(at(format!("each request must be POST {ENDPOINT}")));
        }
        if !ids.insert(line.custom_id.clone()) {
            return Err(at(format!("duplicate custom_id `{}`", line.custom_id)));
        }
        let chat = super::parse_chat(line.body.clone()).map_err(|e| at(api_message(e)))?;
        if chat.stream {
            return Err(at("batch requests cannot stream".into()));
        }
        let routes = state
            .catalog
            .candidates(&chat.model, &batch::batch_prefs(chat.prefs), |r| state.router.latency_ms(r))
            .map_err(|e| at(api_message(e)))?;
        ceiling = ceiling.saturating_add(state.pricing.ceiling(&routes, &chat.body, Tier::Batch).micro);
        lines.push((n as i32 + 1, line.custom_id, line.body));
    }
    if lines.is_empty() {
        return Err(ApiError::bad_request("The input file has no requests."));
    }
    if lines.len() > state.cfg.batch.max_items {
        return Err(ApiError::bad_request(format!("Batches are limited to {} requests.", state.cfg.batch.max_items)));
    }

    let window = state.cfg.batch.completion_window_secs;
    let ceiling = i64::try_from(ceiling).map_err(|_| ApiError::bad_request("This batch is too large to price."))?;
    let hold_id = ledger::hold(&state.db, account.id, ceiling, "batch", window + 3_600).await?.ok_or_else(|| {
        ApiError::payment_failed(format!(
            "This batch can cost up to {}; the balance is {}. Top up with POST /v1/credits/topup.",
            usd(ceiling),
            usd(account.balance_micro)
        ))
    })?;

    let id = format!("batch_{}", Uuid::new_v4().simple());
    let stored = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(
            "INSERT INTO batches (id, account_id, input_file_id, endpoint, completion_window, status, hold_id,
                                  ceiling_micro, metadata, expires_at)
             VALUES ($1, $2, $3, $4, $5, 'in_progress', $6, $7, $8, now() + make_interval(secs => $9))",
        )
        .bind(&id)
        .bind(account.id)
        .bind(&req.input_file_id)
        .bind(ENDPOINT)
        .bind(&req.completion_window)
        .bind(hold_id)
        .bind(ceiling)
        .bind(&req.metadata)
        .bind(window as f64)
        .execute(&mut *tx)
        .await?;
        let (numbers, custom_ids, bodies): (Vec<i32>, Vec<String>, Vec<String>) =
            lines.into_iter().map(|(n, c, b)| (n, c, b.to_string())).collect();
        sqlx::query(
            "INSERT INTO batch_items (batch_id, line, custom_id, body)
             SELECT $1, n, c, b::jsonb FROM UNNEST($2::int[], $3::text[], $4::text[]) AS t(n, c, b)",
        )
        .bind(&id)
        .bind(&numbers)
        .bind(&custom_ids)
        .bind(&bodies)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    };
    if let Err(err) = stored.await {
        let mut tx = state.db.begin().await?;
        ledger::release_hold(&mut tx, hold_id).await?;
        tx.commit().await?;
        return Err(err.into());
    }
    Ok(Json(batch_object(&state, &id, account.id).await?))
}

fn api_message(err: ApiError) -> String {
    match err {
        ApiError::Status { message, .. } => message,
        other => format!("{:?}", other.status()),
    }
}

fn ts(row: &sqlx::postgres::PgRow, col: &str) -> Result<Option<i64>, sqlx::Error> {
    Ok(row.try_get::<Option<DateTime<Utc>>, _>(col)?.map(super::unix))
}

pub async fn batch_object(state: &Shared, id: &str, account: Uuid) -> Result<Value, ApiError> {
    let row = sqlx::query(
        "SELECT b.*,
                (SELECT count(*) FROM batch_items i WHERE i.batch_id = b.id) AS total,
                (SELECT count(*) FROM batch_items i WHERE i.batch_id = b.id AND i.status = 'succeeded') AS completed,
                (SELECT count(*) FROM batch_items i WHERE i.batch_id = b.id
                   AND i.status IN ('failed', 'expired', 'cancelled')) AS failed,
                (SELECT coalesce(sum(cost_micro), 0)::bigint FROM batch_items i WHERE i.batch_id = b.id) AS charged
         FROM batches b WHERE b.id = $1 AND b.account_id = $2",
    )
    .bind(id)
    .bind(account)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found(format!("No batch with id {id}.")))?;
    let status: String = row.try_get("status")?;
    Ok(json!({
        "id": id,
        "object": "batch",
        "endpoint": row.try_get::<String, _>("endpoint")?,
        "errors": null,
        "input_file_id": row.try_get::<String, _>("input_file_id")?,
        "completion_window": row.try_get::<String, _>("completion_window")?,
        "status": status,
        "output_file_id": row.try_get::<Option<String>, _>("output_file_id")?,
        "error_file_id": row.try_get::<Option<String>, _>("error_file_id")?,
        "created_at": ts(&row, "created_at")?,
        "in_progress_at": ts(&row, "created_at")?,
        "expires_at": ts(&row, "expires_at")?,
        "finalizing_at": ts(&row, "finalizing_at")?,
        "completed_at": ts(&row, "completed_at")?,
        "failed_at": null,
        "expired_at": ts(&row, "expired_at")?,
        "cancelling_at": ts(&row, "cancelling_at")?,
        "cancelled_at": ts(&row, "cancelled_at")?,
        "request_counts": {
            "total": row.try_get::<i64, _>("total")?,
            "completed": row.try_get::<i64, _>("completed")?,
            "failed": row.try_get::<i64, _>("failed")?,
        },
        "metadata": row.try_get::<Option<Value>, _>("metadata")?,
        "enclavo": {
            "ceiling_micro_usd": row.try_get::<i64, _>("ceiling_micro")?,
            "charged_micro_usd": row.try_get::<i64, _>("charged")?,
            "discount_bps": state.cfg.batch.discount_bps,
        },
    }))
}

pub async fn retrieve(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    Ok(Json(batch_object(&state, &id, account.id).await?))
}

pub async fn list(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM batches WHERE account_id = $1 ORDER BY created_at DESC LIMIT 100")
            .bind(account.id)
            .fetch_all(&state.db)
            .await?;
    let mut data = Vec::with_capacity(ids.len());
    for id in &ids {
        data.push(batch_object(&state, id, account.id).await?);
    }
    Ok(Json(json!({
        "object": "list",
        "data": data,
        "first_id": ids.first(),
        "last_id": ids.last(),
        "has_more": false,
    })))
}

pub async fn cancel(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let account = billing::require_account(&state, &headers).await?;
    let updated = sqlx::query(
        "UPDATE batches SET status = 'cancelling', cancelling_at = now()
         WHERE id = $1 AND account_id = $2 AND status = 'in_progress'",
    )
    .bind(&id)
    .bind(account.id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if updated == 0 {
        // Unknown ids get a 404 from batch_object; finished ones are returned as they are.
        return Ok(Json(batch_object(&state, &id, account.id).await?));
    }
    batch::wind_down(&state, &id, "cancelled").await.map_err(ApiError::internal)?;
    Ok(Json(batch_object(&state, &id, account.id).await?))
}
