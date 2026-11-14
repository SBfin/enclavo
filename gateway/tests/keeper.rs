mod common;
use std::{sync::Arc, time::Duration};

use axum::{Json, Router, extract::State, routing::post};
use base64::{Engine, engine::general_purpose::STANDARD};
use common::spawn;
use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use enclavo_gateway::{chain::Chain, keeper};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CONTRACT: &str = "auction.test";
const KEEPER: &str = "keeper.test";

/// A NEAR RPC serving the auction contract's `list_auctions`, the keeper's access key, and
/// `clear_auction` transactions, which it checks the way the chain would.
struct MockNear {
    public_key: VerifyingKey,
    permission: Mutex<Value>,
    /// (id, ends_at_ms, status)
    auctions: Mutex<Vec<(u64, u64, &'static str)>>,
    nonce: Mutex<u64>,
    /// (auction id, nonce) of every accepted transaction, in order.
    sent: Mutex<Vec<(u64, u64)>>,
    list_calls: Mutex<u32>,
    send_delay: Duration,
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> &'a [u8] {
    let (head, tail) = bytes.split_at(n);
    *bytes = tail;
    head
}
fn string(bytes: &mut &[u8]) -> Vec<u8> {
    let len = u32::from_le_bytes(take(bytes, 4).try_into().unwrap()) as usize;
    take(bytes, len).to_vec()
}

impl MockNear {
    fn send_tx(&self, signed: &[u8]) -> Value {
        // Signature over sha256 of the transaction, by the keeper's key.
        let (tx, sig) = signed.split_at(signed.len() - 65);
        assert_eq!(sig[0], 0);
        let hash: [u8; 32] = Sha256::digest(tx).into();
        self.public_key.verify(&hash, &Signature::from_bytes(sig[1..].try_into().unwrap())).expect("valid signature");

        let mut b = tx;
        assert_eq!(string(&mut b), KEEPER.as_bytes());
        assert_eq!(take(&mut b, 33), [&[0u8][..], self.public_key.as_bytes()].concat());
        let nonce = u64::from_le_bytes(take(&mut b, 8).try_into().unwrap());
        assert_eq!(string(&mut b), CONTRACT.as_bytes());
        take(&mut b, 32);
        assert_eq!(take(&mut b, 5), [1, 0, 0, 0, 2]);
        assert_eq!(string(&mut b), b"clear_auction");
        let args: Value = serde_json::from_slice(&string(&mut b)).unwrap();
        take(&mut b, 8);
        assert_eq!(take(&mut b, 16), [0u8; 16], "no deposit");
        assert!(b.is_empty());

        let mut current = self.nonce.lock();
        assert!(nonce > *current, "nonce {nonce} was already used");
        *current = nonce;
        let id = args["auction_id"].as_u64().unwrap();
        let mut auctions = self.auctions.lock();
        let auction = auctions.iter_mut().find(|a| a.0 == id).unwrap();
        let panic = |msg: &str| {
            json!({"status": {"Failure": {"ActionError": {"index": 0, "kind": {"FunctionCallError":
                {"ExecutionError": format!("Smart contract panicked: {msg}")}}}}}})
        };
        if auction.2 != "open" {
            return panic("Auction is already cleared");
        }
        if now_ms() < auction.1 {
            return panic("Auction is still accepting bids");
        }
        auction.2 = "cleared";
        self.sent.lock().push((id, nonce));
        json!({"status": {"SuccessValue": ""}})
    }
}

async fn rpc(State(near): State<Arc<MockNear>>, Json(request): Json<Value>) -> Json<Value> {
    let params = &request["params"];
    let result = match (request["method"].as_str().unwrap(), params["request_type"].as_str()) {
        ("query", Some("call_function")) => {
            assert_eq!(params["method_name"], "list_auctions");
            *near.list_calls.lock() += 1;
            let args: Value =
                serde_json::from_slice(&STANDARD.decode(params["args_base64"].as_str().unwrap()).unwrap()).unwrap();
            let (from, limit) =
                (args["from_index"].as_u64().unwrap() as usize, args["limit"].as_u64().unwrap() as usize);
            let page: Vec<Value> = near
                .auctions
                .lock()
                .iter()
                .rev()
                .skip(from)
                .take(limit)
                .map(|(id, ends, status)| json!({"id": id, "ends_at_ms": ends, "status": status}))
                .collect();
            json!({"result": json!(page).to_string().into_bytes(), "logs": []})
        }
        ("query", Some("view_access_key")) => {
            assert_eq!(params["account_id"], KEEPER);
            let key = format!("ed25519:{}", bs58::encode(near.public_key.as_bytes()).into_string());
            assert_eq!(params["public_key"], key);
            json!({"nonce": *near.nonce.lock(), "permission": near.permission.lock().clone(),
                   "block_hash": bs58::encode([5u8; 32]).into_string(), "block_height": 1})
        }
        ("send_tx", _) => {
            tokio::time::sleep(near.send_delay).await;
            assert_eq!(params["wait_until"], "FINAL");
            near.send_tx(&STANDARD.decode(params["signed_tx_base64"].as_str().unwrap()).unwrap())
        }
        other => panic!("unexpected RPC {other:?}"),
    };
    Json(json!({"jsonrpc": "2.0", "id": "enclavo", "result": result}))
}

fn scoped(methods: Value) -> Value {
    json!({"FunctionCall": {"allowance": "1000000000000000000000000", "receiver_id": CONTRACT, "method_names": methods}})
}

async fn mock_near(auctions: Vec<(u64, u64, &'static str)>, send_delay: Duration) -> (Arc<MockNear>, Chain, String) {
    let key = SigningKey::from_bytes(&[11u8; 32]);
    let near = Arc::new(MockNear {
        public_key: key.verifying_key(),
        permission: Mutex::new(scoped(json!(["clear_auction"]))),
        auctions: Mutex::new(auctions),
        nonce: Mutex::new(700),
        sent: Mutex::default(),
        list_calls: Mutex::default(),
        send_delay,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new().route("/", post(rpc)).with_state(near.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let secret = [key.to_bytes().as_slice(), key.verifying_key().as_bytes()].concat();
    let secret = format!("ed25519:{}", bs58::encode(secret).into_string());
    (near, Chain::new(&format!("http://{addr}/"), CONTRACT).unwrap(), secret)
}

#[tokio::test]
async fn settles_closed_auctions_oldest_first_and_only_once() {
    let app = spawn().await;
    let past = now_ms() - 60_000;
    let future = now_ms() + 3_600_000;
    // 0 is settled already, 3 is still taking bids.
    let (near, chain, secret) = mock_near(
        vec![(0, past, "cleared"), (1, past, "open"), (2, past, "open"), (3, future, "open")],
        Duration::ZERO,
    )
    .await;
    let keeper = keeper::Keeper::new(KEEPER, &secret).unwrap();

    assert_eq!(keeper::settle_due(&app.db, &chain, &keeper).await.unwrap(), 2);
    assert_eq!(*near.sent.lock(), vec![(1, 701), (2, 702)]);

    // Nothing is due now; the scan stops at auctions known to be settled.
    assert_eq!(keeper::settle_due(&app.db, &chain, &keeper).await.unwrap(), 0);
    assert_eq!(near.sent.lock().len(), 2);

    // Settled by someone else between the scan and the send: not an error.
    near.auctions.lock()[3].1 = past;
    let other = keeper::Keeper::new(KEEPER, &secret).unwrap();
    near.auctions.lock()[3].2 = "cleared";
    near.auctions.lock().push((4, past, "open"));
    assert_eq!(keeper::settle_due(&app.db, &chain, &other).await.unwrap(), 1);
    assert_eq!(near.sent.lock().last(), Some(&(4, 703)));
}

#[tokio::test]
async fn refuses_keys_broader_than_clear_auction() {
    let app = spawn().await;
    let (near, chain, secret) = mock_near(vec![(0, now_ms() - 60_000, "open")], Duration::ZERO).await;
    let keeper = keeper::Keeper::new(KEEPER, &secret).unwrap();
    for permission in [json!("FullAccess"), scoped(json!([])), scoped(json!(["clear_auction", "withdraw"]))] {
        *near.permission.lock() = permission;
        let error = keeper::settle_due(&app.db, &chain, &keeper).await.unwrap_err();
        assert!(error.to_string().contains("refusing to sign"), "{error}");
    }
    assert!(near.sent.lock().is_empty());
}

#[tokio::test]
async fn concurrent_gateways_send_each_settlement_once() {
    let app = spawn().await;
    let past = now_ms() - 60_000;
    let (near, chain, secret) = mock_near(vec![(0, past, "open"), (1, past, "open")], Duration::from_millis(300)).await;
    let (a, b) = (keeper::Keeper::new(KEEPER, &secret).unwrap(), keeper::Keeper::new(KEEPER, &secret).unwrap());
    let (ra, rb) = tokio::join!(keeper::settle_due(&app.db, &chain, &a), keeper::settle_due(&app.db, &chain, &b));
    assert_eq!(ra.unwrap() + rb.unwrap(), 2);
    assert_eq!(*near.sent.lock(), vec![(0, 701), (1, 702)]);
}
