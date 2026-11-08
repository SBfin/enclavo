//! x402 v2 wire types (specs/x402-specification-v2.md).

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const X402_VERSION: u32 = 2;
pub const PAYMENT_REQUIRED_HEADER: &str = "payment-required";
pub const PAYMENT_SIGNATURE_HEADER: &str = "payment-signature";
pub const PAYMENT_RESPONSE_HEADER: &str = "payment-response";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirements {
    pub scheme: String,
    pub network: String,
    /// Atomic units, as a decimal string.
    pub amount: String,
    pub asset: String,
    pub pay_to: String,
    pub max_timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequired {
    pub x402_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub resource: ResourceInfo,
    pub accepts: Vec<PaymentRequirements>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPayload {
    pub x402_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceInfo>,
    pub accepted: PaymentRequirements,
    /// Scheme-specific; the facilitator interprets it.
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacilitatorRequest<'a> {
    pub x402_version: u32,
    pub payment_payload: &'a PaymentPayload,
    pub payment_requirements: &'a PaymentRequirements,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResponse {
    pub is_valid: bool,
    #[serde(default)]
    pub invalid_reason: Option<String>,
    #[serde(default)]
    pub invalid_message: Option<String>,
    #[serde(default)]
    pub payer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleResponse {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    #[serde(default)]
    pub transaction: String,
    pub network: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
}

fn encode(value: &impl Serialize) -> String {
    STANDARD.encode(serde_json::to_vec(value).expect("x402 types always serialize"))
}

impl PaymentRequired {
    pub fn to_header(&self) -> String {
        encode(self)
    }
}

impl SettleResponse {
    pub fn to_header(&self) -> String {
        encode(self)
    }
}

impl PaymentPayload {
    /// Decodes a `PAYMENT-SIGNATURE` header (base64 JSON).
    pub fn from_header(value: &str) -> Result<Self, String> {
        let bytes = STANDARD.decode(value.trim()).map_err(|_| "PAYMENT-SIGNATURE is not valid base64".to_string())?;
        let payload: Self = serde_json::from_slice(&bytes)
            .map_err(|e| format!("PAYMENT-SIGNATURE is not a valid payment payload: {e}"))?;
        if payload.x402_version != X402_VERSION {
            return Err(format!("x402Version {} is not supported; use {X402_VERSION}", payload.x402_version));
        }
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn payload_round_trips_through_the_header() {
        let raw = json!({
            "x402Version": 2,
            "accepted": {
                "scheme": "upto", "network": "eip155:84532", "amount": "1000",
                "asset": "0xA", "payTo": "0xB", "maxTimeoutSeconds": 60
            },
            "payload": { "signature": "0x01" }
        });
        let header = STANDARD.encode(serde_json::to_vec(&raw).unwrap());
        let payload = PaymentPayload::from_header(&header).unwrap();
        assert_eq!(payload.accepted.pay_to, "0xB");
        assert_eq!(payload.accepted.max_timeout_seconds, 60);
        let back = serde_json::to_value(&payload).unwrap();
        assert_eq!(back["accepted"]["payTo"], "0xB");
    }

    #[test]
    fn rejects_v1_payloads() {
        let raw = json!({ "x402Version": 1, "accepted": {
            "scheme": "exact", "network": "base", "amount": "1", "asset": "a", "payTo": "b", "maxTimeoutSeconds": 1
        }, "payload": {} });
        let header = STANDARD.encode(serde_json::to_vec(&raw).unwrap());
        assert!(PaymentPayload::from_header(&header).is_err());
    }
}
