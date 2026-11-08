//! Who pays for a request, and settling what it actually cost.

use axum::http::HeaderMap;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    AppState,
    config::{PaymentFlow, Scheme},
    error::ApiError,
    ledger::{self, API_KEY_PREFIX, Account, UsageRecord},
    pricing::{Tier, Usage},
    x402::{
        Matched,
        facilitator::FacilitatorUnavailable,
        to_atomic, to_micro,
        types::{PAYMENT_SIGNATURE_HEADER, PaymentPayload, PaymentRequirements, SettleResponse},
    },
};

/// How long a request hold may live before the sweeper returns it to the balance.
const REQUEST_HOLD_SECS: i64 = 30 * 60;

pub enum Payer {
    Account { account_id: Uuid, hold_id: Uuid },
    X402(Box<X402Charge>),
}

pub struct X402Charge {
    payment_id: Uuid,
    payload: PaymentPayload,
    matched: Matched,
}

/// What a finished request used.
pub struct Charge<'a> {
    pub route_id: &'a str,
    pub usage: Usage,
    pub cost_micro: u64,
    pub tier: Tier,
}

#[derive(Debug, Default)]
pub struct Receipt {
    pub charged_micro: u64,
    /// Base64 `PAYMENT-RESPONSE` value for x402 payers.
    pub payment_response: Option<String>,
}

/// The API key in `Authorization: Bearer ek_…`, if any. Other bearer values (SDKs often send a
/// placeholder) are ignored so x402 clients can use stock OpenAI SDKs.
pub fn api_key(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer "))?.trim();
    token.starts_with(API_KEY_PREFIX).then_some(token)
}

pub async fn account(state: &AppState, headers: &HeaderMap) -> Result<Option<Account>, ApiError> {
    match api_key(headers) {
        None => Ok(None),
        Some(key) => match ledger::account_by_key(&state.db, key).await? {
            Some(account) => Ok(Some(account)),
            None => Err(ApiError::unauthorized("Unknown API key.")),
        },
    }
}

pub async fn require_account(state: &AppState, headers: &HeaderMap) -> Result<Account, ApiError> {
    account(state, headers)
        .await?
        .ok_or_else(|| ApiError::unauthorized("This endpoint needs an Enclavo API key (Bearer ek_…)."))
}

fn payment_header(headers: &HeaderMap) -> Result<Option<PaymentPayload>, ApiError> {
    let Some(raw) = headers.get(PAYMENT_SIGNATURE_HEADER) else { return Ok(None) };
    let raw = raw.to_str().map_err(|_| ApiError::bad_request("PAYMENT-SIGNATURE is not ASCII"))?;
    let payload = PaymentPayload::from_header(raw).map_err(ApiError::bad_request)?;
    Ok(Some(payload))
}

