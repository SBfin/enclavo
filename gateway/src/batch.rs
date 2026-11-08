//! Batch scheduler: runs queued batch requests in the gaps live traffic leaves, earliest deadline
//! first, on the cheapest healthy route, charging the discounted price against the batch's hold.

use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use sqlx::Row;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    Shared,
    api::parse_chat,
    catalog::{Preferences, Sort},
    error::ApiError,
    ledger::{self, UsageRecord},
    pricing::{Tier, Usage, estimate_tokens},
};

/// How long a claimed item may run before another worker may take it over: longer than the
/// slowest possible run (every attempt hitting the slowest provider's timeout), plus slack.
fn item_lease_secs(state: &Shared) -> f64 {
    let slowest = state.catalog.providers.values().map(|p| p.timeout.as_secs()).max().unwrap_or(600);
    let per_attempt = slowest + state.cfg.router.first_byte_timeout_secs;
    (per_attempt * state.cfg.router.max_attempts as u64 + 300) as f64
}

/// Batch work always goes to the cheapest route that satisfies the caller's other preferences.
pub fn batch_prefs(mut prefs: Preferences) -> Preferences {
    prefs.sort = Sort::Price;
    prefs
}

struct Item {
    id: i64,
    batch_id: String,
    custom_id: String,
    body: Value,
    attempts: i32,
    account_id: Uuid,
    hold_id: Uuid,
}

/// Runs until shutdown starts; items already running finish under `state.tasks`.
pub async fn run(state: Shared) {
    let cfg = state.cfg.batch.clone();
    let slots = Arc::new(Semaphore::new(cfg.max_concurrency));
    let idle = Duration::from_millis(cfg.poll_interval_ms);
    let pause = || async {
        tokio::select! {
            _ = tokio::time::sleep(idle) => {}
            _ = state.shutdown.cancelled() => {}
        }
    };
    while !state.shutdown.is_cancelled() {
        if let Err(err) = maintain(&state).await {
            tracing::error!(error = %err, "batch maintenance failed");
        }
        let free = slots.available_permits();
        if free == 0 || state.router.live_inflight() >= cfg.live_busy_threshold {
            pause().await;
            continue;
        }
        match claim(&state, free as i64).await {
            Ok(items) if items.is_empty() => pause().await,
            Ok(items) => {
                for item in items {
                    let permit = slots.clone().acquire_owned().await.expect("semaphore is never closed");
                    let state = state.clone();
                    let tasks = state.tasks.clone();
                    tasks.spawn(async move {
                        let batch_id = item.batch_id.clone();
                        if let Err(err) = run_item(&state, item).await {
                            tracing::error!(batch = %batch_id, error = %err, "batch item bookkeeping failed");
                        }
                        drop(permit);
                        if let Err(err) = try_finalize(&state, &batch_id).await {
                            tracing::error!(batch = %batch_id, error = %err, "batch finalize failed");
                        }
                    });
                }
            }
            Err(err) => {
                tracing::error!(error = %err, "claiming batch items failed");
                pause().await;
            }
        }
    }
    tracing::info!("batch scheduler stopped taking new work");
}

/// Recovers items from crashed workers, expires batches past their deadline, finishes cancels.
async fn maintain(state: &Shared) -> sqlx::Result<()> {
    sqlx::query("UPDATE batch_items SET status = 'pending', locked_until = NULL WHERE status = 'running' AND locked_until < now()")
        .execute(&state.db)
        .await?;
    let expired: Vec<String> =
        sqlx::query_scalar("SELECT id FROM batches WHERE status = 'in_progress' AND expires_at < now() LIMIT 100")
            .fetch_all(&state.db)
            .await?;
    for id in expired {
        wind_down(state, &id, "expired").await?;
    }
    let cancelling: Vec<String> =
        sqlx::query_scalar("SELECT id FROM batches WHERE status = 'cancelling' LIMIT 100").fetch_all(&state.db).await?;
    for id in cancelling {
        wind_down(state, &id, "cancelled").await?;
    }
    Ok(())
}

/// Stops pending items with `item_status` (expired or cancelled) and finalizes if nothing is running.
pub async fn wind_down(state: &Shared, batch_id: &str, item_status: &str) -> sqlx::Result<()> {
    let error = json!({ "code": format!("batch_{item_status}"), "message": format!("The batch was {item_status} before this request ran.") });
    sqlx::query(
        "UPDATE batch_items SET status = $2, error = $3, updated_at = now() WHERE batch_id = $1 AND status = 'pending'",
    )
    .bind(batch_id)
    .bind(item_status)
    .bind(error)
    .execute(&state.db)
    .await?;
    try_finalize(state, batch_id).await
}

