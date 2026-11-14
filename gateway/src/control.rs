//! Customer account operations authenticated by the dashboard after it verifies the user's session.
use crate::{Shared, api, error::ApiError, ledger};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

pub fn configured_token_hash() -> Option<[u8; 32]> {
    let token = std::env::var("ENCLAVO_CONTROL_TOKEN").ok().filter(|s| !s.is_empty())?;
    Some(token_hash(&token))
}
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token).into()
}

pub fn user(state: &Shared, headers: &HeaderMap) -> Result<String, ApiError> {
    let expected = state
        .control_token_hash
        .ok_or_else(|| ApiError::no_route("Dashboard account management is not configured."))?;
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("Dashboard authorization is required."))?;
    if token_hash(token) != expected {
        return Err(ApiError::unauthorized("Invalid dashboard authorization."));
    }
    let user = headers
        .get("x-enclavo-user-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or_else(|| ApiError::unauthorized("A verified user identity is required."))?;
    if !state.limits.check_customer(user) {
        return Err(ApiError::rate_limited());
    }
    Ok(user.into())
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/internal/account", get(account))
        .route("/internal/account/import", post(import_account))
        .route("/internal/keys", get(keys).post(create_key))
        .route("/internal/keys/{id}", axum::routing::delete(revoke_key))
        .route("/internal/chat/completions", post(chat))
        .route("/internal/market/bids", get(crate::market::bids).post(crate::market::place_bid))
        .route("/internal/market/bids/{id}", axum::routing::delete(crate::market::cancel_bid))
        .route("/internal/market/contracts", get(crate::market::contracts))
}

pub async fn account_id(conn: &mut PgConnection, user: &str, create: bool) -> Result<Option<Uuid>, ApiError> {
    if create {
        sqlx::query("INSERT INTO accounts(id, api_key_hash, owner_user_id) VALUES($1,$2,$3) ON CONFLICT(owner_user_id) DO NOTHING")
            .bind(Uuid::new_v4()).bind(ledger::hash_key(&ledger::new_api_key())).bind(user).execute(&mut *conn).await?;
    }
    Ok(sqlx::query_scalar("SELECT id FROM accounts WHERE owner_user_id=$1").bind(user).fetch_optional(conn).await?)
}

pub async fn owned_account(state: &Shared, user: &str) -> Result<ledger::Account, ApiError> {
    let row = sqlx::query("SELECT id,balance_micro,held_micro,created_at FROM accounts WHERE owner_user_id=$1")
        .bind(user)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::unauthorized("Create an API key or connect an existing account first."))?;
    Ok(ledger::Account {
        id: row.try_get("id")?,
        balance_micro: row.try_get("balance_micro")?,
        held_micro: row.try_get("held_micro")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn account(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = user(&state, &headers)?;
    let mut conn = state.db.acquire().await?;
    let Some(id) = account_id(&mut conn, &user, false).await? else {
        return Ok(Json(json!({"account": null})));
    };
    let row = sqlx::query("SELECT balance_micro,held_micro,created_at FROM accounts WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    let (requests, spent): (i64, Option<i64>) =
        sqlx::query_as("SELECT (SELECT count(*) FROM usage_events WHERE account_id=$1), (SELECT -sum(amount_micro)::bigint FROM ledger_entries WHERE account_id=$1 AND kind='charge')")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    Ok(Json(json!({"account": {"id": id, "balance_micro_usd": row.try_get::<i64,_>("balance_micro")?,
        "held_micro_usd": row.try_get::<i64,_>("held_micro")?, "requests": requests, "spent_micro_usd": spent.unwrap_or(0)}})))
}

async fn keys(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = user(&state, &headers)?;
    let rows = sqlx::query("SELECT k.id,k.name,k.prefix,k.created_at,k.revoked_at FROM api_keys k JOIN accounts a ON a.id=k.account_id WHERE a.owner_user_id=$1 ORDER BY k.created_at DESC")
        .bind(user).fetch_all(&state.db).await?;
    let keys = rows.into_iter().map(|r| Ok(json!({"id": r.try_get::<Uuid,_>("id")?, "name":r.try_get::<String,_>("name")?,
        "prefix":r.try_get::<String,_>("prefix")?, "created_at":r.try_get::<chrono::DateTime<chrono::Utc>,_>("created_at")?,
        "revoked_at":r.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("revoked_at")?}))).collect::<Result<Vec<_>,sqlx::Error>>()?;
    Ok(Json(json!({"keys": keys})))
}

async fn create_key(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = user(&state, &headers)?;
    let name = body["name"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().count() <= 80)
        .ok_or_else(|| ApiError::bad_request("Give the key a name of 1–80 characters."))?;
    let mut tx = state.db.begin().await?;
    let account = account_id(&mut tx, &user, true).await?.unwrap();
    sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE").bind(account).fetch_one(&mut *tx).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE account_id=$1 AND revoked_at IS NULL")
        .bind(account)
        .fetch_one(&mut *tx)
        .await?;
    if count >= 20 {
        return Err(ApiError::bad_request("Revoke an existing key before creating more than 20 active keys."));
    }
    let id = Uuid::new_v4();
    let key = ledger::new_api_key();
    sqlx::query("INSERT INTO api_keys(id,account_id,name,prefix,key_hash) VALUES($1,$2,$3,$4,$5)")
        .bind(id)
        .bind(account)
        .bind(name)
        .bind(&key[..11])
        .bind(ledger::hash_key(&key))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"name":name,"api_key":key,"note":"Copy this key now. It will not be displayed again."})))
}