/// Replay key for a payment: its network, scheme and signed payload in canonical JSON, so the
/// same authorization re-encoded differently (whitespace, key order, extensions) still collides.
fn replay_key(payload: &PaymentPayload) -> Vec<u8> {
    let key =
        json!({ "network": payload.accepted.network, "scheme": payload.accepted.scheme, "payload": payload.payload });
    let mut canonical = String::new();
    write_canonical(&key, &mut canonical);
    Sha256::digest(canonical.as_bytes()).to_vec()
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn usd(micro: u64) -> String {
    format!("${}.{:06}", micro / 1_000_000, micro % 1_000_000)
}

/// Settles who pays for a request whose cost can reach `ceiling_micro`.
pub async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
    ceiling_micro: u64,
) -> Result<Payer, ApiError> {
    let payment = payment_header(headers)?;
    let mut low_balance = false;
    if let Some(account) = account(state, headers).await? {
        match ledger::hold(&state.db, account.id, ceiling_micro as i64, "request", REQUEST_HOLD_SECS).await? {
            Some(hold_id) => return Ok(Payer::Account { account_id: account.id, hold_id }),
            None => low_balance = true,
        }
    }

    let Some(payload) = payment else {
        if !state.payments.accepts(&Scheme::Upto) {
            return Err(if low_balance {
                ApiError::payment_failed(format!("Balance is below this request's ceiling of {}.", usd(ceiling_micro)))
            } else {
                ApiError::unauthorized("This gateway needs an Enclavo API key (Bearer ek_…).")
            });
        }
        let error = if low_balance {
            format!(
                "Balance is below this request's ceiling of {}; pay for it with x402 or top up.",
                usd(ceiling_micro)
            )
        } else {
            "PAYMENT-SIGNATURE header is required".to_string()
        };
        return Err(state.payments.required(
            path,
            "LLM inference, charged per token up to this maximum",
            &Scheme::Upto,
            ceiling_micro,
            &error,
        ));
    };

    let matched = state.payments.match_payload(&payload, &Scheme::Upto, ceiling_micro).map_err(|reason| {
        state.payments.required(
            path,
            "LLM inference, charged per token up to this maximum",
            &Scheme::Upto,
            ceiling_micro,
            &reason,
        )
    })?;

    let payment_id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO payments (id, signature_hash, purpose, scheme, network, authorized_atomic, status, payload, requirements)
         VALUES ($1, $2, 'request', $3, $4, $5::numeric, 'authorized', $6, $7)
         ON CONFLICT (signature_hash) DO NOTHING",
    )
    .bind(payment_id)
    .bind(replay_key(&payload))
    .bind(&matched.requirements.scheme)
    .bind(&matched.network.network)
    .bind(matched.authorized_atomic.to_string())
    .bind(serde_json::to_value(&payload).map_err(ApiError::internal)?)
    .bind(serde_json::to_value(&matched.requirements).map_err(ApiError::internal)?)
    .execute(&state.db)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(ApiError::conflict("This payment authorization was already used; sign a new one."));
    }

    // Something must be checked before the request runs (spec §6.1).
    let check = match matched.flow {
        PaymentFlow::Authorization => state
            .facilitator
            .verify(&payload, &matched.requirements)
            .await
            .map(|v| (v.is_valid, v.payer, v.invalid_reason.or(v.invalid_message))),
        PaymentFlow::Escrow => state
            .facilitator
            .settle(&payload, &matched.requirements)
            .await
            .map(|s| (s.success, s.payer, s.error_reason.or(s.error_message))),
    };
    match check {
        Ok((true, payer, _)) => {
            sqlx::query("UPDATE payments SET payer = $2, updated_at = now() WHERE id = $1")
                .bind(payment_id)
                .bind(payer)
                .execute(&state.db)
                .await?;
            metrics::counter!("enclavo_payments_total", "purpose" => "request", "outcome" => "authorized").increment(1);
            Ok(Payer::X402(Box::new(X402Charge { payment_id, payload, matched })))
        }
        Ok((false, _, reason)) => {
            let reason = reason.unwrap_or_else(|| "payment rejected".into());
            mark_payment(state, payment_id, "failed", &reason).await;
            metrics::counter!("enclavo_payments_total", "purpose" => "request", "outcome" => "rejected").increment(1);
            Err(state.payments.required(
                path,
                "LLM inference, charged per token up to this maximum",
                &Scheme::Upto,
                ceiling_micro,
                &reason,
            ))
        }
        Err(FacilitatorUnavailable(reason)) => {
            mark_payment(state, payment_id, "failed", &reason).await;
            Err(ApiError::payment_unavailable())
        }
    }
}

async fn mark_payment(state: &AppState, payment_id: Uuid, status: &str, error: &str) {
    let result = sqlx::query("UPDATE payments SET status = $2, error = $3, updated_at = now() WHERE id = $1")
        .bind(payment_id)
        .bind(status)
        .bind(error)
        .execute(&state.db)
        .await;
    if let Err(err) = result {
        tracing::error!(%payment_id, error = %err, "could not record payment status");
    }
}

