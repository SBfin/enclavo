//! Chain-agnostic x402: networks come from config, the facilitator does the chain work.

pub mod cdp;
pub mod facilitator;
pub mod types;

use serde_json::{Value, json};

use crate::{
    config::{NetworkConfig, PaymentFlow, PaymentsConfig, Scheme},
    error::ApiError,
};
use types::{PaymentPayload, PaymentRequired, PaymentRequirements, ResourceInfo, X402_VERSION};

#[derive(Debug, Clone)]
pub struct Payments {
    cfg: PaymentsConfig,
    public_url: String,
}

/// A payment that matched one of the offers, with the requirements to verify and settle against.
#[derive(Debug, Clone)]
pub struct Matched {
    pub network: NetworkConfig,
    pub requirements: PaymentRequirements,
    pub authorized_atomic: u128,
    pub flow: PaymentFlow,
}

impl Payments {
    pub fn new(cfg: PaymentsConfig, public_url: &str) -> Self {
        Self { cfg, public_url: public_url.trim_end_matches('/').to_string() }
    }

    pub fn config(&self) -> &PaymentsConfig {
        &self.cfg
    }

    pub fn accepts(&self, scheme: &Scheme) -> bool {
        self.cfg.networks.iter().any(|n| n.schemes.contains(scheme))
    }

    fn requirement(&self, network: &NetworkConfig, scheme: &Scheme, amount_atomic: u128) -> PaymentRequirements {
        let mut extra = network.extra.clone().unwrap_or_else(|| json!({}));
        if *scheme == Scheme::Upto && network.upto_flow == PaymentFlow::Escrow {
            // Non-default flows must be announced (spec §6.1).
            if let Value::Object(map) = &mut extra {
                map.insert("paymentFlow".into(), "escrow".into());
            }
        }
        let extra = match &extra {
            Value::Object(map) if map.is_empty() => None,
            _ => Some(extra),
        };
        PaymentRequirements {
            scheme: scheme.as_str().into(),
            network: network.network.clone(),
            amount: amount_atomic.to_string(),
            asset: network.asset.clone(),
            pay_to: network.pay_to.clone(),
            max_timeout_seconds: self.cfg.max_timeout_secs,
            extra,
        }
    }

    /// The 402 body offering `amount_micro` under `scheme` on every network that accepts it.
    pub fn required(&self, path: &str, description: &str, scheme: &Scheme, amount_micro: u64, error: &str) -> ApiError {
        let accepts = self
            .cfg
            .networks
            .iter()
            .filter(|n| n.schemes.contains(scheme))
            .map(|n| self.requirement(n, scheme, to_atomic(amount_micro, n.asset_decimals)))
            .collect();
        ApiError::PaymentRequired(Box::new(PaymentRequired {
            x402_version: X402_VERSION,
            error: Some(error.to_string()),
            resource: ResourceInfo {
                url: format!("{}{path}", self.public_url),
                description: Some(description.to_string()),
                mime_type: Some("application/json".into()),
                service_name: Some(self.cfg.service_name.chars().take(32).collect()),
            },
            accepts,
            extensions: None,
        }))
    }

    /// Checks that a payment answers one of our offers: same scheme, network, asset and recipient,
    /// and at least `min_micro` (exactly, for `exact`). Returns the requirements to send the facilitator.
    pub fn match_payload(&self, payload: &PaymentPayload, scheme: &Scheme, min_micro: u64) -> Result<Matched, String> {
        let accepted = &payload.accepted;
        if accepted.scheme != scheme.as_str() {
            return Err(format!("this endpoint takes the `{}` scheme", scheme.as_str()));
        }
        let network = self
            .cfg
            .networks
            .iter()
            .find(|n| n.network == accepted.network && n.schemes.contains(scheme))
            .ok_or_else(|| format!("network {} is not accepted", accepted.network))?;
        if !same_address(&accepted.asset, &network.asset) {
            return Err("payment asset does not match the offer".into());
        }
        if !same_address(&accepted.pay_to, &network.pay_to) {
            return Err("payment recipient does not match the offer".into());
        }
        let authorized: u128 = accepted.amount.parse().map_err(|_| "payment amount is not an integer".to_string())?;
        let needed = to_atomic(min_micro, network.asset_decimals);
        match scheme {
            Scheme::Exact if authorized != needed => {
                return Err(format!("exact payment must be {needed} atomic units"));
            }
            Scheme::Upto if authorized < needed => {
                return Err(format!("authorized maximum {authorized} is below this request's ceiling of {needed}"));
            }
            _ => {}
        }
        let flow = if *scheme == Scheme::Upto { network.upto_flow } else { PaymentFlow::Authorization };
        Ok(Matched {
            network: network.clone(),
            requirements: self.requirement(network, scheme, authorized),
            authorized_atomic: authorized,
            flow,
        })
    }
}

