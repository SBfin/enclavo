//! Mirrors the NEAR auction contract into the gateway: entitlements won on chain become
//! capacity contracts, and key hashes a wallet authorized there become API keys for its account.
//!
//! The contract is the source of truth for who owns capacity. The gateway only reads finalized
//! state through view calls, so it needs no chain key and cannot move funds.
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    Shared,
    keeper::{self, Keeper},
    ledger,
};

const DEFAULT_RPC_URL: &str = "https://test.rpc.fastnear.com";
const PAGE: u64 = 100;
/// Name of the API key row a wallet authorization creates. One per account.
const WALLET_KEY_NAME: &str = "NEAR wallet";

pub struct Chain {
    rpc_url: String,
    contract: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct Entitlement {
    id: i64,
    owner: String,
    route_id: String,
    units: i64,
    tokens_per_iu: i64,
    prompt_weight_bps: i32,
    starts_at_ms: i64,
    ends_at_ms: i64,
    /// Token atomic units. With 6-decimal USDC this is micro-USD.
    paid: String,
}

impl Chain {
    pub fn new(rpc_url: &str, contract: &str) -> anyhow::Result<Self> {
        Ok(Self {
            rpc_url: rpc_url.to_string(),
            contract: contract.to_string(),
            http: reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?,
        })
    }

    /// Enabled by `ENCLAVO_AUCTION_CONTRACT`; `ENCLAVO_NEAR_RPC_URL` overrides the testnet RPC.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(contract) = std::env::var("ENCLAVO_AUCTION_CONTRACT").ok().filter(|s| !s.trim().is_empty()) else {
            return Ok(None);
        };
        let rpc = std::env::var("ENCLAVO_NEAR_RPC_URL").ok().filter(|s| !s.trim().is_empty());
        Self::new(rpc.as_deref().unwrap_or(DEFAULT_RPC_URL), contract.trim()).map(Some)
    }

    /// What a browser needs to read the contract and send it transactions; none of it is secret.
    pub fn public_info() -> Option<Value> {
        let contract = std::env::var("ENCLAVO_AUCTION_CONTRACT").ok().filter(|s| !s.trim().is_empty())?;
        let rpc = std::env::var("ENCLAVO_NEAR_RPC_URL").ok().filter(|s| !s.trim().is_empty());
        let network = if contract.trim().ends_with(".testnet") { "testnet" } else { "mainnet" };
        Some(json!({"network": network, "auction_contract": contract.trim(),
                    "rpc_url": rpc.as_deref().unwrap_or(DEFAULT_RPC_URL)}))
    }

    pub fn contract(&self) -> &str {
        &self.contract
    }

    /// One JSON-RPC call; returns its `result`. `timeout` overrides the client's 15 seconds.
    pub(crate) async fn rpc(&self, method: &str, params: Value, timeout: Option<Duration>) -> anyhow::Result<Value> {
        let request = json!({"jsonrpc":"2.0","id":"enclavo","method":method,"params":params});
        let mut call = self.http.post(&self.rpc_url).json(&request);
        if let Some(timeout) = timeout {
            call = call.timeout(timeout);
        }
        let mut response: Value = call.send().await?.error_for_status()?.json().await?;
        if let Some(error) = response.get("error") {
            bail!("NEAR RPC {method} failed: {error}");
        }
        Ok(response["result"].take())
    }

    /// Calls a view method against the latest finalized block.
    pub(crate) async fn view<T: DeserializeOwned>(&self, method: &str, args: Value) -> anyhow::Result<T> {
        let params = json!({"request_type":"call_function","finality":"final","account_id":self.contract,
            "method_name":method,"args_base64":STANDARD.encode(args.to_string())});
        let result = self.rpc("query", params, None).await?;
        if let Some(error) = result.get("error") {
            bail!("NEAR view {method} failed: {error}");
        }
        let bytes: Vec<u8> = serde_json::from_value(result["result"].clone())
            .with_context(|| format!("NEAR view {method} returned no result"))?;
        serde_json::from_slice(&bytes).with_context(|| format!("NEAR view {method} returned unexpected JSON"))
    }
}

/// One pass: import new entitlements, then reconcile wallet-authorized keys. Returns how many of each changed.
pub async fn sync(db: &PgPool, chain: &Chain) -> anyhow::Result<(u64, u64)> {
    Ok((sync_entitlements(db, chain).await?, sync_keys(db, chain).await?))
}

async fn sync_entitlements(db: &PgPool, chain: &Chain) -> anyhow::Result<u64> {
    let mut imported = 0;
    loop {
        let next: i64 = sqlx::query_scalar("SELECT next_entitlement_id FROM chain_sync WHERE contract=$1")
            .bind(&chain.contract)
            .fetch_optional(db)
            .await?
            .unwrap_or(0);
        let page: Vec<Entitlement> = chain.view("list_entitlements", json!({"from_id": next, "limit": PAGE})).await?;
        let Some(last) = page.last().map(|e| e.id) else { return Ok(imported) };
        let mut tx = db.begin().await?;
        for entitlement in &page {
            match import(&mut tx, &chain.contract, entitlement).await {
                Ok(true) => imported += 1,
                Ok(false) => {}
                // A malformed entry must not block every later one.
                Err(error) => tracing::error!(%error, id = entitlement.id, "skipping on-chain entitlement"),
            }
        }
        sqlx::query(
            "INSERT INTO chain_sync(contract,next_entitlement_id) VALUES($1,$2)
             ON CONFLICT(contract) DO UPDATE SET next_entitlement_id=EXCLUDED.next_entitlement_id",
        )
        .bind(&chain.contract)
        .bind(last + 1)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        if (page.len() as u64) < PAGE {
            return Ok(imported);
        }
    }
}

