//! Prepaid balances, holds and the usage journal. Every balance change writes a ledger entry
//! in the same transaction.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

pub const API_KEY_PREFIX: &str = "ek_";

pub fn hash_key(key: &str) -> Vec<u8> {
    Sha256::digest(key.as_bytes()).to_vec()
}

/// A new API key: 244 random bits from two v4 UUIDs.
pub fn new_api_key() -> String {
    format!("{API_KEY_PREFIX}{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Account {
    pub id: Uuid,
    pub balance_micro: i64,
    pub held_micro: i64,
    pub created_at: DateTime<Utc>,
}

pub async fn account_by_key(pool: &PgPool, key: &str) -> sqlx::Result<Option<Account>> {
    let row = sqlx::query(
        "SELECT a.id, a.balance_micro, a.held_micro, a.created_at FROM accounts a
        WHERE (a.api_key_hash = $1 AND NOT EXISTS (SELECT 1 FROM api_keys k WHERE k.key_hash = $1))
           OR EXISTS (SELECT 1 FROM api_keys k WHERE k.key_hash = $1 AND k.account_id = a.id AND k.revoked_at IS NULL)",
    )
    .bind(hash_key(key))
    .fetch_optional(pool)
    .await?;
    row.map(|r| {
        Ok(Account {
            id: r.try_get("id")?,
            balance_micro: r.try_get("balance_micro")?,
            held_micro: r.try_get("held_micro")?,
            created_at: r.try_get("created_at")?,
        })
    })
    .transpose()
}

/// Creates an empty account and returns its id and API key (the only time the key is visible).
pub async fn create_account(conn: &mut PgConnection) -> sqlx::Result<(Uuid, String)> {
    let id = Uuid::new_v4();
    let key = new_api_key();
    sqlx::query("INSERT INTO accounts (id, api_key_hash) VALUES ($1, $2)")
        .bind(id)
        .bind(hash_key(&key))
        .execute(conn)
        .await?;
    Ok((id, key))
}

/// Issue an explicitly operator-funded spending allowance, without inventing a payment receipt.
pub async fn grant_operator_credit(pool: &PgPool, amount: u64) -> anyhow::Result<(Uuid, String)> {
    anyhow::ensure!(amount > 0 && amount <= i64::MAX as u64, "credit must be positive and fit in micro-USD");
    let mut tx = pool.begin().await?;
    let (id, key) = create_account(&mut tx).await?;
    credit(&mut tx, id, amount as i64, &format!("operator:grant:{}", Uuid::new_v4())).await?;
    tx.commit().await?;
    Ok((id, key))
}

pub async fn grant_existing_credit(pool: &PgPool, account: Uuid, amount: u64) -> anyhow::Result<()> {
    anyhow::ensure!(amount > 0 && amount <= i64::MAX as u64, "credit must be positive and fit in micro-USD");
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE").bind(account).fetch_one(&mut *tx).await?;
    credit(&mut tx, account, amount as i64, &format!("operator:grant:{}", Uuid::new_v4())).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn credit(conn: &mut PgConnection, account: Uuid, amount: i64, reference: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE accounts SET balance_micro = balance_micro + $2 WHERE id = $1")
        .bind(account)
        .bind(amount)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO ledger_entries (account_id, kind, amount_micro, reference) VALUES ($1, 'topup', $2, $3)")
        .bind(account)
        .bind(amount)
        .bind(reference)
        .execute(conn)
        .await?;
    Ok(())
}

/// Moves `amount` from the balance into a hold. Returns `None` when the balance is too low.
pub async fn hold(pool: &PgPool, account: Uuid, amount: i64, kind: &str, ttl_secs: i64) -> sqlx::Result<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    let moved = sqlx::query(
        "UPDATE accounts SET balance_micro = balance_micro - $2, held_micro = held_micro + $2
         WHERE id = $1 AND balance_micro >= $2",
    )
    .bind(account)
    .bind(amount)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if moved == 0 {
        return Ok(None);
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO holds (id, account_id, kind, amount_micro, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
    )
    .bind(id)
    .bind(account)
    .bind(kind)
    .bind(amount)
    .bind(ttl_secs as f64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// Takes up to `cost` out of an active hold as a charge. Returns what was charged.
pub async fn charge_from_hold(conn: &mut PgConnection, hold_id: Uuid, cost: i64, reference: &str) -> sqlx::Result<i64> {
    let Some((account, remaining)) = lock_active_hold(conn, hold_id).await? else { return Ok(0) };
    let charged = remaining.min(cost.max(0));
    if charged == 0 {
        return Ok(0);
    }
    sqlx::query("UPDATE holds SET amount_micro = amount_micro - $2 WHERE id = $1")
        .bind(hold_id)
        .bind(charged)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE accounts SET held_micro = held_micro - $2 WHERE id = $1")
        .bind(account)
        .bind(charged)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO ledger_entries (account_id, kind, amount_micro, reference) VALUES ($1, 'charge', $2, $3)")
        .bind(account)
        .bind(-charged)
        .bind(reference)
        .execute(&mut *conn)
        .await?;
    Ok(charged)
}

/// Charges up to `amount` straight from the balance, for a request whose hold is already gone.
pub async fn charge_balance(conn: &mut PgConnection, account: Uuid, amount: i64, reference: &str) -> sqlx::Result<i64> {
    let charged: Option<i64> = sqlx::query_scalar(
        "WITH a AS (SELECT id, LEAST(balance_micro, $2) AS take FROM accounts WHERE id = $1 FOR UPDATE)
         UPDATE accounts SET balance_micro = balance_micro - a.take FROM a WHERE accounts.id = a.id RETURNING a.take",
    )
    .bind(account)
    .bind(amount.max(0))
    .fetch_optional(&mut *conn)
    .await?;
    let charged = charged.unwrap_or(0);
    if charged > 0 {
        sqlx::query(
            "INSERT INTO ledger_entries (account_id, kind, amount_micro, reference) VALUES ($1, 'charge', $2, $3)",
        )
        .bind(account)
        .bind(-charged)
        .bind(reference)
        .execute(conn)
        .await?;
    }
    Ok(charged)
}

/// Locks an active hold for the rest of the transaction; returns its account and remaining amount.
async fn lock_active_hold(conn: &mut PgConnection, hold_id: Uuid) -> sqlx::Result<Option<(Uuid, i64)>> {
    let row = sqlx::query("SELECT account_id, amount_micro FROM holds WHERE id = $1 AND status = 'active' FOR UPDATE")
        .bind(hold_id)
        .fetch_optional(&mut *conn)
        .await?;
    row.map(|r| Ok((r.try_get("account_id")?, r.try_get("amount_micro")?))).transpose()
}

/// Closes a hold and returns whatever is left of it to the balance.
pub async fn release_hold(conn: &mut PgConnection, hold_id: Uuid) -> sqlx::Result<()> {
    let Some((account, remaining)) = lock_active_hold(conn, hold_id).await? else { return Ok(()) };
    sqlx::query("UPDATE holds SET status = 'closed', amount_micro = 0 WHERE id = $1")
        .bind(hold_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE accounts SET held_micro = held_micro - $2, balance_micro = balance_micro + $2 WHERE id = $1")
        .bind(account)
        .bind(remaining)
        .execute(conn)
        .await?;
    Ok(())
}

/// Releases request holds whose request can no longer be running (e.g. after a crash).
pub async fn release_expired_holds(pool: &PgPool) -> sqlx::Result<u64> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM holds WHERE status = 'active' AND kind = 'request' AND expires_at < now() LIMIT 500",
    )
    .fetch_all(pool)
    .await?;
    for id in &ids {
        let mut tx = pool.begin().await?;
        release_hold(&mut tx, *id).await?;
        tx.commit().await?;
    }
    Ok(ids.len() as u64)
}

pub struct UsageRecord<'a> {
    pub capacity_contract_id: Option<Uuid>,
    pub request_id: &'a str,
    pub account_id: Option<Uuid>,
    pub payment_id: Option<Uuid>,
    pub route_id: &'a str,
    pub tier: &'a str,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub estimated: bool,
    pub cost_micro: i64,
}

pub async fn record_usage(conn: &mut PgConnection, u: &UsageRecord<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO usage_events (request_id, account_id, payment_id, route_id, tier, prompt_tokens,
                                   completion_tokens, usage_estimated, cost_micro, capacity_contract_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(u.request_id)
    .bind(u.account_id)
    .bind(u.payment_id)
    .bind(u.route_id)
    .bind(u.tier)
    .bind(u.prompt_tokens as i64)
    .bind(u.completion_tokens as i64)
    .bind(u.estimated)
    .bind(u.cost_micro)
    .bind(u.capacity_contract_id)
    .execute(conn)
    .await?;
    Ok(())
}
