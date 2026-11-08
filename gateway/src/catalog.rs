use std::{collections::HashMap, sync::Arc, time::Duration};

use serde::Deserialize;

use crate::{config::Config, error::ApiError};

/// Model id that selects every route.
pub const AUTO_MODEL: &str = "enclavo/auto";

#[derive(Debug)]
pub struct Provider {
    pub id: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub max_concurrency: usize,
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct Route {
    pub id: String,
    pub model: String,
    pub display_name: String,
    pub provider: Arc<Provider>,
    pub upstream_model: String,
    pub attestation: String,
    pub attested: bool,
    pub score: u32,
    pub tokens_per_iu: u64,
    pub micro_usd_per_iu: u64,
    pub max_output_tokens: u64,
    pub prompt_weight_bps: u32,
}

#[derive(Debug)]
pub struct Catalog {
    pub providers: HashMap<String, Arc<Provider>>,
    pub routes: Vec<Arc<Route>>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    #[default]
    Price,
    Latency,
}

/// Per-request routing preferences, from the optional `provider` object in the request body.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub sort: Sort,
    /// Provider or route ids to try first, in this order.
    pub order: Vec<String>,
    /// Only these provider or route ids.
    pub only: Vec<String>,
    /// Never these provider or route ids.
    pub ignore: Vec<String>,
    pub allow_fallbacks: bool,
    /// Highest acceptable price in USD per IU.
    pub max_price: Option<f64>,
    pub require_attestation: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            sort: Sort::Price,
            order: Vec::new(),
            only: Vec::new(),
            ignore: Vec::new(),
            allow_fallbacks: true,
            max_price: None,
            require_attestation: false,
        }
    }
}

impl Catalog {
    pub fn from_config(cfg: &Config) -> anyhow::Result<Self> {
        let mut providers = HashMap::new();
        for p in &cfg.providers {
            let api_key = match &p.api_key_env {
                Some(var) => match std::env::var(var) {
                    Ok(key) => Some(key),
                    Err(_) => {
                        tracing::warn!(provider = %p.id, env = %var, "provider API key variable is not set");
                        None
                    }
                },
                None => None,
            };
            providers.insert(
                p.id.clone(),
                Arc::new(Provider {
                    id: p.id.clone(),
                    base_url: p.base_url.trim_end_matches('/').to_string(),
                    api_key,
                    max_concurrency: p.max_concurrency,
                    timeout: Duration::from_secs(p.timeout_secs),
                }),
            );
        }
        let routes = cfg
            .routes
            .iter()
            .map(|r| {
                Arc::new(Route {
                    id: r.id.clone(),
                    model: r.model.clone(),
                    display_name: r.display_name.clone(),
                    provider: providers[&r.provider].clone(),
                    upstream_model: r.upstream_model.clone(),
                    attestation: r.attestation.clone(),
                    attested: r.attested,
                    score: r.score,
                    tokens_per_iu: r.tokens_per_iu,
                    micro_usd_per_iu: r.micro_usd_per_iu,
                    max_output_tokens: r.max_output_tokens,
                    prompt_weight_bps: r.prompt_weight_bps,
                })
            })
            .collect();
        Ok(Self { providers, routes })
    }

    /// Routes a `model` value can be served by, before preferences and health are applied.
    pub fn resolve(&self, model: &str) -> Vec<Arc<Route>> {
        if model == AUTO_MODEL {
            return self.routes.clone();
        }
        if let Some(min) = model.strip_prefix("iu:") {
            return match min.parse::<u32>() {
                Ok(min) => self.routes.iter().filter(|r| r.score >= min).cloned().collect(),
                Err(_) => Vec::new(),
            };
        }
        if let Some(route) = self.routes.iter().find(|r| r.id == model) {
            return vec![route.clone()];
        }
        self.routes.iter().filter(|r| r.model == model).cloned().collect()
    }

