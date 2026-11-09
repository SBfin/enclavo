mod common;
use axum::http::Method;
use common::{TestApp, chat, spawn};
use serde_json::{Value, json};
use uuid::Uuid;

fn customer(app: &TestApp, user: &str, method: Method, path: &str) -> reqwest::RequestBuilder {
    app.http
        .request(method, format!("{}{path}", app.url))
        .bearer_auth("test-control-token")
        .header("x-enclavo-user-id", user)
}
async fn fund(app: &TestApp, user: &str, amount: u64) -> String {
    let (_, key) = enclavo_gateway::ledger::grant_operator_credit(&app.db, amount).await.unwrap();
    let response = customer(app, user, Method::POST, "/internal/account/import")
        .json(&json!({"api_key":key}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    key
}
async fn auction(app: &TestApp, units: i64) -> Uuid {
    let route = app.state.catalog.routes.iter().find(|r| r.id == "m@good").unwrap();
    enclavo_gateway::market::publish(&app.db, route, units, 7, 200_000, 600).await.unwrap()
}
async fn bid(app: &TestApp, user: &str, auction: Uuid, units: i64, price: &str, client: Uuid) -> reqwest::Response {
    customer(app, user, Method::POST, "/internal/market/bids")
        .json(&json!({"auction_id":auction,"units":units,"max_price_usd":price,"client_order_id":client}))
        .send()
        .await
        .unwrap()
}
async fn close(app: &TestApp, id: Uuid) {
    sqlx::query("UPDATE market_auctions SET ends_at=now()-interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(&app.db)
        .await
        .unwrap();
    assert_eq!(enclavo_gateway::market::clear_due(&app.state).await.unwrap(), 1);
    assert_eq!(enclavo_gateway::market::clear_due(&app.state).await.unwrap(), 0);
}
async fn contracts(app: &TestApp, user: &str) -> Value {
    customer(app, user, Method::GET, "/internal/market/contracts").send().await.unwrap().json().await.unwrap()
}

#[tokio::test]
async fn auction_clearing_is_uniform_partially_fills_and_returns_all_unused_reserves() {
    let app = spawn().await;
    let alice = fund(&app, "alice", 100_000_000).await;
    let bob = fund(&app, "bob", 100_000_000).await;
    let charlie = fund(&app, "charlie", 100_000_000).await;
    let lot = auction(&app, 3).await;
    assert_eq!(bid(&app, "alice", lot, 2, "0.30", Uuid::new_v4()).await.status(), 200);
    assert_eq!(bid(&app, "bob", lot, 2, "0.25", Uuid::new_v4()).await.status(), 200);
    assert_eq!(bid(&app, "charlie", lot, 1, "0.10", Uuid::new_v4()).await.status(), 200);
    close(&app, lot).await;
    assert_eq!(app.account(&alice).await["balance_micro_usd"], 96_500_000);
    assert_eq!(app.account(&bob).await["balance_micro_usd"], 98_250_000);
    assert_eq!(app.account(&charlie).await["balance_micro_usd"], 100_000_000);
    for key in [&alice, &bob, &charlie] {
        assert_eq!(app.account(key).await["held_micro_usd"], 0);
    }
    let row: (i64, i64) = sqlx::query_as("SELECT clearing_price_micro,filled_units FROM market_auctions WHERE id=$1")
        .bind(lot)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(row, (250_000, 3));
    assert_eq!(contracts(&app, "alice").await["contracts"][0]["units"], 2);
    assert_eq!(contracts(&app, "bob").await["contracts"][0]["units"], 1);
    assert!(contracts(&app, "charlie").await["contracts"].as_array().unwrap().is_empty());
    assert_eq!(app.account(&alice).await["spent_micro_usd"], 3_500_000);
}

#[tokio::test]
async fn bids_are_idempotent_cancellable_private_and_cannot_overspend() {
    let app = spawn().await;
    let alice = fund(&app, "alice", 2_000_000).await;
    let _bob = fund(&app, "bob", 2_000_000).await;
    let lot = auction(&app, 2).await;
    let client = Uuid::new_v4();
    let result = bid(&app, "alice", lot, 1, "0.2", client).await;
    assert_eq!(result.status(), 200);
    let data: Value = result.json().await.unwrap();
    let id = data["id"].as_str().unwrap();
    assert_eq!(bid(&app, "alice", lot, 1, "0.2", client).await.status(), 200);
    assert_eq!(app.account(&alice).await["held_micro_usd"], 1_400_000);
    assert_eq!(bid(&app, "alice", lot, 2, "0.2", client).await.status(), 409);
    assert_eq!(bid(&app, "alice", lot, 1, "0.2", Uuid::new_v4()).await.status(), 402);
    assert_eq!(
        customer(&app, "bob", Method::DELETE, &format!("/internal/market/bids/{id}")).send().await.unwrap().status(),
        404
    );
    assert_eq!(
        customer(&app, "alice", Method::DELETE, &format!("/internal/market/bids/{id}")).send().await.unwrap().status(),
        200
    );
    assert_eq!(app.account(&alice).await["balance_micro_usd"], 2_000_000);
    assert_eq!(app.account(&alice).await["held_micro_usd"], 0);
    assert_eq!(app.get("/internal/market/bids").send().await.unwrap().status(), 401);
}

#[tokio::test]
async fn paid_capacity_is_consumed_through_dashboard_and_api_without_a_second_cash_charge() {
    let app = spawn().await;
    let key = fund(&app, "alice", 100_000_000).await;
    let _ = fund(&app, "bob", 100_000_000).await;
    let lot = auction(&app, 1).await;
    assert_eq!(bid(&app, "alice", lot, 1, "0.3", Uuid::new_v4()).await.status(), 200);
    close(&app, lot).await;
    let contract = contracts(&app, "alice").await["contracts"][0]["id"].as_str().unwrap().to_owned();
    let before = app.account(&key).await["balance_micro_usd"].clone();
    let mut body = chat("m");
    body["capacity_contract_id"] = contract.clone().into();
    let response =
        customer(&app, "alice", Method::POST, "/internal/chat/completions").json(&body).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-enclavo-cost-micro-usd"], "0");
    assert_eq!(app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap().status(), 200);
    assert_eq!(app.account(&key).await["balance_micro_usd"], before);
    let used = contracts(&app, "alice").await;
    assert_eq!(used["contracts"][0]["used_tokens"], 60.0);
    assert_eq!(used["contracts"][0]["held_tokens"], 0.0);
    body["stream"] = true.into();
    let streamed =
        app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap().text().await.unwrap();
    assert!(streamed.contains("data: [DONE]"));
    assert!(streamed.contains(": enclavo-cost-micro-usd 0"));
    assert_eq!(contracts(&app, "alice").await["contracts"][0]["used_tokens"], 90.0);
    assert_eq!(app.account(&key).await["balance_micro_usd"], before);
    assert_eq!(
        customer(&app, "bob", Method::POST, "/internal/chat/completions").json(&body).send().await.unwrap().status(),
        404
    );
    let public: Value = app.get("/v1/contracts").bearer_auth(&key).send().await.unwrap().json().await.unwrap();
    assert_eq!(public["contracts"][0]["id"], contract);
    assert!(app.upstream.requests.lock().iter().all(|(_, body)| body.get("capacity_contract_id").is_none()));
}

#[tokio::test]
async fn capacity_limits_expiry_failures_and_daily_reset_are_enforced() {
    let app = spawn().await;
    let key = fund(&app, "alice", 100_000_000).await;
    let lot = auction(&app, 1).await;
    assert_eq!(bid(&app, "alice", lot, 1, "0.2", Uuid::new_v4()).await.status(), 200);
    close(&app, lot).await;
    let id: Uuid = contracts(&app, "alice").await["contracts"][0]["id"].as_str().unwrap().parse().unwrap();
    let mut body = chat("m");
    body["capacity_contract_id"] = id.to_string().into();
    // Reserve nearly the whole 1,000-token daily entitlement, then attempt another request.
    let hold = enclavo_gateway::market::reserve_capacity(
        &app.state,
        enclavo_gateway::ledger::account_by_key(&app.db, &key).await.unwrap().unwrap().id,
        id,
        900,
        99,
    )
    .await
    .unwrap();
    let before = app.upstream.requests.lock().len();
    assert_eq!(app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap().status(), 402);
    assert_eq!(app.upstream.requests.lock().len(), before);
    // Crash recovery releases an expired reservation.
    sqlx::query("UPDATE capacity_holds SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(hold)
        .execute(&app.db)
        .await
        .unwrap();
    enclavo_gateway::market::release_expired_capacity(&app.state).await.unwrap();
    assert_eq!(app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap().status(), 200);
    // A different contract day has its own unused allowance.
    sqlx::query("UPDATE capacity_contracts SET starts_at=starts_at-interval '25 hours',ends_at=ends_at-interval '25 hours' WHERE id=$1").bind(id).execute(&app.db).await.unwrap();
    assert_eq!(contracts(&app, "alice").await["contracts"][0]["used_tokens"], 0.0);
    assert_eq!(app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap().status(), 200);
    sqlx::query("UPDATE capacity_contracts SET ends_at=now()-interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(&app.db)
        .await
        .unwrap();
    assert_eq!(app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap().status(), 404);
    assert_eq!(app.account(&key).await["balance_micro_usd"], 98_600_000);
}

#[tokio::test]
async fn concurrent_bids_and_capacity_reservations_cannot_reuse_the_same_budget() {
    let app = spawn().await;
    let key = fund(&app, "alice", 2_000_000).await;
    let first = auction(&app, 1).await;
    let second = auction(&app, 1).await;
    let (a, b) = tokio::join!(
        bid(&app, "alice", first, 1, "0.2", Uuid::new_v4()),
        bid(&app, "alice", second, 1, "0.2", Uuid::new_v4())
    );
    assert_eq!([a.status().as_u16(), b.status().as_u16()].into_iter().filter(|status| *status == 200).count(), 1);
    assert_eq!(app.account(&key).await["held_micro_usd"], 1_400_000);
    sqlx::query("UPDATE market_auctions SET ends_at=now()-interval '1 second'").execute(&app.db).await.unwrap();
    let (a, b) =
        tokio::join!(enclavo_gateway::market::clear_due(&app.state), enclavo_gateway::market::clear_due(&app.state));
    assert_eq!(a.unwrap() + b.unwrap(), 2);
    let id: Uuid = contracts(&app, "alice").await["contracts"][0]["id"].as_str().unwrap().parse().unwrap();
    let account = enclavo_gateway::ledger::account_by_key(&app.db, &key).await.unwrap().unwrap();
    let (a, b) = tokio::join!(
        enclavo_gateway::market::reserve_capacity(&app.state, account.id, id, 600, 0),
        enclavo_gateway::market::reserve_capacity(&app.state, account.id, id, 600, 0)
    );
    assert_eq!([a.is_ok(), b.is_ok()].into_iter().filter(|ok| *ok).count(), 1);
    assert_eq!(contracts(&app, "alice").await["contracts"][0]["held_tokens"], 600.0);
    let empty: Option<i64> =
        sqlx::query_scalar("SELECT clearing_price_micro FROM market_auctions WHERE filled_units=0")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(empty.is_none());
}

#[tokio::test]
async fn a_failed_capacity_provider_releases_its_quota_and_never_switches_to_another_contract_route() {
    let app = spawn().await;
    let key = fund(&app, "alice", 100_000_000).await;
    let route = app.state.catalog.routes.iter().find(|r| r.id == "m@broken").unwrap();
    let lot = enclavo_gateway::market::publish(&app.db, route, 1, 7, 100_000, 600).await.unwrap();
    assert_eq!(bid(&app, "alice", lot, 1, "0.1", Uuid::new_v4()).await.status(), 200);
    close(&app, lot).await;
    let id = contracts(&app, "alice").await["contracts"][0]["id"].as_str().unwrap().to_owned();
    let mut body = chat("m");
    body["capacity_contract_id"] = id.into();
    assert_eq!(app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap().status(), 502);
    let capacity = contracts(&app, "alice").await;
    assert_eq!(capacity["contracts"][0]["used_tokens"], 0.0);
    assert_eq!(capacity["contracts"][0]["held_tokens"], 0.0);
    assert!(app.upstream.requests.lock().iter().all(|(provider, _)| provider == "broken"));
    assert_eq!(app.account(&key).await["balance_micro_usd"], 99_300_000);
}
