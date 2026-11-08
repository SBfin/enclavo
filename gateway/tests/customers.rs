mod common;
use axum::http::Method;
use common::{TestApp, chat, spawn};
use serde_json::{Value, json};

fn customer(app: &TestApp, user: &str, method: Method, path: &str) -> reqwest::RequestBuilder {
    app.http
        .request(method, format!("{}{path}", app.url))
        .bearer_auth("test-control-token")
        .header("x-enclavo-user-id", user)
}

#[tokio::test]
async fn customer_keys_are_private_shared_and_revocable() {
    let app = spawn().await;
    assert_eq!(app.get("/internal/keys").header("x-enclavo-user-id", "alice").send().await.unwrap().status(), 401);
    assert_eq!(
        app.get("/internal/keys")
            .bearer_auth("wrong-control-token")
            .header("x-enclavo-user-id", "alice")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = customer(&app, "alice", Method::POST, "/internal/keys")
        .json(&json!({"name":"Application"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let created: Value = response.json().await.unwrap();
    let key = created["api_key"].as_str().unwrap();
    assert!(key.starts_with("ek_"));
    let keys: Value =
        customer(&app, "alice", Method::GET, "/internal/keys").send().await.unwrap().json().await.unwrap();
    assert_eq!(keys["keys"].as_array().unwrap().len(), 1);
    assert!(!keys.to_string().contains(key));
    let other: Value = customer(&app, "bob", Method::GET, "/internal/keys").send().await.unwrap().json().await.unwrap();
    assert_eq!(other["keys"].as_array().unwrap().len(), 0);
    let account: Value = app.get("/v1/account").bearer_auth(key).send().await.unwrap().json().await.unwrap();
    assert_eq!(account["balance_micro_usd"], 0);
    let path = format!("/internal/keys/{}", created["id"].as_str().unwrap());
    assert_eq!(customer(&app, "bob", Method::DELETE, &path).send().await.unwrap().status(), 404);
    assert_eq!(customer(&app, "alice", Method::DELETE, &path).send().await.unwrap().status(), 200);
    assert_eq!(app.get("/v1/account").bearer_auth(key).send().await.unwrap().status(), 401);
}

#[tokio::test]
async fn connecting_an_existing_allowance_binds_it_to_one_identity_and_preserves_spend() {
    let app = spawn().await;
    let (account_id, key) = enclavo_gateway::ledger::grant_operator_credit(&app.db, 1_000_000).await.unwrap();
    assert_eq!(
        customer(&app, "alice", Method::POST, "/internal/account/import")
            .json(&json!({"api_key":key}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        customer(&app, "bob", Method::POST, "/internal/account/import")
            .json(&json!({"api_key":key}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let created: Value = customer(&app, "alice", Method::POST, "/internal/keys")
        .json(&json!({"name":"Second key"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let child = created["api_key"].as_str().unwrap();
    let request =
        customer(&app, "alice", Method::POST, "/internal/chat/completions").json(&chat("m")).send().await.unwrap();
    assert_eq!(request.status(), 200);
    let balance = app.account(child).await;
    assert_eq!(balance["id"], account_id.to_string());
    assert_eq!(balance["balance_micro_usd"], 994_000);
    assert_eq!(app.account(&key).await["balance_micro_usd"], 994_000);
    let keys: Value =
        customer(&app, "alice", Method::GET, "/internal/keys").send().await.unwrap().json().await.unwrap();
    let imported = keys["keys"].as_array().unwrap().iter().find(|k| k["name"] == "Imported account key").unwrap()["id"]
        .as_str()
        .unwrap();
    assert_eq!(
        customer(&app, "alice", Method::DELETE, &format!("/internal/keys/{imported}")).send().await.unwrap().status(),
        200
    );
    assert_eq!(app.get("/v1/account").bearer_auth(&key).send().await.unwrap().status(), 401);
    assert_eq!(
        customer(&app, "alice", Method::POST, "/internal/account/import")
            .json(&json!({"api_key":key}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(app.get("/v1/account").bearer_auth(child).send().await.unwrap().status(), 200);
    assert_eq!(
        customer(&app, "bob", Method::POST, "/internal/chat/completions")
            .json(&chat("m"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

#[tokio::test]
async fn key_creation_has_a_persistent_limit_even_when_requests_race() {
    let app = spawn().await;
    let requests = (0..22).map(|i| {
        customer(&app, "alice", Method::POST, "/internal/keys").json(&json!({"name":format!("Key {i}")})).send()
    });
    let results = futures::future::join_all(requests).await;
    assert_eq!(results.into_iter().filter(|result| result.as_ref().unwrap().status() == 200).count(), 20);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys").fetch_one(&app.db).await.unwrap();
    assert_eq!(count, 20);
}