    /// Candidate routes for a request, filtered by preferences and in the order to try them.
    /// `latency` gives each route's observed latency for the latency sort.
    pub fn candidates(
        &self,
        model: &str,
        prefs: &Preferences,
        latency: impl Fn(&Route) -> f64,
    ) -> Result<Vec<Arc<Route>>, ApiError> {
        let matched = self.resolve(model);
        if matched.is_empty() {
            return Err(ApiError::not_found(format!("The model `{model}` does not exist.")));
        }
        let max_price_micro = prefs.max_price.map(|usd| (usd * 1_000_000.0).floor() as u64);
        let named = |r: &Route, ids: &[String]| ids.iter().any(|id| *id == r.id || *id == r.provider.id);
        let mut routes: Vec<_> = matched
            .into_iter()
            .filter(|r| prefs.only.is_empty() || named(r, &prefs.only))
            .filter(|r| !named(r, &prefs.ignore))
            .filter(|r| !prefs.require_attestation || r.attested)
            .filter(|r| max_price_micro.is_none_or(|max| r.micro_usd_per_iu <= max))
            .collect();
        if routes.is_empty() {
            return Err(ApiError::no_route(format!(
                "No route for `{model}` matches the requested provider preferences."
            )));
        }

        match prefs.sort {
            Sort::Price => routes.sort_by(|a, b| {
                a.micro_usd_per_iu.cmp(&b.micro_usd_per_iu).then_with(|| latency(a).total_cmp(&latency(b)))
            }),
            Sort::Latency => routes.sort_by(|a, b| {
                latency(a).total_cmp(&latency(b)).then_with(|| a.micro_usd_per_iu.cmp(&b.micro_usd_per_iu))
            }),
        }
        if !prefs.order.is_empty() {
            let rank =
                |r: &Route| prefs.order.iter().position(|id| *id == r.id || *id == r.provider.id).unwrap_or(usize::MAX);
            routes.sort_by_key(|r| rank(r)); // stable, keeps the sort above within each rank
        }
        if !prefs.allow_fallbacks {
            routes.truncate(1);
        }
        Ok(routes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(id: &str, model: &str, provider: &str, price: u64, score: u32, attested: bool) -> Arc<Route> {
        Arc::new(Route {
            id: id.into(),
            model: model.into(),
            display_name: model.into(),
            provider: Arc::new(Provider {
                id: provider.into(),
                base_url: String::new(),
                api_key: None,
                max_concurrency: 1,
                timeout: Duration::from_secs(1),
            }),
            upstream_model: model.into(),
            attestation: String::new(),
            attested,
            score,
            tokens_per_iu: 1_000_000,
            micro_usd_per_iu: price,
            max_output_tokens: 1000,
            prompt_weight_bps: 10_000,
        })
    }

    fn catalog() -> Catalog {
        Catalog {
            providers: HashMap::new(),
            routes: vec![
                route("glm@venice", "glm", "venice", 255_000, 42, true),
                route("glm@near", "glm", "near", 185_000, 42, true),
                route("ds@redpill", "ds", "redpill", 232_000, 45, false),
            ],
        }
    }

    fn ids(routes: &[Arc<Route>]) -> Vec<&str> {
        routes.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn resolves_route_ids_models_and_scores() {
        let c = catalog();
        assert_eq!(ids(&c.resolve("glm@near")), ["glm@near"]);
        assert_eq!(c.resolve("glm").len(), 2);
        assert_eq!(ids(&c.resolve("iu:45")), ["ds@redpill"]);
        assert_eq!(c.resolve(AUTO_MODEL).len(), 3);
        assert!(c.resolve("nope").is_empty());
    }

    #[test]
    fn sorts_by_price_and_applies_preferences() {
        let c = catalog();
        let none = |_: &Route| 0.0;
        let p = Preferences::default();
        assert_eq!(ids(&c.candidates("glm", &p, none).unwrap()), ["glm@near", "glm@venice"]);

        let p = Preferences { order: vec!["venice".into()], ..Default::default() };
        assert_eq!(ids(&c.candidates("glm", &p, none).unwrap()), ["glm@venice", "glm@near"]);

        let p = Preferences { require_attestation: true, ..Default::default() };
        assert_eq!(ids(&c.candidates(AUTO_MODEL, &p, none).unwrap()), ["glm@near", "glm@venice"]);

        let p = Preferences { max_price: Some(0.2), ..Default::default() };
        assert_eq!(ids(&c.candidates(AUTO_MODEL, &p, none).unwrap()), ["glm@near"]);

        let p = Preferences { allow_fallbacks: false, ..Default::default() };
        assert_eq!(ids(&c.candidates("glm", &p, none).unwrap()), ["glm@near"]);

        let p = Preferences { ignore: vec!["near".into(), "venice".into()], ..Default::default() };
        assert!(c.candidates("glm", &p, none).is_err());
    }

    #[test]
    fn latency_sort_uses_observed_latency() {
        let c = catalog();
        let p = Preferences { sort: Sort::Latency, ..Default::default() };
        let lat = |r: &Route| if r.id == "glm@venice" { 10.0 } else { 50.0 };
        assert_eq!(ids(&c.candidates("glm", &p, lat).unwrap()), ["glm@venice", "glm@near"]);
    }
}
