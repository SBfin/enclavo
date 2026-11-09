//! Live NEAR AI setup. Discover the selected model and its actual token prices before serving.

use anyhow::{Context, ensure};
use serde_json::Value;

use crate::config::{Config, ProviderConfig, RouteConfig};

pub const BASE_URL: &str = "https://cloud-api.near.ai/v1";
pub const DEFAULT_MODEL: &str = "z-ai/glm-5.3-flash";

pub async fn load() -> anyhow::Result<Config> {
    let key = std::env::var("NEAR_AI_API_KEY").context(
        "Set NEAR_AI_API_KEY in .env to run live inference; mocks are only available through compose.test.yaml",
    )?;
    ensure!(!key.trim().is_empty(), "NEAR_AI_API_KEY must not be empty");
    let model =
        std::env::var("NEAR_AI_MODEL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_MODEL.into());
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
    let catalog: Value = http
        .get(format!("{BASE_URL}/model/list"))
        .send()
        .await?
        .error_for_status()
        .context("fetching the NEAR AI model catalog")?
        .json()
        .await?;
    let selected = std::env::var("NEAR_AI_MODELS")
        .ok()
        .map(|s| s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect::<Vec<_>>());
    config_all_from_catalog(&catalog, &model, selected.as_deref())
}

pub fn config_from_catalog(catalog: &Value, model_id: &str) -> anyhow::Result<Config> {
    let models = catalog["models"].as_array().context("NEAR AI returned no model catalog")?;
    let model = models
        .iter()
        .find(|m| m["modelId"] == model_id)
        .with_context(|| format!("NEAR_AI_MODEL {model_id} is absent from the current NEAR AI catalog"))?;
    let meta = &model["metadata"];
    ensure!(meta["isReady"] != false, "NEAR AI model {model_id} is not ready");
    ensure!(
        meta["verifiable"] == true && meta["attestationSupported"] == true,
        "NEAR AI model {model_id} does not advertise hardware attestation support"
    );
    ensure!(
        meta["architecture"]["outputModalities"].as_array().is_some_and(|a| a.iter().any(|v| v == "text")),
        "NEAR_AI_MODEL must be a text generation model"
    );
    ensure!(
        meta["architecture"]["inputModalities"].as_array().is_some_and(|a| a.iter().any(|v| v == "text"))
            && meta["supportedFeatures"].as_array().is_some_and(|a| !a.is_empty())
            && model["textPricing"].is_null(),
        "Selected model is not a supported flat-priced chat model"
    );
    let input = price_per_million(&model["inputCostPerToken"])?;
    let output = price_per_million(&model["outputCostPerToken"])?;
    ensure!(output > 0, "NEAR AI output token price must be positive");
    let weight = (input as u128 * 10_000).div_ceil(output as u128);
    let mut cfg: Config = toml::from_str(include_str!("../deploy/near-ai.toml"))?;
    cfg.providers = vec![ProviderConfig {
        id: "near-ai".into(),
        base_url: BASE_URL.into(),
        api_key_env: Some("NEAR_AI_API_KEY".into()),
        max_concurrency: 16,
        timeout_secs: 180,
    }];
    cfg.routes = vec![RouteConfig {
        input_micro_usd_per_iu: Some(input),
        id: format!("{model_id}@near-ai"),
        model: model_id.into(),
        display_name: meta["modelDisplayName"].as_str().unwrap_or(model_id).into(),
        provider: "near-ai".into(),
        upstream_model: model_id.into(),
        // A provider capability is not a cryptographically verified proof.
        attestation: "Provider-reported TEE; proof not verified".into(),
        attested: false,
        score: 0,
        tokens_per_iu: 1_000_000,
        micro_usd_per_iu: output,
        max_output_tokens: meta["maxOutputLength"].as_u64().unwrap_or(8192).min(8192),
        prompt_weight_bps: weight.try_into().context("NEAR AI input/output price ratio is too large")?,
        metadata: meta.clone(),
    }];
    if let Ok(url) = std::env::var("ENCLAVO_PUBLIC_URL") {
        cfg.server.public_url = url;
    }
    cfg.validate()?;
    tracing::info!(
        model = model_id,
        input_micro_per_million = input,
        output_micro_per_million = output,
        "live NEAR AI model discovered; attestation proof verification is not enabled"
    );
    Ok(cfg)
}

/// Load all supported confidential chat models; an optional allowlist limits exposure explicitly.
pub fn config_all_from_catalog(
    catalog: &Value,
    preferred: &str,
    selected: Option<&[String]>,
) -> anyhow::Result<Config> {
    let models = catalog["models"].as_array().context("NEAR AI returned no model catalog")?;
    let ids: Vec<String> = match selected {
        Some(ids) => {
            ensure!(!ids.is_empty(), "NEAR_AI_MODELS cannot be empty");
            ids.to_vec()
        }
        None => models
            .iter()
            .filter(|m| {
                let meta = &m["metadata"];
                meta["isReady"] != false
                    && meta["verifiable"] == true
                    && meta["attestationSupported"] == true
                    && meta["architecture"]["inputModalities"].as_array().is_some_and(|a| a.iter().any(|v| v == "text"))
                    && meta["architecture"]["outputModalities"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|v| v == "text"))
                    && meta["supportedFeatures"].as_array().is_some_and(|a| !a.is_empty())
                    && m["textPricing"].is_null()
            })
            .filter_map(|m| m["modelId"].as_str().map(str::to_owned))
            .collect(),
    };
    let mut result: Option<Config> = None;
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        match config_from_catalog(catalog, &id) {
            Ok(cfg) => match &mut result {
                Some(result) => result.routes.extend(cfg.routes),
                None => result = Some(cfg),
            },
            Err(error) if selected.is_none() => {
                tracing::warn!(model = id, %error, "skipping unsupported NEAR AI model")
            }
            Err(error) => return Err(error),
        }
    }
    let mut cfg = result.context("NEAR AI has no supported ready confidential chat models")?;
    cfg.routes
        .sort_by(|a, b| (a.model != preferred).cmp(&(b.model != preferred)).then(a.display_name.cmp(&b.display_name)));
    cfg.validate()?;
    Ok(cfg)
}