/// Charges what a request used (`None` when it produced nothing) and releases the rest.
/// Never fails the request: problems are logged and left for the sweepers.
pub async fn finalize(state: &AppState, payer: Payer, request_id: &str, charge: Option<Charge<'_>>) -> Receipt {
    let result = match payer {
        Payer::Account { account_id, hold_id } => {
            finalize_account(state, account_id, hold_id, request_id, charge).await
        }
        Payer::X402(x) => finalize_x402(state, *x, request_id, charge).await,
    };
    match result {
        Ok(receipt) => {
            metrics::counter!("enclavo_charged_micro_usd_total").increment(receipt.charged_micro);
            receipt
        }
        Err(err) => {
            tracing::error!(request_id, error = %err, "could not finalize payment");
            Receipt::default()
        }
    }
}

async fn finalize_account(
    state: &AppState,
    account_id: Uuid,
    hold_id: Uuid,
    request_id: &str,
    charge: Option<Charge<'_>>,
) -> sqlx::Result<Receipt> {
    // A failure here would leave the hold for the sweeper, i.e. free inference, so retry.
    let mut attempt = 0;
    loop {
        match try_finalize_account(state, account_id, hold_id, request_id, charge.as_ref()).await {
            Ok(receipt) => return Ok(receipt),
            Err(err) if attempt < 3 => {
                attempt += 1;
                tracing::warn!(request_id, error = %err, attempt, "retrying account finalize");
                tokio::time::sleep(std::time::Duration::from_millis(100 * attempt)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn try_finalize_account(
    state: &AppState,
    account_id: Uuid,
    hold_id: Uuid,
    request_id: &str,
    charge: Option<&Charge<'_>>,
) -> sqlx::Result<Receipt> {
    let mut tx = state.db.begin().await?;
    let mut charged = 0;
    if let Some(c) = charge {
        let cost = c.cost_micro as i64;
        charged = ledger::charge_from_hold(&mut tx, hold_id, cost, request_id).await?;
        if charged < cost {
            // The hold was swept (the request outlived it); collect from the balance instead.
            charged += ledger::charge_balance(&mut tx, account_id, cost - charged, request_id).await?;
        }
        ledger::record_usage(&mut tx, &usage_record(request_id, Some(account_id), None, c, charged)).await?;
    }
    ledger::release_hold(&mut tx, hold_id).await?;
    tx.commit().await?;
    Ok(Receipt { charged_micro: charged as u64, payment_response: None })
}

fn usage_record<'a>(
    request_id: &'a str,
    account_id: Option<Uuid>,
    payment_id: Option<Uuid>,
    c: &Charge<'a>,
    charged: i64,
) -> UsageRecord<'a> {
    UsageRecord {
        request_id,
        account_id,
        payment_id,
        route_id: c.route_id,
        tier: c.tier.as_str(),
        prompt_tokens: c.usage.prompt_tokens,
        completion_tokens: c.usage.completion_tokens,
        estimated: c.usage.estimated,
        cost_micro: charged,
    }
}

async fn finalize_x402(
    state: &AppState,
    x: X402Charge,
    request_id: &str,
    charge: Option<Charge<'_>>,
) -> sqlx::Result<Receipt> {
    let decimals = x.matched.network.asset_decimals;
    let ceiling = to_micro(x.matched.authorized_atomic, decimals);
    let cost = charge.as_ref().map(|c| c.cost_micro.min(ceiling)).unwrap_or(0);

    let receipt = if cost == 0 && x.matched.flow == PaymentFlow::Authorization {
        // Nothing to collect: an unsettled authorization simply expires, so no transaction is spent.
        mark_payment(state, x.payment_id, "released", "").await;
        Receipt::default()
    } else {
        let mut requirements = x.matched.requirements.clone();
        requirements.amount = to_atomic(cost, decimals).to_string();
        // Bookkeeping failures are logged, never allowed to skip the settlement itself.
        let recorded = sqlx::query(
            "UPDATE payments SET status = 'settling', settle_atomic = $2::numeric, charged_micro = $3,
                    requirements = $4, updated_at = now() WHERE id = $1",
        )
        .bind(x.payment_id)
        .bind(&requirements.amount)
        .bind(cost as i64)
        .bind(serde_json::to_value(&requirements).unwrap_or(Value::Null))
        .execute(&state.db)
        .await;
        if let Err(err) = recorded {
            tracing::error!(payment_id = %x.payment_id, error = %err, "could not record settlement amount");
        }
        let (status, settled) = settle_and_record(state, x.payment_id, &x.payload, &requirements, false).await;
        Receipt {
            charged_micro: if status == SettleStatus::Settled { cost } else { 0 },
            payment_response: settled.map(|s| s.to_header()),
        }
    };

    if let Some(c) = &charge {
        let mut conn = state.db.acquire().await?;
        ledger::record_usage(
            &mut conn,
            &usage_record(request_id, None, Some(x.payment_id), c, receipt.charged_micro as i64),
        )
        .await?;
    }
    Ok(receipt)
}

/// Where a settlement stands after one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleStatus {
    Settled,
    /// Not final: the facilitator was unreachable, or broadcast without seeing confirmation
    /// (`settlement_pending`). Retried with the identical request, which facilitators dedupe.
    Pending,
    Failed,
    /// The outcome can't be known from here; someone has to check the chain.
    NeedsReview,
}

impl SettleStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::Pending => "settle_pending",
            Self::Failed => "failed",
            Self::NeedsReview => "needs_review",
        }
    }
}

