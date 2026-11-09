//! The real HTTP facilitator client against a local stand-in: CDP JWT auth and verdict parsing.

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::HeaderMap, http::StatusCode, response::IntoResponse, routing::post};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use enclavo_gateway::x402::{
    cdp::CdpSigner,
    facilitator::{Facilitator, FacilitatorAuth, HttpFacilitator},
    types::{PaymentPayload, PaymentRequirements},
};
use parking_lot::Mutex;
use serde_json::{Value, json};

#[derive(Default)]
struct Seen {
    auth: Mutex<Vec<String>>,
}

async fn verify(State(seen): State<Arc<Seen>>, headers: HeaderMap) -> impl IntoResponse {
    seen.auth.lock().push(headers["authorization"].to_str().unwrap().to_string());
    Json(json!({ "isValid": true, "payer": "0xP" }))
}

async fn settle(State(seen): State<Arc<Seen>>, headers: HeaderMap) -> impl IntoResponse {
    seen.auth.lock().push(headers["authorization"].to_str().unwrap().to_string());
    // CDP reports an unconfirmed broadcast as a 500 with a verdict body.
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "success": false, "errorReason": "settlement_pending", "transaction": "0xabc", "network": "eip155:8453"
        })),
    )
}

fn payload() -> (PaymentPayload, PaymentRequirements) {
    let req = PaymentRequirements {
        scheme: "upto".into(),
        network: "eip155:8453".into(),
        amount: "10".into(),
        asset: "0xA".into(),
        pay_to: "0xB".into(),
        max_timeout_seconds: 60,
        extra: None,
    };
    (
        PaymentPayload { x402_version: 2, resource: None, accepted: req.clone(), payload: json!({}), extensions: None },
        req,
    )
}

#[tokio::test]
async fn cdp_auth_signs_each_call_for_its_own_path() {
    let seen = Arc::new(Seen::default());
    let router = Router::new()
        .route("/platform/v2/x402/verify", post(verify))
        .route("/platform/v2/x402/settle", post(settle))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let seed = [5u8; 32];
    let public = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
    let secret = STANDARD.encode([seed.as_slice(), public.as_bytes()].concat());
    let auth = FacilitatorAuth::Cdp(Box::new(CdpSigner::new("my-key", &secret).unwrap()));
    let facilitator =
        HttpFacilitator::new(&format!("http://{addr}/platform/v2/x402"), auth, std::time::Duration::from_secs(5))
            .unwrap();

    let (p, r) = payload();
    assert!(facilitator.verify(&p, &r).await.unwrap().is_valid);
    let settled = facilitator.settle(&p, &r).await.expect("a 500 with a verdict body is a verdict, not an outage");
    assert!(!settled.success);
    assert_eq!(settled.error_reason.as_deref(), Some("settlement_pending"));
    assert_eq!(settled.transaction, "0xabc");

    let auth = seen.auth.lock().clone();
    for (header, path) in auth.iter().zip(["verify", "settle"]) {
        let token = header.strip_prefix("Bearer ").expect("bearer token");
        let parts: Vec<&str> = token.split('.').collect();
        let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["uri"], format!("POST {addr}/platform/v2/x402/{path}"));
        assert_eq!(claims["sub"], "my-key");
        let sig = ed25519_dalek::Signature::from_bytes(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap().try_into().unwrap());
        public.verify_strict(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).unwrap();
    }
    assert_ne!(auth[0], auth[1], "a fresh token per call");
}

#[tokio::test]
async fn rejected_credentials_are_an_outage_not_a_verdict() {
    let router = Router::new()
        .route("/verify", post(|| async { (StatusCode::UNAUTHORIZED, Json(json!({ "error": "bad jwt" }))) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let facilitator =
        HttpFacilitator::new(&format!("http://{addr}"), FacilitatorAuth::None, std::time::Duration::from_secs(5))
            .unwrap();
    let (p, r) = payload();
    assert!(facilitator.verify(&p, &r).await.is_err());
}