async fn revoke_key(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = user(&state, &headers)?;
    let result=sqlx::query("UPDATE api_keys k SET revoked_at=COALESCE(k.revoked_at,now()) FROM accounts a WHERE k.id=$1 AND k.account_id=a.id AND a.owner_user_id=$2")
        .bind(id).bind(user).execute(&state.db).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("API key not found."));
    }
    Ok(Json(json!({"revoked":true})))
}

async fn import_account(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = user(&state, &headers)?;
    let key = body["api_key"]
        .as_str()
        .map(str::trim)
        .filter(|s| s.starts_with("ek_") && s.len() <= 256)
        .ok_or_else(|| ApiError::bad_request("Enter an existing Enclavo spending key."))?;
    let mut tx = state.db.begin().await?;
    let row = sqlx::query("SELECT id,owner_user_id FROM accounts WHERE api_key_hash=$1 FOR UPDATE")
        .bind(ledger::hash_key(key))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::unauthorized("Unknown account key."))?;
    let id: Uuid = row.try_get("id")?;
    if row.try_get::<Option<String>, _>("owner_user_id")?.is_some_and(|owner| owner != user) {
        return Err(ApiError::conflict("This account is already linked to another user."));
    }
    if account_id(&mut tx, &user, false).await?.is_some_and(|owned| owned != id) {
        return Err(ApiError::conflict("You already have an account. Contact the operator to transfer an allowance."));
    }
    // A revoked imported key must never become usable again through this endpoint.
    let revoked: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_keys WHERE key_hash=$1 AND revoked_at IS NOT NULL)")
            .bind(ledger::hash_key(key))
            .fetch_one(&mut *tx)
            .await?;
    if revoked {
        return Err(ApiError::unauthorized("This key has been revoked."));
    }
    sqlx::query("UPDATE accounts SET owner_user_id=$1 WHERE id=$2").bind(&user).bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO api_keys(id,account_id,name,prefix,key_hash) VALUES($1,$2,'Imported account key',$3,$4) ON CONFLICT(key_hash) DO NOTHING")
        .bind(Uuid::new_v4()).bind(id).bind(&key[..key.len().min(11)]).bind(ledger::hash_key(key)).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"connected":true,"account_id":id})))
}

async fn chat(State(state): State<Shared>, headers: HeaderMap, bytes: Bytes) -> Result<Response, ApiError> {
    let user = user(&state, &headers)?;
    let account = owned_account(&state, &user).await?;
    api::chat::completions_for_account(state, headers, bytes, account).await
}