fn price_per_million(price: &Value) -> anyhow::Result<u64> {
    ensure!(price["currency"] == "USD", "NEAR AI token prices must be in USD");
    let amount = price["amount"].as_u64().context("NEAR AI token price has no integer amount")? as u128;
    let scale: u32 = price["scale"].as_u64().context("NEAR AI token price has no scale")?.try_into()?;
    let denominator = 10u128.checked_pow(scale).context("NEAR AI token price scale is too large")?;
    // USD/token -> micro-USD/million tokens, with conservative integer rounding.
    Ok((amount * 1_000_000_000_000u128).div_ceil(denominator).try_into()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> Value {
        json!({"models": [{"modelId": DEFAULT_MODEL,
            "inputCostPerToken": {"amount": 80, "scale": 9, "currency": "USD"},
            "outputCostPerToken": {"amount": 500, "scale": 9, "currency": "USD"},
            "metadata": {"isReady": true, "verifiable": true, "attestationSupported": true,
                "architecture": {"inputModalities": ["text"], "outputModalities": ["text"]}, "supportedFeatures": ["tools"], "maxOutputLength": 65536}
        }]})
    }

    #[test]
    fn discovers_real_prices_without_inventing_verification_or_benchmarks() {
        let cfg = config_from_catalog(&catalog(), DEFAULT_MODEL).unwrap();
        let route = &cfg.routes[0];
        assert_eq!(route.upstream_model, DEFAULT_MODEL);
        assert_eq!(route.micro_usd_per_iu, 500_000);
        assert_eq!(route.prompt_weight_bps, 1600);
        assert_eq!(route.max_output_tokens, 8192);
        assert!(!route.attested);
        assert_eq!(route.score, 0);
        assert!(cfg.payments.networks.is_empty());
        assert_eq!(cfg.batch.discount_bps, 0);
    }

    #[test]
    fn rejects_missing_unready_external_and_non_text_models() {
        assert!(config_from_catalog(&catalog(), "invented-model").is_err());
        for field in ["isReady", "verifiable", "attestationSupported"] {
            let mut data = catalog();
            data["models"][0]["metadata"][field] = false.into();
            assert!(config_from_catalog(&data, DEFAULT_MODEL).is_err(), "{field}");
        }
        let mut data = catalog();
        data["models"][0]["metadata"]["architecture"]["outputModalities"] = json!(["image"]);
        assert!(config_from_catalog(&data, DEFAULT_MODEL).is_err());
    }

    #[test]
    fn discovers_multiple_chat_models_and_excludes_other_tasks_and_unready_models() {
        let mut data = catalog();
        let mut second = data["models"][0].clone();
        second["modelId"] = "Qwen/Example".into();
        second["metadata"]["modelDisplayName"] = "Qwen Example".into();
        let mut embedding = second.clone();
        embedding["modelId"] = "embedding".into();
        embedding["metadata"]["architecture"]["outputModalities"] = json!(["embedding"]);
        let mut reranker = second.clone();
        reranker["modelId"] = "reranker".into();
        reranker["metadata"]["supportedFeatures"] = Value::Null;
        let mut unready = second.clone();
        unready["modelId"] = "unready".into();
        unready["metadata"]["isReady"] = false.into();
        let mut external = second.clone();
        external["modelId"] = "external".into();
        external["metadata"]["verifiable"] = false.into();
        data["models"].as_array_mut().unwrap().extend([second, embedding, reranker, unready, external]);
        let cfg = config_all_from_catalog(&data, DEFAULT_MODEL, None).unwrap();
        assert_eq!(cfg.routes.len(), 2);
        assert_eq!(cfg.routes[0].model, DEFAULT_MODEL);
        assert_eq!(cfg.routes[1].metadata["modelDisplayName"], "Qwen Example");
        assert_eq!(
            config_all_from_catalog(&data, DEFAULT_MODEL, Some(&["Qwen/Example".into()])).unwrap().routes.len(),
            1
        );
        assert!(config_all_from_catalog(&data, DEFAULT_MODEL, Some(&[])).is_err());
    }

    #[test]
    fn price_conversion_rounds_up_and_rejects_invalid_currency_or_scale() {
        assert_eq!(price_per_million(&json!({"amount": 1, "scale": 13, "currency": "USD"})).unwrap(), 1);
        assert!(price_per_million(&json!({"amount": 1, "scale": 100, "currency": "USD"})).is_err());
        assert!(price_per_million(&json!({"amount": 1, "scale": 9, "currency": "EUR"})).is_err());
    }
}