const SETTLEMENT_PENDING: &str = "settlement_pending";

/// Settles and records the outcome. A settled top-up is credited in the same transaction that
/// marks it settled, so it is credited exactly once. `retry` is set when an earlier attempt's
/// outcome was unknown: a terminal error then may just mean it already went through.
async fn settle_and_record(
    state: &AppState,
    payment_id: Uuid,
    payload: &PaymentPayload,
    requirements: &PaymentRequirements,
    retry: bool,
) -> (SettleStatus, Option<SettleResponse>) {
    let outcome = state.facilitator.settle(payload, requirements).await;
    let reason_of = |s: &SettleResponse| {
        s.error_reason.clone().or(s.error_message.clone()).unwrap_or_else(|| "settlement failed".into())
    };
    let (status, error, response) = match outcome {
        Ok(s) if s.success => (SettleStatus::Settled, None, Some(s)),
        Ok(s) if s.error_reason.as_deref() == Some(SETTLEMENT_PENDING) => {
            tracing::warn!(%payment_id, transaction = %s.transaction, "settlement broadcast but not yet confirmed");
            (SettleStatus::Pending, Some(SETTLEMENT_PENDING.to_string()), Some(s))
        }
        Ok(s) if retry => {
            let reason = reason_of(&s);
            tracing::error!(%payment_id, reason, "settlement retry rejected; an earlier attempt may have settled");
            (SettleStatus::NeedsReview, Some(reason), Some(s))
        }
        Ok(s) => {
            let reason = reason_of(&s);
            tracing::error!(%payment_id, reason, "settlement rejected");
            (SettleStatus::Failed, Some(reason), Some(s))
        }
        Err(FacilitatorUnavailable(reason)) => {
            tracing::warn!(%payment_id, reason, "facilitator unavailable; settlement will be retried");
            (SettleStatus::Pending, Some(reason), None)
        }
    };
    let transaction = response.as_ref().map(|s| s.transaction.clone()).filter(|t| !t.is_empty());
    let payer = response.as_ref().and_then(|s| s.payer.clone());
    if let Err(err) = record_settlement(state, payment_id, status, transaction, payer, error).await {
        tracing::error!(%payment_id, status = status.as_str(), error = %err, "could not record settlement outcome");
    }
    metrics::counter!("enclavo_settlements_total", "outcome" => status.as_str()).increment(1);
    if status == SettleStatus::NeedsReview {
        metrics::counter!("enclavo_payments_needs_review_total").increment(1);
    }
    (status, response)
}