async fn claim(state: &Shared, limit: i64) -> sqlx::Result<Vec<Item>> {
    let rows = sqlx::query(
        "UPDATE batch_items i SET status = 'running', attempts = i.attempts + 1, updated_at = now(),
                locked_until = now() + make_interval(secs => $2)
         FROM batches b
         WHERE i.batch_id = b.id AND i.id IN (
             SELECT i2.id FROM batch_items i2 JOIN batches b2 ON b2.id = i2.batch_id
             WHERE i2.status = 'pending' AND i2.next_attempt_at <= now() AND b2.status = 'in_progress'
             ORDER BY b2.expires_at, i2.id
             LIMIT $1
             FOR UPDATE OF i2 SKIP LOCKED)
         RETURNING i.id, i.batch_id, i.custom_id, i.body, i.attempts, b.account_id, b.hold_id",
    )
    .bind(limit)
    .bind(item_lease_secs(state))
    .fetch_all(&state.db)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(Item {
                id: r.try_get("id")?,
                batch_id: r.try_get("batch_id")?,
                custom_id: r.try_get("custom_id")?,
                body: r.try_get("body")?,
                attempts: r.try_get("attempts")?,
                account_id: r.try_get("account_id")?,
                hold_id: r.try_get("hold_id")?,
            })
        })
        .collect()
}

