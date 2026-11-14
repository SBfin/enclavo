//! Settles on-chain auctions as soon as bidding ends, so winners get capacity without anyone
//! pressing Settle in a browser.
//!
//! `clear_auction` is permissionless, so the keeper signs with a NEAR function-call access key
//! that can only call that one method on the auction contract and cannot attach deposits. The
//! keeper refuses to sign with any key the chain reports as broader than that, so a leaked key
//! can at worst spend its own gas allowance. Without a key configured the gateway stays read-only.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::chain::Chain;

const METHOD: &str = "clear_auction";
/// The contract's own maximum per transaction; a full order book clears within it.
const GAS: u64 = 300_000_000_000_000;
/// Wait past the close so the chain's block time has certainly passed it too.
const GRACE_MS: u64 = 5_000;
const PAGE: u64 = 100;
/// `send_tx` waits for finality, which takes a few seconds on NEAR.
const SEND_TIMEOUT: Duration = Duration::from_secs(60);
/// Arbitrary constant naming the Postgres advisory lock that keeps one gateway machine sending.
const LOCK_KEY: i64 = 0x656e_636c_6176_6f4b;

pub struct Keeper {
    account: String,
    key: SigningKey,
    /// Every auction below this id is known to be cleared.
    first_open: AtomicU64,
}

#[derive(Deserialize)]
struct Auction {
    id: u64,
    ends_at_ms: u64,
    status: String,
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Cleared,
    /// Someone else settled it first, for example from the market page.
    AlreadyCleared,
    /// The chain's clock has not reached the close yet; retried on the next pass.
    NotYetClosed,
}

impl Keeper {
    /// `secret` is a NEAR secret key as wallets and `near` CLI print it: `ed25519:` and the base58
    /// of the 32-byte seed followed by the 32-byte public key.
    pub fn new(account: &str, secret: &str) -> anyhow::Result<Self> {
        let account = account.trim();
        ensure!(!account.is_empty(), "keeper account id is empty");
        let encoded = secret.trim().strip_prefix("ed25519:").context("keeper key must start with `ed25519:`")?;
        let bytes = bs58::decode(encoded).into_vec().context("keeper key is not base58")?;
        let seed: [u8; 32] = match bytes.len() {
            32 | 64 => bytes[..32].try_into().expect("length checked"),
            n => bail!("keeper key decodes to {n} bytes; expected 64 (seed and public key)"),
        };
        let key = SigningKey::from_bytes(&seed);
        if bytes.len() == 64 && key.verifying_key().to_bytes()[..] != bytes[32..] {
            bail!("keeper key is inconsistent: its public half does not match its seed");
        }
        Ok(Self { account: account.to_string(), key, first_open: AtomicU64::new(0) })
    }

    /// Enabled by `ENCLAVO_NEAR_KEEPER_ACCOUNT` and `ENCLAVO_NEAR_KEEPER_KEY`; both or neither.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let var = |name| std::env::var(name).ok().filter(|s: &String| !s.trim().is_empty());
        match (var("ENCLAVO_NEAR_KEEPER_ACCOUNT"), var("ENCLAVO_NEAR_KEEPER_KEY")) {
            (Some(account), Some(key)) => Self::new(&account, &key).map(Some),
            (None, None) => Ok(None),
            _ => bail!("set both ENCLAVO_NEAR_KEEPER_ACCOUNT and ENCLAVO_NEAR_KEEPER_KEY, or neither"),
        }
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn public_key(&self) -> String {
        format!("ed25519:{}", bs58::encode(self.key.verifying_key().to_bytes()).into_string())
    }

