mod common;
use std::{collections::HashMap, sync::Arc};

use axum::{Json, Router, extract::State, routing::post};
use base64::{Engine, engine::general_purpose::STANDARD};
use common::{TestApp, chat, spawn};
use enclavo_gateway::{chain, ledger};
use parking_lot::Mutex;
use serde_json::{Value, json};

/// The auction contract's view methods, as the NEAR RPC serves them.
#[derive(Default)]
struct MockContract {
    entitlements: Mutex<Vec<Value>>,
    keys: Mutex<HashMap<String, String>>,
}

async fn rpc(State(contract): State<Arc<MockContract>>, Json(request): Json<Value>) -> Json<Value> {
    let params = &request["params"];
    assert_eq!((params["finality"].as_str(), params["account_id"].as_str()), (Some("final"), Some("auction.test")));
    let args: Value =
        serde_json::from_slice(&STANDARD.decode(params["args_base64"].as_str().unwrap()).unwrap()).unwrap();
    let result = match params["method_name"].as_str().unwrap() {
        "list_entitlements" => {
            let from = args["from_id"].as_i64().unwrap();
            json!(contract.entitlements.lock().iter().filter(|e| e["id"].as_i64().unwrap() >= from).collect::<Vec<_>>())
        }
        "key_hash_of" => json!(contract.keys.lock().get(args["account_id"].as_str().unwrap())),
        other => panic!("unexpected view {other}"),
    };
    Json(json!({"jsonrpc":"2.0","id":"enclavo","result":{"result":result.to_string().into_bytes(),"logs":[]}}))
}

async fn mock_chain() -> (Arc<MockContract>, chain::Chain) {
    let contract = Arc::new(MockContract::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new().route("/", post(rpc)).with_state(contract.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (contract, chain::Chain::new(&format!("http://{addr}/"), "auction.test").unwrap())
}

fn entitlement(id: i64, owner: &str, units: i64) -> Value {
    let now = chrono::Utc::now().timestamp_millis();
    json!({"id":id,"auction_id":0,"bid_index":id,"owner":owner,"route_id":"m@good","units":units,
           "tokens_per_iu":1000,"prompt_weight_bps":5000,"starts_at_ms":now-1000,"ends_at_ms":now+7*86_400_000,
           "paid":"1400000"})
}

fn key_hash(key: &str) -> String {
    hex::encode(ledger::hash_key(key))
}

async fn contracts(app: &TestApp, key: &str) -> reqwest::Response {
    app.get("/v1/contracts").bearer_auth(key).send().await.unwrap()
}

#[tokio::test]
async fn on_chain_entitlement_is_spendable_with_a_wallet_authorized_key() {
    let app = spawn().await;
    let (contract, chain) = mock_chain().await;
    contract.entitlements.lock().push(entitlement(0, "alice.testnet", 1));
    let key = ledger::new_api_key();

    // The capacity is mirrored, but nobody can use it until the wallet authorizes a key.
    assert_eq!(chain::sync(&app.db, &chain).await.unwrap(), (1, 0));
    assert_eq!(contracts(&app, &key).await.status(), 401);
    contract.keys.lock().insert("alice.testnet".into(), key_hash(&key));
    assert_eq!(chain::sync(&app.db, &chain).await.unwrap(), (0, 1));
    assert_eq!(chain::sync(&app.db, &chain).await.unwrap(), (0, 0));

    let owned: Value = contracts(&app, &key).await.json().await.unwrap();
    let capacity = &owned["contracts"][0];
    assert_eq!(owned["contracts"].as_array().unwrap().len(), 1);
    assert_eq!(
        (capacity["chain_contract"].as_str(), capacity["chain_entitlement_id"].as_i64()),
        (Some("auction.test"), Some(0))
    );
    assert_eq!((capacity["auction_id"].is_null(), capacity["active"].as_bool()), (true, Some(true)));
    assert_eq!(
        (capacity["daily_tokens"].as_f64(), capacity["paid_micro_usd"].as_i64()),
        (Some(1000.0), Some(1_400_000))
    );

    // Inference draws on the entitlement; the account holds no credit and is charged none.
    let mut body = chat("m");
    body["capacity_contract_id"] = capacity["id"].clone();
    let response = app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-enclavo-cost-micro-usd"], "0");
    assert_eq!(app.account(&key).await["balance_micro_usd"], 0);
    let used: Value = contracts(&app, &key).await.json().await.unwrap();
    assert!(used["contracts"][0]["used_tokens"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn key_rotation_and_revocation_on_chain_take_effect() {
    let app = spawn().await;
    let (contract, chain) = mock_chain().await;
    contract.entitlements.lock().push(entitlement(0, "alice.testnet", 1));
    let (old, new) = (ledger::new_api_key(), ledger::new_api_key());
    contract.keys.lock().insert("alice.testnet".into(), key_hash(&old));
    chain::sync(&app.db, &chain).await.unwrap();
    assert_eq!(contracts(&app, &old).await.status(), 200);

    contract.keys.lock().insert("alice.testnet".into(), key_hash(&new));
    assert_eq!(chain::sync(&app.db, &chain).await.unwrap(), (0, 1));
    assert_eq!(contracts(&app, &old).await.status(), 401);
    assert_eq!(contracts(&app, &new).await.status(), 200);

    contract.keys.lock().clear();
    assert_eq!(chain::sync(&app.db, &chain).await.unwrap(), (0, 1));
    assert_eq!(contracts(&app, &new).await.status(), 401);
}

#[tokio::test]
async fn owners_are_isolated_and_bad_entries_do_not_block_the_mirror() {
    let app = spawn().await;
    let (contract, chain) = mock_chain().await;
    let mut broken = entitlement(1, "mallory.testnet", 1);
    broken["tokens_per_iu"] = 0.into();
    contract.entitlements.lock().extend([entitlement(0, "alice.testnet", 1), broken, entitlement(2, "bob.testnet", 2)]);
    let (alice, bob) = (ledger::new_api_key(), ledger::new_api_key());
    contract
        .keys
        .lock()
        .extend([("alice.testnet".to_string(), key_hash(&alice)), ("bob.testnet".to_string(), key_hash(&bob))]);
    assert_eq!(chain::sync(&app.db, &chain).await.unwrap(), (2, 2));

    // A later entitlement for a known owner lands on the same account.
    contract.entitlements.lock().push(entitlement(3, "bob.testnet", 5));
    assert_eq!(chain::sync(&app.db, &chain).await.unwrap(), (1, 0));
    let alices: Value = contracts(&app, &alice).await.json().await.unwrap();
    let bobs: Value = contracts(&app, &bob).await.json().await.unwrap();
    assert_eq!((alices["contracts"].as_array().unwrap().len(), bobs["contracts"].as_array().unwrap().len()), (1, 2));

    // Bob cannot spend Alice's capacity.
    let mut body = chat("m");
    body["capacity_contract_id"] = alices["contracts"][0]["id"].clone();
    assert_eq!(app.post("/v1/chat/completions").bearer_auth(&bob).json(&body).send().await.unwrap().status(), 404);
}
