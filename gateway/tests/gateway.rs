mod common;

use std::time::Duration;

use common::{chat, decode_header, pay, payment_required, spawn};
use futures::StreamExt;
use serde_json::{Value, json};

/// 10 prompt + 20 completion tokens at $0.20 per 1000-token IU on m@good.
const LIVE_COST: i64 = 6_000;

#[tokio::test]
async fn operator_credit_pays_for_usage_without_fabricating_a_blockchain_payment() {
    let app = common::spawn_with(|mut cfg| {
        cfg.payments.networks.clear();
        cfg
    })
    .await;
    let (_, key) = enclavo_gateway::ledger::grant_operator_credit(&app.db, 1_000_000).await.unwrap();
    let response = app.post("/v1/chat/completions").bearer_auth(&key).json(&chat("m")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(app.account(&key).await["balance_micro_usd"], 1_000_000 - LIVE_COST);
    assert!(app.facilitator.verified.lock().is_empty());
    assert!(app.facilitator.settled.lock().is_empty());
    let payments: i64 = sqlx::query_scalar("SELECT count(*) FROM payments").fetch_one(&app.db).await.unwrap();
    assert_eq!(payments, 0);
    let reference: String = sqlx::query_scalar("SELECT reference FROM ledger_entries WHERE kind = 'topup'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert!(reference.starts_with("operator:grant:"));
    assert!(enclavo_gateway::ledger::grant_operator_credit(&app.db, 0).await.is_err());
    assert!(enclavo_gateway::ledger::grant_operator_credit(&app.db, u64::MAX).await.is_err());
    let response = app.post("/v1/chat/completions").json(&chat("m")).send().await.unwrap();
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn lists_routes_as_models() {
    let app = spawn().await;
    let body: Value = app.get("/v1/models").send().await.unwrap().json().await.unwrap();
    let ids: Vec<&str> = body["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"m@good") && ids.contains(&"enclavo/auto"));
}

#[tokio::test]
async fn unpaid_request_gets_an_upto_offer_for_its_ceiling() {
    let app = spawn().await;
    let response = app.post("/v1/chat/completions").json(&chat("m")).send().await.unwrap();
    assert_eq!(response.status(), 402);
    let required = payment_required(&response);
    assert_eq!(required["x402Version"], 2);
    let offer = &required["accepts"][0];
    assert_eq!(offer["scheme"], "upto");
    assert_eq!(offer["network"], "eip155:84532");
    assert_eq!(offer["payTo"], "0xPAY");
    let ceiling: u64 = offer["amount"].as_str().unwrap().parse().unwrap();
    assert!(ceiling > LIVE_COST as u64, "ceiling {ceiling} must cover the worst case");
    assert!(app.upstream.requests.lock().is_empty(), "nothing runs before payment");
}

#[tokio::test]
async fn x402_request_falls_back_and_settles_actual_cost() {
    let app = spawn().await;
    let first = app.post("/v1/chat/completions").json(&chat("m")).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    let signature = pay(&offer, "sig-1");

    let response =
        app.post("/v1/chat/completions").header("payment-signature", &signature).json(&chat("m")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-enclavo-route"], "m@good", "the cheaper broken route failed over");
    assert_eq!(response.headers()["x-enclavo-cost-micro-usd"], LIVE_COST.to_string().as_str());
    let receipt = decode_header(response.headers()["payment-response"].to_str().unwrap());
    assert_eq!(receipt["success"], true);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["model"], "m@good");

    // The provider saw its own model id and the clamped budget.
    let requests = app.upstream.requests.lock().clone();
    assert_eq!(requests.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["broken", "good"]);
    assert_eq!(requests[1].1["model"], "org/m");
    assert_eq!(requests[1].1["max_tokens"], 50);

    // Verified against the signed maximum, settled for what was used.
    assert_eq!(app.facilitator.verified.lock()[0].amount, offer["amount"].as_str().unwrap());
    assert_eq!(app.facilitator.settled.lock()[0].amount, LIVE_COST.to_string());
    let (status, charged): (String, i64) =
        sqlx::query_as("SELECT status, charged_micro FROM payments").fetch_one(&app.db).await.unwrap();
    assert_eq!((status.as_str(), charged), ("settled", LIVE_COST));

    let replay =
        app.post("/v1/chat/completions").header("payment-signature", &signature).json(&chat("m")).send().await.unwrap();
    assert_eq!(replay.status(), 409, "a payment authorization runs one request");
}

#[tokio::test]
async fn invalid_or_short_payments_are_rejected_before_running() {
    let app = spawn().await;
    let first = app.post("/v1/chat/completions").json(&chat("m")).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();

    let bad = app
        .post("/v1/chat/completions")
        .header("payment-signature", pay(&offer, "bad"))
        .json(&chat("m"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 402);
    assert_eq!(payment_required(&bad)["error"], "invalid_signature");

    let mut short = offer.clone();
    short["amount"] = json!("1");
    let short = app
        .post("/v1/chat/completions")
        .header("payment-signature", pay(&short, "s2"))
        .json(&chat("m"))
        .send()
        .await
        .unwrap();
    assert_eq!(short.status(), 402);

    let mut elsewhere = offer.clone();
    elsewhere["payTo"] = json!("0xTHIEF");
    let elsewhere = app
        .post("/v1/chat/completions")
        .header("payment-signature", pay(&elsewhere, "s3"))
        .json(&chat("m"))
        .send()
        .await
        .unwrap();
    assert_eq!(elsewhere.status(), 402);

    assert!(app.upstream.requests.lock().is_empty());
    assert!(app.facilitator.settled.lock().is_empty());
}

#[tokio::test]
async fn failed_requests_cost_nothing() {
    let app = spawn().await;
    // x402: every route fails, so the authorization is never settled.
    let first = app.post("/v1/chat/completions").json(&chat("fail")).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    let response = app
        .post("/v1/chat/completions")
        .header("payment-signature", pay(&offer, "f1"))
        .json(&chat("fail"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    assert!(app.facilitator.settled.lock().is_empty());

    // Prepaid: the hold is released in full.
    let key = app.funded_account("1").await;
    let response = app.post("/v1/chat/completions").bearer_auth(&key).json(&chat("fail")).send().await.unwrap();
    assert!(response.status() == 502 || response.status() == 503, "{}", response.status());
    let rejected = app.post("/v1/chat/completions").bearer_auth(&key).json(&chat("reject")).send().await.unwrap();
    assert_eq!(rejected.status(), 400, "provider 4xx passes through");
    let body: Value = rejected.json().await.unwrap();
    assert_eq!(body["error"]["message"], "bad prompt");
    let account = app.account(&key).await;
    assert_eq!(account["balance_micro_usd"], 1_000_000);
    assert_eq!(account["held_micro_usd"], 0);
}

#[tokio::test]
async fn rate_limited_provider_opens_its_circuit() {
    let app = spawn().await;
    let key = app.funded_account("1").await;
    for _ in 0..2 {
        app.post("/v1/chat/completions").bearer_auth(&key).json(&chat("fail")).send().await.unwrap();
    }
    let limited_calls = app.upstream.requests.lock().iter().filter(|(k, _)| k == "limited").count();
    assert_eq!(limited_calls, 1, "a 429 with Retry-After takes the route out of rotation");
}

#[tokio::test]
async fn prepaid_account_pays_from_its_balance() {
    let app = spawn().await;
    let key = app.funded_account("2.5").await;
    assert_eq!(app.account(&key).await["balance_micro_usd"], 2_500_000);

    let response = app.post("/v1/chat/completions").bearer_auth(&key).json(&chat("m@good")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("payment-response").is_none());

    let account = app.account(&key).await;
    assert_eq!(account["balance_micro_usd"], 2_500_000 - LIVE_COST);
    assert_eq!(account["held_micro_usd"], 0);
    assert_eq!(account["spent_micro_usd"], LIVE_COST);
    let entries: i64 =
        sqlx::query_scalar("SELECT sum(amount_micro)::bigint FROM ledger_entries").fetch_one(&app.db).await.unwrap();
    assert_eq!(entries, 2_500_000 - LIVE_COST, "the journal matches the balance");
}

#[tokio::test]
async fn low_balance_falls_back_to_x402() {
    let app = spawn().await;
    let key = app.funded_account("1").await;
    sqlx::query("UPDATE accounts SET balance_micro = 10").execute(&app.db).await.unwrap();
    let response = app.post("/v1/chat/completions").bearer_auth(&key).json(&chat("m")).send().await.unwrap();
    assert_eq!(response.status(), 402);
    let required = payment_required(&response);
    assert!(required["error"].as_str().unwrap().contains("Balance"));

    let offer = required["accepts"][0].clone();
    let paid = app
        .post("/v1/chat/completions")
        .bearer_auth(&key)
        .header("payment-signature", pay(&offer, "fallback"))
        .json(&chat("m"))
        .send()
        .await
        .unwrap();
    assert_eq!(paid.status(), 200);
    assert!(paid.headers().get("payment-response").is_some());
    assert_eq!(app.account(&key).await["balance_micro_usd"], 10);
}

async fn read_sse(response: reqwest::Response) -> (Vec<Value>, Vec<String>) {
    let mut text = String::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
    }
    let mut data = Vec::new();
    let mut comments = Vec::new();
    for line in text.lines() {
        if let Some(d) = line.strip_prefix("data: ") {
            data.push(if d == "[DONE]" { json!("[DONE]") } else { serde_json::from_str(d).unwrap() });
        } else if let Some(c) = line.strip_prefix(':') {
            comments.push(c.trim().to_string());
        }
    }
    (data, comments)
}

#[tokio::test]
async fn streams_with_receipt_before_done() {
    let app = spawn().await;
    let mut body = chat("m");
    body["stream"] = json!(true);

    let first = app.post("/v1/chat/completions").json(&body).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    let response = app
        .post("/v1/chat/completions")
        .header("payment-signature", pay(&offer, "st1"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-enclavo-route"], "m@good");
    let (data, comments) = read_sse(response).await;

    let text: String = data.iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
    assert_eq!(text, "Hello");
    assert!(data.iter().filter(|c| c.is_object()).all(|c| c["model"] == "m@good"));
    assert_eq!(data.last().unwrap(), "[DONE]");
    assert!(comments.contains(&format!("enclavo-cost-micro-usd {LIVE_COST}")));
    let receipt = comments.iter().find_map(|c| c.strip_prefix("payment-response ")).expect("payment receipt comment");
    assert_eq!(decode_header(receipt)["success"], true);
    assert_eq!(app.facilitator.settled.lock()[0].amount, LIVE_COST.to_string());

    let requests = app.upstream.requests.lock().clone();
    assert_eq!(requests.last().unwrap().1["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn streaming_charges_prepaid_balance() {
    let app = spawn().await;
    let key = app.funded_account("1").await;
    let mut body = chat("m@good");
    body["stream"] = json!(true);
    let response = app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap();
    let (data, _) = read_sse(response).await;
    assert_eq!(data.last().unwrap(), "[DONE]");
    let account = app.account(&key).await;
    assert_eq!(account["balance_micro_usd"], 1_000_000 - LIVE_COST);
    assert_eq!(account["held_micro_usd"], 0);
}

#[tokio::test]
async fn top_up_replay_does_not_credit_twice() {
    let app = spawn().await;
    let body = json!({ "amount_usd": "5" });
    let first = app.post("/v1/credits/topup").json(&body).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    assert_eq!(offer["scheme"], "exact");
    assert_eq!(offer["amount"], "5000000");
    let signature = pay(&offer, "topup-1");
    let ok = app.post("/v1/credits/topup").header("payment-signature", &signature).json(&body).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    let key = ok.json::<Value>().await.unwrap()["api_key"].as_str().unwrap().to_string();
    let replay = app
        .post("/v1/credits/topup")
        .bearer_auth(&key)
        .header("payment-signature", &signature)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 409);
    assert_eq!(app.account(&key).await["balance_micro_usd"], 5_000_000);

    // A second top-up with a new signature adds to the same account, without a new key.
    let more = app
        .post("/v1/credits/topup")
        .bearer_auth(&key)
        .header("payment-signature", pay(&offer, "topup-2"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(more.json::<Value>().await.unwrap().get("api_key").is_none());
    assert_eq!(app.account(&key).await["balance_micro_usd"], 10_000_000);
}

#[tokio::test]
async fn unreachable_facilitator_leaves_settlement_pending() {
    let app = spawn().await;
    let first = app.post("/v1/chat/completions").json(&chat("m@good")).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    let signature = pay(&offer, "pending-1");
    // Verify succeeds, then the facilitator goes away before settlement.
    let request = app.post("/v1/chat/completions").header("payment-signature", &signature).json(&chat("m@good"));
    let facilitator = app.facilitator.clone();
    let handle = tokio::spawn(async move { request.send().await.unwrap() });
    // The mock answers instantly, so take it down only for the settle by polling verify.
    for _ in 0..200 {
        if !facilitator.verified.lock().is_empty() {
            *facilitator.down.lock() = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let response = handle.await.unwrap();
    assert_eq!(response.status(), 200, "the answer is not held hostage by settlement");
    let status: String = sqlx::query_scalar("SELECT status FROM payments").fetch_one(&app.db).await.unwrap();
    if status == "settle_pending" {
        *app.facilitator.down.lock() = false;
        sqlx::query("UPDATE payments SET updated_at = now() - interval '1 hour'").execute(&app.db).await.unwrap();
        enclavo_gateway::billing::retry_pending_settlements(&app.state).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM payments").fetch_one(&app.db).await.unwrap();
        assert_eq!(status, "settled");
    } else {
        // The settle won the race against the outage; it must have settled cleanly.
        assert_eq!(status, "settled");
    }
}

#[tokio::test]
async fn batch_runs_at_a_discount_and_writes_output() {
    let app = spawn().await;
    tokio::spawn(enclavo_gateway::batch::run(app.state.clone()));
    let key = app.funded_account("1").await;

    let mut jsonl = String::new();
    for i in 0..3 {
        let line = json!({ "custom_id": format!("req-{i}"), "method": "POST", "url": "/v1/chat/completions", "body": chat("m") });
        jsonl.push_str(&format!("{line}\n"));
    }
    jsonl.push_str(&format!(
        "{}\n",
        json!({ "custom_id": "bad", "method": "POST", "url": "/v1/chat/completions", "body": chat("reject") })
    ));
    let form = reqwest::multipart::Form::new()
        .text("purpose", "batch")
        .part("file", reqwest::multipart::Part::bytes(jsonl.into_bytes()).file_name("input.jsonl"));
    let file: Value =
        app.post("/v1/files").bearer_auth(&key).multipart(form).send().await.unwrap().json().await.unwrap();
    assert_eq!(file["purpose"], "batch");

    let created = app
        .post("/v1/batches")
        .bearer_auth(&key)
        .json(&json!({ "input_file_id": file["id"], "endpoint": "/v1/chat/completions", "completion_window": "24h" }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);
    let batch: Value = created.json().await.unwrap();
    assert_eq!(batch["status"], "in_progress");
    assert!(app.account(&key).await["held_micro_usd"].as_i64().unwrap() > 0, "the batch ceiling is held");

    let id = batch["id"].as_str().unwrap().to_string();
    let mut done = Value::Null;
    for _ in 0..200 {
        done = app.get(&format!("/v1/batches/{id}")).bearer_auth(&key).send().await.unwrap().json().await.unwrap();
        if done["status"] == "completed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["request_counts"], json!({ "total": 4, "completed": 3, "failed": 1 }));
    let batch_cost = LIVE_COST * 7 / 10; // 30% batch discount
    assert_eq!(done["enclavo"]["charged_micro_usd"], 3 * batch_cost);

    let output = app
        .get(&format!("/v1/files/{}/content", done["output_file_id"].as_str().unwrap()))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let lines: Vec<Value> = output.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["custom_id"], "req-0");
    assert_eq!(lines[0]["response"]["status_code"], 200);
    assert_eq!(lines[0]["response"]["body"]["model"], "m@good");
    let errors = app
        .get(&format!("/v1/files/{}/content", done["error_file_id"].as_str().unwrap()))
        .bearer_auth(&key)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let error: Value = serde_json::from_str(errors.lines().next().unwrap()).unwrap();
    assert_eq!(error["custom_id"], "bad");
    assert_eq!(error["response"]["status_code"], 400);

    let account = app.account(&key).await;
    assert_eq!(account["held_micro_usd"], 0, "the rest of the hold came back");
    assert_eq!(account["balance_micro_usd"], 1_000_000 - 3 * batch_cost);
}

#[tokio::test]
async fn batch_needs_enough_balance_and_can_be_cancelled() {
    let app = spawn().await; // no batch worker, so the batch stays queued
    let key = app.funded_account("1").await;
    let line = json!({ "custom_id": "a", "method": "POST", "url": "/v1/chat/completions", "body": chat("m") });
    let form = reqwest::multipart::Form::new()
        .text("purpose", "batch")
        .part("file", reqwest::multipart::Part::bytes(format!("{line}\n").into_bytes()).file_name("in.jsonl"));
    let file: Value =
        app.post("/v1/files").bearer_auth(&key).multipart(form).send().await.unwrap().json().await.unwrap();
    let create = json!({ "input_file_id": file["id"], "endpoint": "/v1/chat/completions", "completion_window": "24h" });

    let batch: Value =
        app.post("/v1/batches").bearer_auth(&key).json(&create).send().await.unwrap().json().await.unwrap();
    let id = batch["id"].as_str().unwrap();
    let cancelled: Value =
        app.post(&format!("/v1/batches/{id}/cancel")).bearer_auth(&key).send().await.unwrap().json().await.unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["request_counts"]["failed"], 1);
    assert_eq!(app.account(&key).await["balance_micro_usd"], 1_000_000);

    sqlx::query("UPDATE accounts SET balance_micro = 1").execute(&app.db).await.unwrap();
    let poor = app.post("/v1/batches").bearer_auth(&key).json(&create).send().await.unwrap();
    assert_eq!(poor.status(), 402);
}

#[tokio::test]
async fn validates_requests_and_keys() {
    let app = spawn().await;
    let no_messages = app.post("/v1/chat/completions").json(&json!({ "model": "m" })).send().await.unwrap();
    assert_eq!(no_messages.status(), 400);
    let unknown = app.post("/v1/chat/completions").json(&chat("nope")).send().await.unwrap();
    assert_eq!(unknown.status(), 404);
    let bad_key =
        app.post("/v1/chat/completions").bearer_auth("ek_doesnotexist").json(&chat("m")).send().await.unwrap();
    assert_eq!(bad_key.status(), 401);
    let mut prefs = chat("m");
    prefs["provider"] = json!({ "only": ["nobody"] });
    let none = app.post("/v1/chat/completions").json(&prefs).send().await.unwrap();
    assert_eq!(none.status(), 503);
    // Placeholder SDK keys are ignored rather than rejected, so x402 clients can use stock SDKs.
    let placeholder =
        app.post("/v1/chat/completions").bearer_auth("sk-anything").json(&chat("m")).send().await.unwrap();
    assert_eq!(placeholder.status(), 402);
}

#[tokio::test]
async fn client_leaving_mid_stream_is_still_charged() {
    let app = spawn().await;
    let key = app.funded_account("1").await;
    let mut body = chat("slow");
    body["stream"] = json!(true);
    let response = app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap();
    let mut stream = response.bytes_stream();
    stream.next().await.unwrap().unwrap(); // read one chunk, then hang up
    drop(stream);

    let mut account = Value::Null;
    for _ in 0..60 {
        account = app.account(&key).await;
        if account["held_micro_usd"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(account["held_micro_usd"], 0, "the hold is settled, not left for the sweeper");
    assert_eq!(account["balance_micro_usd"], 1_000_000 - LIVE_COST, "billed from the provider's usage report");
}

#[tokio::test]
async fn re_encoded_payment_is_still_a_replay() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let app = spawn().await;
    let first = app.post("/v1/chat/completions").json(&chat("m@good")).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    let payload = json!({ "x402Version": 2, "accepted": offer, "payload": { "signature": "same", "nonce": "n1" } });
    let compact = STANDARD.encode(serde_json::to_vec(&payload).unwrap());
    let pretty = STANDARD.encode(
        serde_json::to_vec_pretty(&json!({
            "payload": { "nonce": "n1", "signature": "same" }, "accepted": offer, "x402Version": 2, "extensions": {}
        }))
        .unwrap(),
    );
    let ok = app
        .post("/v1/chat/completions")
        .header("payment-signature", compact)
        .json(&chat("m@good"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let replay = app
        .post("/v1/chat/completions")
        .header("payment-signature", pretty)
        .json(&chat("m@good"))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 409);
}

/// Makes stored payments look old enough for the retry pass to pick them up.
async fn age_payments(app: &common::TestApp) {
    sqlx::query("UPDATE payments SET updated_at = now() - interval '2 hours'").execute(&app.db).await.unwrap();
}

async fn payment_status(app: &common::TestApp) -> String {
    sqlx::query_scalar("SELECT status FROM payments WHERE purpose = 'topup' ORDER BY created_at DESC LIMIT 1")
        .fetch_one(&app.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn top_up_interrupted_mid_settle_is_credited_once_on_retry() {
    let app = spawn().await;
    let key = app.funded_account("1").await;
    // A crash after the payment row was written and before the outcome was recorded.
    sqlx::query("UPDATE payments SET status = 'settling', transaction = NULL").execute(&app.db).await.unwrap();
    sqlx::query("UPDATE accounts SET balance_micro = 0").execute(&app.db).await.unwrap();
    sqlx::query("DELETE FROM ledger_entries").execute(&app.db).await.unwrap();
    age_payments(&app).await;

    let billing = enclavo_gateway::billing::retry_pending_settlements;
    billing(&app.state).await.unwrap(); // settling -> settle_pending
    age_payments(&app).await;
    billing(&app.state).await.unwrap(); // resend -> settled + credited
    age_payments(&app).await;
    billing(&app.state).await.unwrap(); // nothing left to do
    assert_eq!(payment_status(&app).await, "settled");
    assert_eq!(app.account(&key).await["balance_micro_usd"], 1_000_000, "credited exactly once");
}

#[tokio::test]
async fn pending_settlement_is_not_a_failure() {
    let app = spawn().await;
    *app.facilitator.pending.lock() = true;
    let body = json!({ "amount_usd": "3" });
    let first = app.post("/v1/credits/topup").json(&body).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    let response = app
        .post("/v1/credits/topup")
        .header("payment-signature", pay(&offer, "pend-1"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "pending");
    let key = body["api_key"].as_str().expect("the key is handed back while the credit is pending").to_string();
    assert_eq!(app.account(&key).await["balance_micro_usd"], 0);
    assert_eq!(payment_status(&app).await, "settle_pending");

    // The transaction confirms; one resend of the identical request reports success.
    *app.facilitator.pending.lock() = false;
    age_payments(&app).await;
    enclavo_gateway::billing::retry_pending_settlements(&app.state).await.unwrap();
    assert_eq!(payment_status(&app).await, "settled");
    assert_eq!(app.account(&key).await["balance_micro_usd"], 3_000_000);
    let tx: String = sqlx::query_scalar("SELECT transaction FROM payments").fetch_one(&app.db).await.unwrap();
    assert_eq!(tx, "0xtx1");
}

/// Starts a $2 top-up whose first settlement comes back `settlement_pending`.
async fn pending_top_up(app: &common::TestApp, signature: &str) {
    *app.facilitator.pending.lock() = true;
    let body = json!({ "amount_usd": "2" });
    let first = app.post("/v1/credits/topup").json(&body).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    let response = app
        .post("/v1/credits/topup")
        .header("payment-signature", pay(&offer, signature))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
}

async fn retry_once(app: &common::TestApp) {
    age_payments(app).await;
    enclavo_gateway::billing::retry_pending_settlements(&app.state).await.unwrap();
}

#[tokio::test]
async fn still_pending_after_one_resend_goes_to_review() {
    let app = spawn().await;
    pending_top_up(&app, "rev-1").await;
    retry_once(&app).await; // the one resend CDP allows, still pending
    retry_once(&app).await;
    assert_eq!(payment_status(&app).await, "needs_review");
    assert!(app.facilitator.settled.lock().is_empty());
}

#[tokio::test]
async fn rejection_on_a_resend_goes_to_review_not_failed() {
    let app = spawn().await;
    pending_top_up(&app, "rev-2").await;
    // The resend is rejected, which may only mean the first broadcast already landed.
    *app.facilitator.pending.lock() = false;
    *app.facilitator.reject_settle.lock() = true;
    retry_once(&app).await;
    assert_eq!(payment_status(&app).await, "needs_review");
}

#[tokio::test]
async fn rejection_on_the_first_attempt_is_a_plain_failure() {
    let app = spawn().await;
    *app.facilitator.reject_settle.lock() = true;
    let body = json!({ "amount_usd": "2" });
    let first = app.post("/v1/credits/topup").json(&body).send().await.unwrap();
    let offer = payment_required(&first)["accepts"][0].clone();
    let response = app
        .post("/v1/credits/topup")
        .header("payment-signature", pay(&offer, "rev-3"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 402);
    assert_eq!(payment_status(&app).await, "failed");
}

#[tokio::test]
async fn rejects_absurd_choice_counts() {
    let app = spawn().await;
    let mut body = chat("m");
    body["n"] = json!(1u64 << 51);
    let response = app.post("/v1/chat/completions").json(&body).send().await.unwrap();
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn shutdown_drains_open_streams_before_exit() {
    let app = spawn().await;
    let key = app.funded_account("1").await;
    let mut body = chat("slow");
    body["stream"] = json!(true);
    let response = app.post("/v1/chat/completions").bearer_auth(&key).json(&body).send().await.unwrap();
    assert_eq!(response.status(), 200);

    // Shutdown starts while the answer is still streaming.
    app.state.shutdown.cancel();
    let ready = app.get("/readyz").send().await.unwrap();
    assert_eq!(ready.status(), 503, "a draining instance reports not ready");

    let reader = tokio::spawn(read_sse(response));
    assert!(enclavo_gateway::drain(&app.state, Duration::from_secs(10)).await, "the stream finished within the drain");
    let (data, _) = reader.await.unwrap();
    assert_eq!(data.last().unwrap(), "[DONE]", "the client got the whole answer");
    let account = app.account(&key).await;
    assert_eq!(account["held_micro_usd"], 0);
    assert_eq!(account["balance_micro_usd"], 1_000_000 - LIVE_COST);
}

#[tokio::test]
async fn metrics_stay_off_the_public_port_when_a_private_one_is_set() {
    let app = common::spawn_with(|mut cfg| {
        cfg.server.metrics_listen = Some("127.0.0.1:0".into());
        cfg
    })
    .await;
    assert_eq!(app.get("/metrics").send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn per_ip_limit_uses_the_trusted_proxy_header() {
    let app = common::spawn_with(|mut cfg| {
        cfg.server.client_ip_header = Some("fly-client-ip".into());
        cfg.limits.per_ip_rps = 1;
        cfg.limits.per_ip_burst = 1;
        cfg
    })
    .await;
    let send = |ip: &'static str| app.get("/v1/models").header("fly-client-ip", ip).send();
    assert_eq!(send("203.0.113.1").await.unwrap().status(), 200);
    assert_eq!(send("203.0.113.1").await.unwrap().status(), 429, "same client, over its limit");
    assert_eq!(send("203.0.113.2").await.unwrap().status(), 200, "another client has its own budget");
}