    /// Ids of auctions still open after their close, oldest first.
    async fn due(&self, chain: &Chain, now_ms: u64) -> anyhow::Result<Vec<u64>> {
        let floor = self.first_open.load(Ordering::Relaxed);
        let (mut due, mut newest, mut lowest_open) = (Vec::new(), None, None);
        let mut from = 0;
        // `list_auctions` is newest first, so stop at the first auction already known to be cleared.
        'pages: loop {
            let page: Vec<Auction> = chain.view("list_auctions", json!({"from_index": from, "limit": PAGE})).await?;
            for auction in &page {
                if auction.id < floor {
                    break 'pages;
                }
                newest.get_or_insert(auction.id);
                if auction.status == "open" {
                    lowest_open = Some(auction.id);
                    if now_ms >= auction.ends_at_ms.saturating_add(GRACE_MS) {
                        due.push(auction.id);
                    }
                }
            }
            if (page.len() as u64) < PAGE {
                break;
            }
            from += PAGE;
        }
        // Every auction from the floor up was seen: all below the lowest open one are cleared.
        let next = lowest_open.or(newest.map(|id| id + 1)).unwrap_or(floor);
        self.first_open.fetch_max(next, Ordering::Relaxed);
        due.reverse();
        Ok(due)
    }

    /// Nonce and a recent block hash for the keeper key, after checking the key is limited to
    /// `clear_auction` on this contract.
    async fn access_key(&self, chain: &Chain) -> anyhow::Result<(u64, [u8; 32])> {
        let result = chain
            .rpc(
                "query",
                json!({"request_type": "view_access_key", "finality": "final",
                       "account_id": self.account, "public_key": self.public_key()}),
                None,
            )
            .await?;
        if let Some(error) = result.get("error") {
            bail!("keeper key {} is not an access key of {}: {error}", self.public_key(), self.account);
        }
        check_permission(&result["permission"], chain.contract())?;
        let nonce = result["nonce"].as_u64().context("access key has no nonce")?;
        let hash = bs58::decode(result["block_hash"].as_str().context("access key view has no block hash")?)
            .into_vec()
            .context("block hash is not base58")?;
        Ok((nonce, hash.try_into().map_err(|_| anyhow::anyhow!("block hash is not 32 bytes"))?))
    }

    async fn clear(&self, chain: &Chain, auction_id: u64, nonce: u64, block_hash: [u8; 32]) -> anyhow::Result<Outcome> {
        let signed = self.sign_clear(chain.contract(), auction_id, nonce, block_hash);
        let result = chain
            .rpc(
                "send_tx",
                json!({"signed_tx_base64": STANDARD.encode(signed), "wait_until": "FINAL"}),
                Some(SEND_TIMEOUT),
            )
            .await?;
        outcome(&result)
    }

    fn sign_clear(&self, contract: &str, auction_id: u64, nonce: u64, block_hash: [u8; 32]) -> Vec<u8> {
        let args = json!({ "auction_id": auction_id }).to_string();
        let tx = encode_function_call(
            &self.account,
            &self.key.verifying_key().to_bytes(),
            nonce,
            contract,
            &block_hash,
            METHOD,
            args.as_bytes(),
            GAS,
        );
        let hash: [u8; 32] = Sha256::digest(&tx).into();
        let mut signed = tx;
        signed.push(0); // ED25519
        signed.extend_from_slice(&self.key.sign(&hash).to_bytes());
        signed
    }
}

/// The key must be a function-call key for this contract whose only method is `clear_auction`.
fn check_permission(permission: &Value, contract: &str) -> anyhow::Result<()> {
    let Some(call) = permission.get("FunctionCall") else {
        bail!("refusing to sign with the keeper key: it is a {permission} key, not a function-call key");
    };
    ensure!(
        call["receiver_id"] == contract,
        "refusing to sign with the keeper key: it is scoped to {}, not {contract}",
        call["receiver_id"]
    );
    ensure!(
        call["method_names"] == json!([METHOD]),
        "refusing to sign with the keeper key: its methods are {}, not only {METHOD}",
        call["method_names"]
    );
    Ok(())
}

/// Reads a `send_tx` result. Contract panics come back as a successful RPC call with a failure status.
fn outcome(result: &Value) -> anyhow::Result<Outcome> {
    let status = &result["status"];
    if status.get("SuccessValue").is_some() {
        return Ok(Outcome::Cleared);
    }
    let Some(failure) = status.get("Failure") else {
        bail!("clear_auction returned no final status: {result}");
    };
    let text = failure.to_string();
    if text.contains("Auction is already cleared") {
        Ok(Outcome::AlreadyCleared)
    } else if text.contains("Auction is still accepting bids") {
        Ok(Outcome::NotYetClosed)
    } else {
        bail!("clear_auction failed: {failure}")
    }
}

/// Borsh encoding of a NEAR `Transaction::V0` with one `FunctionCall` action and no deposit.
#[allow(clippy::too_many_arguments)]
fn encode_function_call(
    signer: &str,
    public_key: &[u8; 32],
    nonce: u64,
    receiver: &str,
    block_hash: &[u8; 32],
    method: &str,
    args: &[u8],
    gas: u64,
) -> Vec<u8> {
    fn bytes(out: &mut Vec<u8>, value: &[u8]) {
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value);
    }
    let mut out = Vec::with_capacity(160 + args.len());
    bytes(&mut out, signer.as_bytes());
    out.push(0); // ED25519
    out.extend_from_slice(public_key);
    out.extend_from_slice(&nonce.to_le_bytes());
    bytes(&mut out, receiver.as_bytes());
    out.extend_from_slice(block_hash);
    out.extend_from_slice(&1u32.to_le_bytes()); // one action
    out.push(2); // Action::FunctionCall
    bytes(&mut out, method.as_bytes());
    bytes(&mut out, args);
    out.extend_from_slice(&gas.to_le_bytes());
    out.extend_from_slice(&0u128.to_le_bytes()); // deposit
    out
}

