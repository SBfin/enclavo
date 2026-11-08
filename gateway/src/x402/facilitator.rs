use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;

use super::cdp::CdpSigner;
use super::types::{
    FacilitatorRequest, PaymentPayload, PaymentRequirements, SettleResponse, VerifyResponse, X402_VERSION,
};
use crate::config::{FacilitatorAuthKind, PaymentsConfig};

/// The facilitator could not be reached or answered with something other than a verdict.
#[derive(Debug, thiserror::Error)]
#[error("facilitator unavailable: {0}")]
pub struct FacilitatorUnavailable(pub String);

/// Verifies and settles payments. All chain-specific work happens behind this.
#[async_trait]
pub trait Facilitator: Send + Sync {
    async fn verify(
        &self,
        payload: &PaymentPayload,
        req: &PaymentRequirements,
    ) -> Result<VerifyResponse, FacilitatorUnavailable>;
    async fn settle(
        &self,
        payload: &PaymentPayload,
        req: &PaymentRequirements,
    ) -> Result<SettleResponse, FacilitatorUnavailable>;
}

pub enum FacilitatorAuth {
    None,
    /// A fixed `Authorization` header value.
    Static(String),
    /// A fresh CDP JWT for every call.
    Cdp(Box<CdpSigner>),
}

impl FacilitatorAuth {
    /// Reads credentials named in config from the environment. Fails if the chosen method has none.
    pub fn from_config(cfg: &PaymentsConfig) -> anyhow::Result<Self> {
        let var = |name: &str| std::env::var(name).with_context(|| format!("set {name} for facilitator auth"));
        Ok(match cfg.facilitator_auth {
            FacilitatorAuthKind::None => Self::None,
            FacilitatorAuthKind::Static => {
                let name = cfg
                    .facilitator_auth_env
                    .as_deref()
                    .context("facilitator_auth = \"static\" needs facilitator_auth_env")?;
                Self::Static(var(name)?)
            }
            FacilitatorAuthKind::Cdp => {
                Self::Cdp(Box::new(CdpSigner::new(&var(&cfg.cdp_key_id_env)?, &var(&cfg.cdp_key_secret_env)?)?))
            }
        })
    }

    fn header(&self, url: &reqwest::Url) -> Option<String> {
        match self {
            Self::None => None,
            Self::Static(value) => Some(value.clone()),
            Self::Cdp(signer) => Some(signer.authorization("POST", url)),
        }
    }
}

pub struct HttpFacilitator {
    http: reqwest::Client,
    base_url: String,
    auth: FacilitatorAuth,
}

impl HttpFacilitator {
    pub fn new(base_url: &str, auth: FacilitatorAuth, timeout: Duration) -> anyhow::Result<Self> {
        let base_url = base_url.trim_end_matches('/').to_string();
        reqwest::Url::parse(&base_url).context("payments.facilitator_url")?;
        Ok(Self { http: reqwest::Client::builder().timeout(timeout).build()?, base_url, auth })
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        payload: &PaymentPayload,
        req: &PaymentRequirements,
    ) -> Result<T, FacilitatorUnavailable> {
        let body =
            FacilitatorRequest { x402_version: X402_VERSION, payment_payload: payload, payment_requirements: req };
        let url = reqwest::Url::parse(&format!("{}/{path}", self.base_url))
            .map_err(|e| FacilitatorUnavailable(e.to_string()))?;
        let mut request = self.http.post(url.clone()).json(&body);
        if let Some(auth) = self.auth.header(&url) {
            request = request.header(reqwest::header::AUTHORIZATION, auth);
        }
        let response = request.send().await.map_err(|e| FacilitatorUnavailable(e.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(FacilitatorUnavailable(format!("{path} rejected our credentials ({status})")));
        }
        // Facilitators answer a rejected payment with a verdict body, sometimes under a 4xx or 5xx
        // (CDP reports `settlement_pending` as a 500).
        let text = response.text().await.map_err(|e| FacilitatorUnavailable(e.to_string()))?;
        serde_json::from_str(&text).map_err(|_| {
            FacilitatorUnavailable(format!("{path} answered {status}: {}", text.chars().take(200).collect::<String>()))
        })
    }
}

#[async_trait]
impl Facilitator for HttpFacilitator {
    async fn verify(
        &self,
        payload: &PaymentPayload,
        req: &PaymentRequirements,
    ) -> Result<VerifyResponse, FacilitatorUnavailable> {
        self.call("verify", payload, req).await
    }

    async fn settle(
        &self,
        payload: &PaymentPayload,
        req: &PaymentRequirements,
    ) -> Result<SettleResponse, FacilitatorUnavailable> {
        self.call("settle", payload, req).await
    }
}