async fn record_settlement(
    state: &AppState,
    payment_id: Uuid,
    status: SettleStatus,
    transaction: Option<String>,
    payer: Option<String>,
    error: Option<String>,
) -> sqlx::Result<()> {
    let mut tx = state.db.begin().await?;
    let row = sqlx::query(
        "UPDATE payments SET status = $2, transaction = COALESCE($3, transaction), payer = COALESCE($4, payer),
                error = $5, settle_attempts = settle_attempts + 1, updated_at = now()
         WHERE id = $1 AND status <> 'settled'
         RETURNING purpose, account_id, charged_micro",
    )
    .bind(payment_id)
    .bind(status.as_str())
    .bind(transaction)
    .bind(payer)
    .bind(error)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(row) = row
        && status == SettleStatus::Settled
        && row.try_get::<String, _>("purpose")? == "topup"
    {
        let account: Uuid = row.try_get("account_id")?;
        let amount: i64 = row.try_get("charged_micro")?;
        ledger::credit(&mut tx, account, amount, &format!("payment:{payment_id}")).await?;
    }
    tx.commit().await
}

/// Retries settlements whose outcome isn't final yet. Rows are claimed (`settling`) so several
/// gateway instances never work the same one.
pub async fn retry_pending_settlements(state: &AppState) -> sqlx::Result<u64> {
    let max = state.payments.config().max_settle_attempts;
    // A row left `settling` lost its worker mid-settle; resending the identical request is safe.
    sqlx::query(
        "UPDATE payments SET status = 'settle_pending', updated_at = now()
         WHERE status = 'settling' AND updated_at < now() - interval '10 minutes'",
    )
    .execute(&state.db)
    .await?;
    // CDP: resend a `settlement_pending` once, then stop and watch the chain. Anything else
    // gets `max_settle_attempts`. Either way an unknown outcome goes to a person, never `failed`.
    let flagged = sqlx::query(
        "UPDATE payments SET status = 'needs_review', updated_at = now()
         WHERE status = 'settle_pending'
           AND (settle_attempts >= $1 OR (error = $2 AND settle_attempts >= 2))",
    )
    .bind(max)
    .bind(SETTLEMENT_PENDING)
    .execute(&state.db)
    .await?
    .rows_affected();
    if flagged > 0 {
        tracing::error!(flagged, "payments with an unknown settlement outcome need review");
        metrics::counter!("enclavo_payments_needs_review_total").increment(flagged);
    }
    let rows = sqlx::query(
        "UPDATE payments SET status = 'settling', updated_at = now()
         WHERE id IN (
             SELECT id FROM payments
             WHERE status = 'settle_pending'
               AND updated_at < now() - make_interval(secs => LEAST(3600, 5 * power(2, settle_attempts)))
             ORDER BY updated_at LIMIT 50
             FOR UPDATE SKIP LOCKED)
         RETURNING id, payload, requirements",
    )
    .fetch_all(&state.db)
    .await?;
    let mut retried = 0;
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let parsed = serde_json::from_value::<PaymentPayload>(row.try_get("payload")?).and_then(|p| {
            Ok((p, serde_json::from_value::<PaymentRequirements>(row.try_get("requirements").unwrap_or(Value::Null))?))
        });
        match parsed {
            Ok((payload, requirements)) => {
                settle_and_record(state, id, &payload, &requirements, true).await;
                retried += 1;
            }
            Err(err) => mark_payment(state, id, "needs_review", &format!("stored payment unreadable: {err}")).await,
        }
    }
    Ok(retried)
}

pub struct TopUp {
    pub account_id: Uuid,
    /// False while the transfer is broadcast but unconfirmed; the credit lands once it settles.
    pub settled: bool,
    /// Set only when the top-up created the account.
    pub api_key: Option<String>,
    pub credited_micro: u64,
    pub payment_response: String,
}