/// EVM addresses compare case-insensitively (checksum casing); others, such as base58, exactly.
fn same_address(a: &str, b: &str) -> bool {
    if a.starts_with("0x") && b.starts_with("0x") { a.eq_ignore_ascii_case(b) } else { a == b }
}

pub fn to_atomic(micro: u64, decimals: u32) -> u128 {
    micro as u128 * 10u128.pow(decimals - 6)
}

/// Whole micro-USD in an atomic amount, rounded down.
pub fn to_micro(atomic: u128, decimals: u32) -> u64 {
    u64::try_from(atomic / 10u128.pow(decimals - 6)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payments() -> Payments {
        Payments::new(
            PaymentsConfig {
                facilitator_url: "http://f".into(),
                facilitator_auth_env: None,
                facilitator_auth: Default::default(),
                cdp_key_id_env: String::new(),
                cdp_key_secret_env: String::new(),
                facilitator_timeout_secs: 5,
                service_name: "Enclavo".into(),
                max_timeout_secs: 60,
                min_topup_micro: 1,
                max_topup_micro: 1_000_000_000,
                max_settle_attempts: 3,
                networks: vec![
                    NetworkConfig {
                        network: "eip155:8453".into(),
                        asset: "0xAbC".into(),
                        pay_to: "0xDeF".into(),
                        asset_decimals: 6,
                        schemes: vec![Scheme::Upto, Scheme::Exact],
                        upto_flow: PaymentFlow::Authorization,
                        extra: Some(json!({ "name": "USD Coin", "version": "2" })),
                    },
                    NetworkConfig {
                        network: "solana:5eykt".into(),
                        asset: "EPjF".into(),
                        pay_to: "Merch".into(),
                        asset_decimals: 6,
                        schemes: vec![Scheme::Upto],
                        upto_flow: PaymentFlow::Escrow,
                        extra: None,
                    },
                ],
            },
            "https://gw.example/",
        )
    }

    fn payload(scheme: &str, network: &str, asset: &str, pay_to: &str, amount: &str) -> PaymentPayload {
        PaymentPayload {
            x402_version: 2,
            resource: None,
            accepted: PaymentRequirements {
                scheme: scheme.into(),
                network: network.into(),
                amount: amount.into(),
                asset: asset.into(),
                pay_to: pay_to.into(),
                max_timeout_seconds: 60,
                extra: None,
            },
            payload: json!({}),
            extensions: None,
        }
    }

    #[test]
    fn offers_every_network_for_the_scheme() {
        let ApiError::PaymentRequired(req) =
            payments().required("/v1/chat/completions", "chat", &Scheme::Upto, 1234, "pay")
        else {
            panic!("expected a 402")
        };
        assert_eq!(req.accepts.len(), 2);
        assert_eq!(req.resource.url, "https://gw.example/v1/chat/completions");
        assert_eq!(req.accepts[0].amount, "1234");
        assert_eq!(req.accepts[1].extra.as_ref().unwrap()["paymentFlow"], "escrow");
        let ApiError::PaymentRequired(req) = payments().required("/t", "t", &Scheme::Exact, 1, "pay") else { panic!() };
        assert_eq!(req.accepts.len(), 1);
    }

    #[test]
    fn matches_only_our_offers() {
        let p = payments();
        let ok =
            p.match_payload(&payload("upto", "eip155:8453", "0xabc", "0xdef", "5000"), &Scheme::Upto, 4000).unwrap();
        assert_eq!(ok.authorized_atomic, 5000);
        assert_eq!(ok.requirements.amount, "5000");
        assert_eq!(ok.requirements.pay_to, "0xDeF");
        assert!(
            p.match_payload(&payload("upto", "eip155:8453", "0xabc", "0xdef", "3999"), &Scheme::Upto, 4000).is_err()
        );
        assert!(p.match_payload(&payload("upto", "eip155:8453", "0xabc", "0xbad", "5000"), &Scheme::Upto, 1).is_err());
        assert!(p.match_payload(&payload("upto", "solana:5eykt", "EPjF", "merch", "5"), &Scheme::Upto, 1).is_err());
        assert!(p.match_payload(&payload("exact", "solana:5eykt", "EPjF", "Merch", "5"), &Scheme::Exact, 5).is_err());
        assert!(p.match_payload(&payload("exact", "eip155:8453", "0xabc", "0xdef", "6"), &Scheme::Exact, 5).is_err());
        let escrow = p.match_payload(&payload("upto", "solana:5eykt", "EPjF", "Merch", "5"), &Scheme::Upto, 1).unwrap();
        assert_eq!(escrow.flow, PaymentFlow::Escrow);
    }

    #[test]
    fn converts_between_micro_and_atomic() {
        assert_eq!(to_atomic(1_500_000, 6), 1_500_000);
        assert_eq!(to_atomic(1, 18), 1_000_000_000_000);
        assert_eq!(to_micro(1_999_999_999_999, 18), 1);
    }
}