/// One pass: settle every auction past its close. Returns how many this pass settled.
pub async fn settle_due(db: &PgPool, chain: &Chain, keeper: &Keeper) -> anyhow::Result<u64> {
    let due = keeper.due(chain, now_ms()).await?;
    if due.is_empty() {
        return Ok(0);
    }
    // Several gateway machines may run this loop; only the lock holder sends, so they never
    // compete for the key's nonce. A transaction-scoped lock is released even if this task fails.
    let mut lock = db.begin().await?;
    let held: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(LOCK_KEY).fetch_one(&mut *lock).await?;
    if !held {
        return Ok(0);
    }
    let (mut nonce, block_hash) = keeper.access_key(chain).await?;
    let mut settled = 0;
    for auction_id in due {
        nonce += 1;
        match keeper.clear(chain, auction_id, nonce, block_hash).await {
            Ok(Outcome::Cleared) => {
                settled += 1;
                tracing::info!(auction_id, "settled on-chain auction");
            }
            Ok(Outcome::AlreadyCleared) => tracing::debug!(auction_id, "auction was already settled"),
            Ok(Outcome::NotYetClosed) => tracing::debug!(auction_id, "auction has not closed on chain yet"),
            // The nonce may or may not have been used; read it again on the next pass.
            Err(error) => {
                tracing::error!(auction_id, %error, "could not settle on-chain auction");
                break;
            }
        }
    }
    lock.commit().await?;
    Ok(settled)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keeper() -> Keeper {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let secret = [key.to_bytes().as_slice(), key.verifying_key().as_bytes()].concat();
        Keeper::new("enclavo-keeper.testnet", &format!("ed25519:{}", bs58::encode(secret).into_string())).unwrap()
    }

    /// Produced with the official `near-api-types` 0.8.7 `SignedTransaction` for the same inputs.
    const REFERENCE: &str = "FgAAAGVuY2xhdm8ta2VlcGVyLnRlc3RuZXQA6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iwqAAAAAAAAABcAAABlbmNsYXZvLWF1Y3Rpb24udGVzdG5ldAkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJAQAAAAINAAAAY2xlYXJfYXVjdGlvbhAAAAB7ImF1Y3Rpb25faWQiOjF9AMBuMdkQAQAAAAAAAAAAAAAAAAAAAAAAAB5k0bVx0W97JxTdpgRdazOU9S81V+SN/jr0CHvxM9r9TR9HAyfsui8W03pNos1a18OSF1g99pIJvXu0Hclypw8=";

    #[test]
    fn signed_transaction_matches_the_reference_encoding() {
        let k = keeper();
        assert_eq!(k.public_key(), "ed25519:GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB");
        let signed = k.sign_clear("enclavo-auction.testnet", 1, 42, [9u8; 32]);
        assert_eq!(STANDARD.encode(signed), REFERENCE);
    }

    #[test]
    fn parses_cli_keys_and_rejects_bad_ones() {
        assert!(Keeper::new("a.testnet", &format!("ed25519:{}", bs58::encode([3u8; 32]).into_string())).is_ok());
        assert!(Keeper::new("a.testnet", "secp256k1:abc").is_err());
        assert!(Keeper::new("a.testnet", "ed25519:0OIl").is_err());
        assert!(Keeper::new("a.testnet", &format!("ed25519:{}", bs58::encode([1u8; 10]).into_string())).is_err());
        let mismatched = [[1u8; 32], [2u8; 32]].concat();
        assert!(Keeper::new("a.testnet", &format!("ed25519:{}", bs58::encode(mismatched).into_string())).is_err());
        assert!(Keeper::new(" ", &format!("ed25519:{}", bs58::encode([3u8; 32]).into_string())).is_err());
    }

    #[test]
    fn only_a_clear_auction_key_for_this_contract_is_accepted() {
        let contract = "enclavo-auction.testnet";
        let scoped = |receiver: &str, methods: Value| json!({"FunctionCall": {"allowance": "1000", "receiver_id": receiver, "method_names": methods}});
        assert!(check_permission(&scoped(contract, json!(["clear_auction"])), contract).is_ok());
        assert!(check_permission(&json!("FullAccess"), contract).is_err());
        assert!(check_permission(&scoped("other.testnet", json!(["clear_auction"])), contract).is_err());
        // An empty list allows every method.
        assert!(check_permission(&scoped(contract, json!([])), contract).is_err());
        assert!(check_permission(&scoped(contract, json!(["clear_auction", "withdraw"])), contract).is_err());
    }

    #[test]
    fn reads_transaction_outcomes() {
        let panic = |msg: &str| {
            json!({"status": {"Failure": {"ActionError": {"index": 0, "kind": {"FunctionCallError":
                {"ExecutionError": format!("Smart contract panicked: {msg}")}}}}}})
        };
        assert_eq!(outcome(&json!({"status": {"SuccessValue": ""}})).unwrap(), Outcome::Cleared);
        assert_eq!(outcome(&panic("Auction is already cleared")).unwrap(), Outcome::AlreadyCleared);
        assert_eq!(outcome(&panic("Auction is still accepting bids")).unwrap(), Outcome::NotYetClosed);
        assert!(outcome(&panic("Auction not found")).is_err());
        assert!(outcome(&json!({"final_execution_status": "NONE"})).is_err());
    }
}