/// Credits a prepaid balance from an `exact` x402 payment, creating the account if needed.
pub async fn top_up(state: &AppState, headers: &HeaderMap, path: &str, amount_micro: u64) -> Result<TopUp, ApiError> {
    let cfg = state.payments.config();
    if amount_micro < cfg.min_topup_micro || amount_micro > cfg.max_topup_micro {
        return Err(ApiError::bad_request(format!(
            "Top-ups must be between {} and {}.",
            usd(cfg.min_topup_micro),
            usd(cfg.max_topup_micro)
        )));
    }
    if !state.payments.accepts(&Scheme::Exact) {
        return Err(ApiError::bad_request("No network is configured for top-ups."));
    }
    let existing = account(state, headers).await?;
    let description = "Prepaid Enclavo inference credit";
    let Some(payload) = payment_header(headers)? else {
        return Err(state.payments.required(
            path,
            description,
            &Scheme::Exact,
            amount_micro,
            "PAYMENT-SIGNATURE header is required",
        ));
    };
    let matched = state
        .payments
        .match_payload(&payload, &Scheme::Exact, amount_micro)
        .map_err(|reason| state.payments.required(path, description, &Scheme::Exact, amount_micro, &reason))?;

    // Account and payment row exist before money moves, so whatever happens next can be
    // settled and credited later from the row alone.
    let mut tx = state.db.begin().await?;
    let (account_id, api_key) = match &existing {
        Some(a) => (a.id, None),
        None => {
            let (id, key) = ledger::create_account(&mut tx).await?;
            (id, Some(key))
        }
    };
    let payment_id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO payments (id, signature_hash, purpose, scheme, network, account_id, authorized_atomic,
                               settle_atomic, charged_micro, status, payload, requirements)
         VALUES ($1, $2, 'topup', 'exact', $3, $4, $5::numeric, $5::numeric, $6, 'authorized', $7, $8)
         ON CONFLICT (signature_hash) DO NOTHING",
    )
    .bind(payment_id)
    .bind(replay_key(&payload))
    .bind(&matched.network.network)
    .bind(account_id)
    .bind(matched.authorized_atomic.to_string())
    .bind(amount_micro as i64)
    .bind(serde_json::to_value(&payload).map_err(ApiError::internal)?)
    .bind(serde_json::to_value(&matched.requirements).map_err(ApiError::internal)?)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(ApiError::conflict("This payment was already used."));
    }
    tx.commit().await?;

    let verdict = match state.facilitator.verify(&payload, &matched.requirements).await {
        Ok(v) => v,
        Err(FacilitatorUnavailable(reason)) => {
            mark_payment(state, payment_id, "failed", &reason).await;
            return Err(ApiError::payment_unavailable());
        }
    };
    if !verdict.is_valid {
        let reason = verdict.invalid_reason.or(verdict.invalid_message).unwrap_or_else(|| "payment rejected".into());
        mark_payment(state, payment_id, "failed", &reason).await;
        return Err(state.payments.required(path, description, &Scheme::Exact, amount_micro, &reason));
    }
    mark_payment(state, payment_id, "settling", "").await;
    let (status, settled) = settle_and_record(state, payment_id, &payload, &matched.requirements, false).await;
    let payment_response = settled.as_ref().map(|s| s.to_header()).unwrap_or_default();
    match status {
        SettleStatus::Settled => {
            metrics::counter!("enclavo_payments_total", "purpose" => "topup", "outcome" => "settled").increment(1);
            Ok(TopUp { account_id, settled: true, api_key, credited_micro: amount_micro, payment_response })
        }
        // The money may be on its way: hand back the key so the credit is usable once it lands.
        SettleStatus::Pending | SettleStatus::NeedsReview => {
            Ok(TopUp { account_id, settled: false, api_key, credited_micro: 0, payment_response })
        }
        SettleStatus::Failed => {
            let reason = settled.and_then(|s| s.error_reason.or(s.error_message)).unwrap_or_else(|| "unknown".into());
            Err(ApiError::payment_failed(format!("Settlement failed: {reason}")))
        }
    }
}
