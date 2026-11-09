#![allow(dead_code)]

use std::{net::SocketAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use enclavo_gateway::{
    AppState, MIGRATOR, Shared, app,
    config::Config,
    x402::{
        facilitator::{Facilitator, FacilitatorUnavailable},
        types::{PaymentPayload, PaymentRequirements, SettleResponse, VerifyResponse},
    },
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool};

/// Provider stand-in. The first path segment picks the behavior:
/// `good` answers, `broken` fails with 503, `limited` answers 429, `reject` answers 400.
#[derive(Default)]
pub struct MockUpstream {
    pub requests: Mutex<Vec<(String, Value)>>,
}

async fn upstream_handler(
    State(mock): State<Arc<MockUpstream>>,
    Path(kind): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    mock.requests.lock().push((kind.clone(), body.clone()));
    match kind.as_str() {
        "broken" => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": { "message": "down" } }))).into_response(),
        "limited" => (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "30")], Json(json!({ "error": { "message": "slow down" } })))
            .into_response(),
        "reject" => (StatusCode::BAD_REQUEST, Json(json!({ "error": { "message": "bad prompt", "code": "invalid" } }))).into_response(),
        "slow" => {
            // Ten chunks 100ms apart, then usage: long enough for a client to walk away mid-answer.
            let model = body["model"].as_str().unwrap_or_default().to_string();
            let events = futures::stream::unfold(0u32, move |i| {
                let model = model.clone();
                async move {
                    if i > 11 {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let line = match i {
                        0..=9 => json!({ "id": "s", "model": model, "choices": [{ "index": 0, "delta": { "content": "word " } }] }).to_string(),
                        10 => json!({ "id": "s", "model": model, "choices": [],
                                      "usage": { "prompt_tokens": 10, "completion_tokens": 20 } }).to_string(),
                        _ => "[DONE]".to_string(),
                    };
                    Some((Ok::<_, std::convert::Infallible>(format!("data: {line}\n\n")), i + 1))
                }
            });
            ([("content-type", "text/event-stream")], axum::body::Body::from_stream(events)).into_response()
        }
        _ if body["stream"] == true => {
            let model = body["model"].as_str().unwrap_or_default();
            let chunk = |content: &str| {
                json!({ "id": "c1", "object": "chat.completion.chunk", "model": model,
                        "choices": [{ "index": 0, "delta": { "content": content } }] })
            };
            let usage = json!({ "id": "c1", "object": "chat.completion.chunk", "model": model, "choices": [],
                                "usage": { "prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30 } });
            let text = format!("data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n", chunk("Hel"), chunk("lo"), usage);
            ([("content-type", "text/event-stream")], text).into_response()
        }
        _ => Json(json!({
            "id": "c1", "object": "chat.completion", "model": body["model"],
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "Hello" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30 }
        }))
        .into_response(),
    }
}

/// Facilitator stand-in: signatures equal to `bad` are invalid; `down` makes it unreachable.
#[derive(Default)]
pub struct MockFacilitator {
    pub verified: Mutex<Vec<PaymentRequirements>>,
    pub settled: Mutex<Vec<PaymentRequirements>>,
    pub down: Mutex<bool>,
    /// Settle answers `settlement_pending` (broadcast, not confirmed).
    pub pending: Mutex<bool>,
    /// Settle answers a terminal error.
    pub reject_settle: Mutex<bool>,
}

#[async_trait]
impl Facilitator for MockFacilitator {
    async fn verify(
        &self,
        payload: &PaymentPayload,
        req: &PaymentRequirements,
    ) -> Result<VerifyResponse, FacilitatorUnavailable> {
        if *self.down.lock() {
            return Err(FacilitatorUnavailable("down".into()));
        }
        self.verified.lock().push(req.clone());
        let valid = payload.payload["signature"] != "bad";
        Ok(VerifyResponse {
            is_valid: valid,
            invalid_reason: (!valid).then(|| "invalid_signature".into()),
            invalid_message: None,
            payer: Some("0xPAYER".into()),
        })
    }

    async fn settle(
        &self,
        _payload: &PaymentPayload,
        req: &PaymentRequirements,
    ) -> Result<SettleResponse, FacilitatorUnavailable> {
        if *self.down.lock() {
            return Err(FacilitatorUnavailable("down".into()));
        }
        if *self.pending.lock() || *self.reject_settle.lock() {
            let pending = *self.pending.lock();
            return Ok(SettleResponse {
                success: false,
                error_reason: Some(if pending { "settlement_pending" } else { "invalid_transaction_state" }.into()),
                error_message: None,
                payer: Some("0xPAYER".into()),
                transaction: if pending { "0xpending".into() } else { String::new() },
                network: req.network.clone(),
                amount: None,
            });
        }
        self.settled.lock().push(req.clone());
        Ok(SettleResponse {
            success: true,
            error_reason: None,
            error_message: None,
            payer: Some("0xPAYER".into()),
            transaction: format!("0xtx{}", self.settled.lock().len()),
            network: req.network.clone(),
            amount: Some(req.amount.clone()),
        })
    }
}

pub struct TestApp {
    pub url: String,
    pub http: reqwest::Client,
    pub state: Shared,
    pub upstream: Arc<MockUpstream>,
    pub facilitator: Arc<MockFacilitator>,
    pub db: PgPool,
}

fn admin_url() -> String {
    std::env::var("TEST_DATABASE_ADMIN_URL").unwrap_or_else(|_| {
        let user = std::env::var("USER").unwrap_or_else(|_| "postgres".into());
        format!("postgres://{user}@localhost/postgres")
    })
}

static CLEANUP: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Drops test databases left by earlier runs (once per test binary).
async fn drop_stale_databases(conn: &mut PgConnection) {
    let stale: Vec<String> = sqlx::query_scalar(
        "SELECT datname FROM pg_database d WHERE datname LIKE 'enclavo_test_%'
           AND NOT EXISTS (SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname)",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap_or_default();
    for name in stale {
        let _ = sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS {name}"))).execute(&mut *conn).await;
    }
}

async fn fresh_database() -> String {
    let admin = admin_url();
    let name = format!("enclavo_test_{}", uuid::Uuid::new_v4().simple());
    let mut conn = PgConnection::connect(&admin).await.expect("connect to the Postgres admin database");
    CLEANUP.get_or_init(|| drop_stale_databases(&mut conn)).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&mut conn)
        .await
        .expect("create test database");
    let base = admin.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap();
    format!("{base}/{name}")
}

async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    addr
}

pub fn config(upstream: SocketAddr, extra: &str) -> Config {
    let base = format!("http://{upstream}");
    let raw = format!(
        r#"
[server]
listen = "127.0.0.1:0"
public_url = "http://gateway.test"

[database]
url = "unused"

[pricing]
markup_bps = 0
default_max_tokens = 100

[router]
max_attempts = 3
first_byte_timeout_secs = 5
failure_threshold = 2
cooldown_secs = 60

[limits]
per_key_rps = 1000
per_key_burst = 1000
per_ip_rps = 1000
per_ip_burst = 1000

[batch]
discount_bps = 3000
poll_interval_ms = 50
max_item_attempts = 2

[payments]
facilitator_url = "http://unused"
min_topup_micro = 1000000

[[payments.networks]]
network = "eip155:84532"
asset = "0xUSDC"
pay_to = "0xPAY"
schemes = ["upto", "exact"]
extra = {{ name = "USDC", version = "2" }}

[[providers]]
id = "broken"
base_url = "{base}/broken"

[[providers]]
id = "good"
base_url = "{base}/good"

[[providers]]
id = "limited"
base_url = "{base}/limited"

[[providers]]
id = "reject"
base_url = "{base}/reject"

[[routes]]
id = "m@broken"
model = "m"
display_name = "M"
provider = "broken"
upstream_model = "org/m-broken"
attestation = "TEE"
attested = true
score = 42
tokens_per_iu = 1000
micro_usd_per_iu = 100000

[[routes]]
id = "m@good"
model = "m"
display_name = "M"
provider = "good"
upstream_model = "org/m"
attestation = "TEE"
attested = true
score = 42
tokens_per_iu = 1000
micro_usd_per_iu = 200000

[[routes]]
id = "fail@limited"
model = "fail"
display_name = "Fail"
provider = "limited"
upstream_model = "x"
attestation = "none"
score = 10
tokens_per_iu = 1000
micro_usd_per_iu = 100000

[[routes]]
id = "fail@broken"
model = "fail"
display_name = "Fail"
provider = "broken"
upstream_model = "x"
attestation = "none"
score = 10
tokens_per_iu = 1000
micro_usd_per_iu = 100000

[[providers]]
id = "slow"
base_url = "{base}/slow"

[[routes]]
id = "slow@slow"
model = "slow"
display_name = "Slow"
provider = "slow"
upstream_model = "s"
attestation = "TEE"
score = 42
tokens_per_iu = 1000
micro_usd_per_iu = 200000

[[routes]]
id = "reject@reject"
model = "reject"
display_name = "Reject"
provider = "reject"
upstream_model = "x"
attestation = "none"
score = 10
tokens_per_iu = 1000
micro_usd_per_iu = 100000
{extra}
"#
    );
    let cfg: Config = toml::from_str(&raw).expect("test config parses");
    cfg.validate().expect("test config is valid");
    cfg
}

pub async fn spawn() -> TestApp {
    spawn_with(|cfg| cfg).await
}

pub async fn spawn_with(adjust: impl FnOnce(Config) -> Config) -> TestApp {
    let upstream = Arc::new(MockUpstream::default());
    let upstream_addr =
        serve(Router::new().route("/{kind}/chat/completions", post(upstream_handler)).with_state(upstream.clone()))
            .await;
    let cfg = adjust(config(upstream_addr, ""));

    let db = PgPool::connect(&fresh_database().await).await.unwrap();
    MIGRATOR.run(&db).await.unwrap();
    let facilitator = Arc::new(MockFacilitator::default());
    let mut state = AppState::new(cfg, db.clone(), facilitator.clone(), None).unwrap();
    Arc::get_mut(&mut state).unwrap().control_token_hash =
        Some(enclavo_gateway::control::token_hash("test-control-token"));
    let addr = serve(app(state.clone())).await;
    TestApp {
        url: format!("http://{addr}"),
        http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap(),
        state,
        upstream,
        facilitator,
        db,
    }
}

pub fn chat(model: &str) -> Value {
    json!({ "model": model, "messages": [{ "role": "user", "content": "Say hello" }], "max_tokens": 50 })
}

/// Decodes the `PAYMENT-REQUIRED` header of a 402.
pub fn payment_required(response: &reqwest::Response) -> Value {
    let header = response.headers().get("payment-required").expect("PAYMENT-REQUIRED header").to_str().unwrap();
    serde_json::from_slice(&STANDARD.decode(header).unwrap()).unwrap()
}

/// A client payment answering the first offer, with a fresh (or chosen) signature.
pub fn pay(offer: &Value, signature: &str) -> String {
    let payload = json!({ "x402Version": 2, "accepted": offer, "payload": { "signature": signature } });
    STANDARD.encode(serde_json::to_vec(&payload).unwrap())
}

pub fn decode_header(value: &str) -> Value {
    serde_json::from_slice(&STANDARD.decode(value).unwrap()).unwrap()
}

impl TestApp {
    pub fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.post(format!("{}{path}", self.url))
    }

    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(format!("{}{path}", self.url))
    }

    /// Tops up a new account over x402 and returns its API key.
    pub async fn funded_account(&self, usd: &str) -> String {
        let body = json!({ "amount_usd": usd });
        let first = self.post("/v1/credits/topup").json(&body).send().await.unwrap();
        assert_eq!(first.status(), 402);
        let offer = payment_required(&first)["accepts"][0].clone();
        let paid = self
            .post("/v1/credits/topup")
            .header("payment-signature", pay(&offer, &uuid::Uuid::new_v4().to_string()))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(paid.status(), 200, "{}", paid.text().await.unwrap());
        let body: Value = paid.json().await.unwrap();
        body["api_key"].as_str().unwrap().to_string()
    }

    pub async fn account(&self, key: &str) -> Value {
        self.get("/v1/account").bearer_auth(key).send().await.unwrap().json().await.unwrap()
    }
}