async fn import(tx: &mut sqlx::PgConnection, contract: &str, e: &Entitlement) -> anyhow::Result<bool> {
    ensure!((1..=1_000_000).contains(&e.units) && e.tokens_per_iu > 0 && e.prompt_weight_bps >= 0, "invalid size");
    ensure!(e.units as i128 * e.tokens_per_iu as i128 * 10_000 <= i64::MAX as i128, "daily capacity is too large");
    ensure!(e.starts_at_ms >= 0 && e.ends_at_ms > e.starts_at_ms, "invalid term");
    let paid: i64 = e.paid.parse().context("paid amount is out of range")?;
    ensure!(paid >= 0, "paid amount is negative");
    // The account's own key is random and discarded; access comes only from a wallet-authorized key.
    sqlx::query("INSERT INTO accounts(id,api_key_hash,near_account_id) VALUES($1,$2,$3) ON CONFLICT(near_account_id) DO NOTHING")
        .bind(Uuid::new_v4())
        .bind(ledger::hash_key(&ledger::new_api_key()))
        .bind(&e.owner)
        .execute(&mut *tx)
        .await?;
    let account: Uuid = sqlx::query_scalar("SELECT id FROM accounts WHERE near_account_id=$1")
        .bind(&e.owner)
        .fetch_one(&mut *tx)
        .await?;
    let inserted = sqlx::query(
        "INSERT INTO capacity_contracts(id,account_id,route_id,units,tokens_per_iu,prompt_weight_bps,starts_at,ends_at,paid_micro,chain_contract,chain_entitlement_id)
         VALUES($1,$2,$3,$4,$5,$6,to_timestamp($7::float8/1000),to_timestamp($8::float8/1000),$9,$10,$11)
         ON CONFLICT(chain_contract,chain_entitlement_id) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(account)
    .bind(&e.route_id)
    .bind(e.units)
    .bind(e.tokens_per_iu)
    .bind(e.prompt_weight_bps)
    .bind(e.starts_at_ms as f64)
    .bind(e.ends_at_ms as f64)
    .bind(paid)
    .bind(contract)
    .bind(e.id)
    .execute(&mut *tx)
    .await?;
    Ok(inserted.rows_affected() == 1)
}

/// Makes each chain-owned account's active wallet key match the hash its owner authorized on chain.
async fn sync_keys(db: &PgPool, chain: &Chain) -> anyhow::Result<u64> {
    let accounts: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id,near_account_id FROM accounts WHERE near_account_id IS NOT NULL ORDER BY created_at")
            .fetch_all(db)
            .await?;
    let mut changed = 0;
    for (account, near_account) in accounts {
        let authorized: Option<String> = chain.view("key_hash_of", json!({"account_id": near_account})).await?;
        let hash = authorized.as_deref().map(hex::decode).transpose().ok().flatten().filter(|h| h.len() == 32);
        let current: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT key_hash FROM api_keys WHERE account_id=$1 AND name=$2 AND revoked_at IS NULL")
                .bind(account)
                .bind(WALLET_KEY_NAME)
                .fetch_optional(db)
                .await?;
        if hash == current {
            continue;
        }
        let mut tx = db.begin().await?;
        let revoked =
            sqlx::query("UPDATE api_keys SET revoked_at=now() WHERE account_id=$1 AND name=$2 AND revoked_at IS NULL")
                .bind(account)
                .bind(WALLET_KEY_NAME)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        let mut added = 0;
        if let Some(hash) = hash {
            // A hash already known to the gateway is never reassigned, whoever authorizes it later.
            added = sqlx::query(
                "INSERT INTO api_keys(id,account_id,name,prefix,key_hash) VALUES($1,$2,$3,'near',$4)
                 ON CONFLICT(key_hash) DO NOTHING",
            )
            .bind(Uuid::new_v4())
            .bind(account)
            .bind(WALLET_KEY_NAME)
            .bind(hash)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        changed += (revoked + added).min(1);
    }
    Ok(changed)
}

/// Polls the contract until shutdown. With a keeper, first settles auctions past their close so
/// the same pass imports the entitlements they create.
pub async fn run(state: Shared, chain: Chain, keeper: Option<Keeper>) {
    tracing::info!(contract = chain.contract(), rpc = chain.rpc_url, "mirroring on-chain auction entitlements");
    match &keeper {
        Some(k) => tracing::info!(account = k.account(), key = k.public_key(), "settling auctions when they close"),
        None => tracing::info!("no keeper key configured; auctions settle only when someone calls clear_auction"),
    }
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = state.shutdown.cancelled() => return,
        }
        if let Some(keeper) = &keeper
            && let Err(error) = keeper::settle_due(&state.db, &chain, keeper).await
        {
            tracing::error!(%error, "auction settlement pass failed");
        }
        match sync(&state.db, &chain).await {
            Ok((0, 0)) => {}
            Ok((entitlements, keys)) => tracing::info!(entitlements, keys, "on-chain state mirrored"),
            Err(error) => tracing::error!(%error, "on-chain sync failed"),
        }
    }
}