async fn run_item(state: &Shared, item: Item) -> sqlx::Result<()> {
    let request_id = format!("batch_req_{}", item.id);
    let outcome = async {
        let chat = parse_chat(item.body.clone())?;
        let routes = state.catalog.candidates(&chat.model, &batch_prefs(chat.prefs), |r| state.router.latency_ms(r))?;
        let ceiling = state.pricing.ceiling(&routes, &chat.body, Tier::Batch);
        let answer = state.upstream.complete(&state.router, &routes, &ceiling.completion_caps, &chat.body).await?;
        let usage = answer.body.get("usage").and_then(Usage::from_openai).unwrap_or_else(|| Usage {
            prompt_tokens: ceiling.prompt_bound,
            completion_tokens: answer.body.get("choices").map(|c| estimate_tokens(&c.to_string())).unwrap_or(0),
            estimated: true,
        });
        let cost = state.pricing.cost(&answer.route, usage, Tier::Batch).min(ceiling.micro);
        Ok::<_, ApiError>((answer, usage, cost))
    };

    match outcome.await {
        Ok((answer, usage, cost)) => {
            let mut body = answer.body;
            if let Some(obj) = body.as_object_mut() {
                obj.insert("model".into(), Value::String(answer.route.id.clone()));
            }
            let response = json!({ "status_code": 200, "request_id": request_id, "body": body });
            let mut tx = state.db.begin().await?;
            if !still_owned(&mut tx, &item).await? {
                tracing::warn!(batch = %item.batch_id, custom_id = %item.custom_id, "item was reclaimed; dropping this result");
                return Ok(());
            }
            let reference = format!("{}:{}", item.batch_id, item.custom_id);
            let charged = ledger::charge_from_hold(&mut tx, item.hold_id, cost as i64, &reference).await?;
            ledger::record_usage(
                &mut tx,
                &UsageRecord {
                    request_id: &request_id,
                    account_id: Some(item.account_id),
                    payment_id: None,
                    route_id: &answer.route.id,
                    tier: Tier::Batch.as_str(),
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    estimated: usage.estimated,
                    cost_micro: charged,
                },
            )
            .await?;
            sqlx::query(
                "UPDATE batch_items SET status = 'succeeded', response = $2, route_id = $3, cost_micro = $4,
                        locked_until = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(item.id)
            .bind(response)
            .bind(&answer.route.id)
            .bind(charged)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            metrics::counter!("enclavo_batch_items_total", "outcome" => "succeeded").increment(1);
        }
        Err(err) => {
            let status = err.status().as_u16();
            let (retryable, response, error) = match err {
                // The provider rejected the request itself; retrying won't help.
                ApiError::Upstream { status, body } => (
                    false,
                    Some(json!({ "status_code": status.as_u16(), "request_id": request_id, "body": body })),
                    json!({ "code": "upstream_rejected", "message": "The provider rejected this request." }),
                ),
                ApiError::Status { code, message, .. } => {
                    let retryable = matches!(code, "no_route_available" | "upstream_failed");
                    (retryable, None, json!({ "code": code, "message": message }))
                }
                ApiError::PaymentRequired(_) => {
                    (false, None, json!({ "code": "payment_required", "message": "unexpected" }))
                }
            };
            if retryable && item.attempts < state.cfg.batch.max_item_attempts {
                let backoff = 5.0 * 2f64.powi(item.attempts);
                sqlx::query(
                    "UPDATE batch_items SET status = 'pending', locked_until = NULL, error = $2, updated_at = now(),
                            next_attempt_at = now() + make_interval(secs => $3)
                     WHERE id = $1 AND status = 'running' AND attempts = $4",
                )
                .bind(item.id)
                .bind(&error)
                .bind(backoff)
                .bind(item.attempts)
                .execute(&state.db)
                .await?;
                metrics::counter!("enclavo_batch_items_total", "outcome" => "retried").increment(1);
            } else {
                sqlx::query(
                    "UPDATE batch_items SET status = 'failed', response = $2, error = $3, locked_until = NULL,
                            updated_at = now() WHERE id = $1 AND status = 'running' AND attempts = $4",
                )
                .bind(item.id)
                .bind(response)
                .bind(&error)
                .bind(item.attempts)
                .execute(&state.db)
                .await?;
                tracing::warn!(batch = %item.batch_id, custom_id = %item.custom_id, status, "batch item failed");
                metrics::counter!("enclavo_batch_items_total", "outcome" => "failed").increment(1);
            }
        }
    }
    Ok(())
}

/// Locks the item if this worker still holds its claim (same attempt, still running).
async fn still_owned(conn: &mut sqlx::PgConnection, item: &Item) -> sqlx::Result<bool> {
    let owned =
        sqlx::query("SELECT 1 FROM batch_items WHERE id = $1 AND status = 'running' AND attempts = $2 FOR UPDATE")
            .bind(item.id)
            .bind(item.attempts)
            .fetch_optional(conn)
            .await?;
    Ok(owned.is_some())
}

/// Once no item is pending or running: writes the output and error files, closes the batch and
/// returns what is left of the hold. Safe to call any number of times, from any worker.
pub async fn try_finalize(state: &Shared, batch_id: &str) -> sqlx::Result<()> {
    let mut tx = state.db.begin().await?;
    let Some(row) = sqlx::query(
        "SELECT status, account_id, hold_id FROM batches
         WHERE id = $1 AND status IN ('in_progress', 'cancelling') FOR UPDATE",
    )
    .bind(batch_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(());
    };
    let open: i64 =
        sqlx::query_scalar("SELECT count(*) FROM batch_items WHERE batch_id = $1 AND status IN ('pending', 'running')")
            .bind(batch_id)
            .fetch_one(&mut *tx)
            .await?;
    if open > 0 {
        return Ok(());
    }
    let status: String = row.try_get("status")?;
    let account: Uuid = row.try_get("account_id")?;
    let hold_id: Uuid = row.try_get("hold_id")?;

    let items =
        sqlx::query("SELECT id, custom_id, status, response, error FROM batch_items WHERE batch_id = $1 ORDER BY line")
            .bind(batch_id)
            .fetch_all(&mut *tx)
            .await?;
    let (mut output, mut errors) = (String::new(), String::new());
    let mut any_expired = false;
    for item in &items {
        let id: i64 = item.try_get("id")?;
        let item_status: String = item.try_get("status")?;
        any_expired |= item_status == "expired";
        let line = json!({
            "id": format!("batch_req_{id}"),
            "custom_id": item.try_get::<String, _>("custom_id")?,
            "response": item.try_get::<Option<Value>, _>("response")?,
            "error": if item_status == "succeeded" { Value::Null } else { item.try_get::<Option<Value>, _>("error")?.unwrap_or(Value::Null) },
        });
        let target = if item_status == "succeeded" { &mut output } else { &mut errors };
        target.push_str(&line.to_string());
        target.push('\n');
    }

    let new_file = |content: &str| (!content.is_empty()).then(|| format!("file-{}", Uuid::new_v4().simple()));
    let output_file = new_file(&output);
    let error_file = new_file(&errors);
    for (id, content, suffix) in [(&output_file, &output, "output"), (&error_file, &errors, "errors")] {
        let Some(id) = id else { continue };
        sqlx::query("INSERT INTO files (id, account_id, purpose, filename, bytes, content) VALUES ($1, $2, 'batch_output', $3, $4, $5)")
            .bind(id)
            .bind(account)
            .bind(format!("{batch_id}_{suffix}.jsonl"))
            .bind(content.len() as i64)
            .bind(content.as_bytes())
            .execute(&mut *tx)
            .await?;
    }

    let final_status = if status == "cancelling" {
        "cancelled"
    } else if any_expired {
        "expired"
    } else {
        "completed"
    };
    sqlx::query(
        "UPDATE batches SET status = $2, output_file_id = $3, error_file_id = $4, finalizing_at = now(),
                completed_at = CASE WHEN $2 = 'completed' THEN now() END,
                expired_at = CASE WHEN $2 = 'expired' THEN now() END,
                cancelled_at = CASE WHEN $2 = 'cancelled' THEN now() END
         WHERE id = $1",
    )
    .bind(batch_id)
    .bind(final_status)
    .bind(&output_file)
    .bind(&error_file)
    .execute(&mut *tx)
    .await?;
    ledger::release_hold(&mut tx, hold_id).await?;
    tx.commit().await?;
    tracing::info!(batch = batch_id, status = final_status, "batch finalized");
    Ok(())
}
