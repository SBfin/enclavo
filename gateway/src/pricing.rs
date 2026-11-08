//! Integer pricing in micro-USD. Prices are per IU; a request's cost is its IU-weighted tokens.

use std::sync::Arc;

use serde_json::Value;

use crate::{catalog::Route, config::PricingConfig};

/// Tokens assumed for one media input (image, audio, video, file) when bounding prompt size.
/// Dynamic-resolution vision models can spend this much on a single image.
const MEDIA_TOKEN_BOUND: u64 = 16_384;
const MEDIA_TYPES: [&str; 9] =
    ["image_url", "input_image", "image", "input_audio", "audio", "video_url", "video", "file", "input_file"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Live,
    Batch,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Live => "live",
            Tier::Batch => "batch",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// True when the provider reported no usage and the gateway counted it itself.
    pub estimated: bool,
}

impl Usage {
    /// Reads an OpenAI `usage` object.
    pub fn from_openai(usage: &Value) -> Option<Self> {
        Some(Self {
            prompt_tokens: usage.get("prompt_tokens")?.as_u64()?,
            completion_tokens: usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
            estimated: false,
        })
    }
}

/// The most a request may cost, and the completion budget each route is held to so it can't go over.
#[derive(Debug, Clone)]
pub struct Ceiling {
    pub micro: u64,
    pub prompt_bound: u64,
    pub completion_caps: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct Pricing {
    pub markup_bps: u32,
    pub batch_discount_bps: u32,
    pub default_max_tokens: u64,
}

fn ceil_div(n: u128, d: u128) -> u128 {
    n.div_ceil(d)
}

impl Pricing {
    pub fn new(cfg: &PricingConfig, batch_discount_bps: u32) -> Self {
        Self { markup_bps: cfg.markup_bps, batch_discount_bps, default_max_tokens: cfg.default_max_tokens }
    }

    /// Cost of `usage` on `route`, rounded up to the next micro-USD.
    pub fn cost(&self, route: &Route, usage: Usage, tier: Tier) -> u64 {
        // Scale everything by 10_000 (prompt weight bps) and divide once at the end.
        let numerator = (|| {
            let input = match route.input_micro_usd_per_iu {
                Some(price) => (usage.prompt_tokens as u128).checked_mul(price as u128)?.checked_mul(10_000)?,
                None => (usage.prompt_tokens as u128)
                    .checked_mul(route.prompt_weight_bps as u128)?
                    .checked_mul(route.micro_usd_per_iu as u128)?,
            };
            let output =
                (usage.completion_tokens as u128).checked_mul(10_000)?.checked_mul(route.micro_usd_per_iu as u128)?;
            input.checked_add(output)?.checked_mul(10_000 + self.markup_bps as u128)
        })();
        let Some(mut numerator) = numerator else {
            return u64::MAX;
        };
        let mut denominator = route.tokens_per_iu as u128 * 10_000 * 10_000;
        if tier == Tier::Batch {
            let Some(discounted) = numerator.checked_mul(10_000 - self.batch_discount_bps as u128) else {
                return u64::MAX;
            };
            numerator = discounted;
            denominator *= 10_000;
        }
        u64::try_from(ceil_div(numerator, denominator)).unwrap_or(u64::MAX)
    }

    /// Completion tokens a route may produce for this request.
    pub fn completion_cap(&self, route: &Route, body: &Value) -> u64 {
        let requested = body
            .get("max_completion_tokens")
            .or_else(|| body.get("max_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(self.default_max_tokens);
        requested.clamp(1, route.max_output_tokens)
    }

    /// Worst-case cost over every candidate route, so any fallback stays inside what was paid.
    pub fn ceiling(&self, routes: &[Arc<Route>], body: &Value, tier: Tier) -> Ceiling {
        let prompt_bound = prompt_token_bound(body);
        let choices = body.get("n").and_then(Value::as_u64).unwrap_or(1).max(1);
        let mut micro = 0;
        let mut completion_caps = Vec::with_capacity(routes.len());
        for route in routes {
            let cap = self.completion_cap(route, body);
            completion_caps.push(cap);
            let usage =
                Usage { prompt_tokens: prompt_bound, completion_tokens: cap.saturating_mul(choices), estimated: false };
            micro = micro.max(self.cost(route, usage, tier));
        }
        Ceiling { micro, prompt_bound, completion_caps }
    }
}

/// Upper bound on prompt tokens. Each BPE token covers at least one byte, so the JSON size of
/// the whole body bounds every text token a provider could read from it, and the punctuation
/// covers chat-template overhead. Media parts are bounded separately.
pub fn prompt_token_bound(body: &Value) -> u64 {
    let bytes = serde_json::to_vec(body).map(|b| b.len() as u64).unwrap_or(0);
    (16 + bytes).saturating_add(count_media(body).saturating_mul(MEDIA_TOKEN_BOUND))
}

fn count_media(v: &Value) -> u64 {
    match v {
        Value::Array(items) => items.iter().map(count_media).sum(),
        Value::Object(map) => {
            let own = map.get("type").and_then(Value::as_str).is_some_and(|t| MEDIA_TYPES.contains(&t)) as u64;
            own + map.values().map(count_media).sum::<u64>()
        }
        _ => 0,
    }
}

/// Rough count for text the gateway saw itself, used only when a provider reports no usage.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::catalog::Provider;

    fn route(price: u64, per_iu: u64, max_out: u64) -> Arc<Route> {
        Arc::new(Route {
            input_micro_usd_per_iu: None,
            metadata: serde_json::Value::Null,
            id: "r".into(),
            model: "m".into(),
            display_name: "m".into(),
            provider: Arc::new(Provider {
                id: "p".into(),
                base_url: String::new(),
                api_key: None,
                max_concurrency: 1,
                timeout: Duration::from_secs(1),
            }),
            upstream_model: "m".into(),
            attestation: String::new(),
            attested: true,
            score: 42,
            tokens_per_iu: per_iu,
            micro_usd_per_iu: price,
            max_output_tokens: max_out,
            prompt_weight_bps: 10_000,
        })
    }

    fn pricing(markup: u32) -> Pricing {
        Pricing { markup_bps: markup, batch_discount_bps: 3000, default_max_tokens: 100 }
    }

    #[test]
    fn cost_rounds_up_and_applies_markup_and_discount() {
        let r = route(185_000, 1_000_000, 1000);
        let usage = Usage { prompt_tokens: 1_000_000, completion_tokens: 0, estimated: false };
        assert_eq!(pricing(0).cost(&r, usage, Tier::Live), 185_000);
        assert_eq!(pricing(1000).cost(&r, usage, Tier::Live), 203_500);
        assert_eq!(pricing(0).cost(&r, usage, Tier::Batch), 129_500);
        let one = Usage { prompt_tokens: 0, completion_tokens: 1, estimated: false };
        assert_eq!(pricing(0).cost(&r, one, Tier::Live), 1, "a single token still costs a micro-USD");
        assert_eq!(pricing(0).cost(&r, Usage::default(), Tier::Live), 0);
    }

    #[test]
    fn separate_input_prices_are_exact_instead_of_rounded_to_a_ratio() {
        let mut r = route(1_100_000, 1_000_000, 8192);
        Arc::get_mut(&mut r).unwrap().input_micro_usd_per_iu = Some(170_000);
        let usage = Usage { prompt_tokens: 1_000_000, completion_tokens: 0, estimated: false };
        assert_eq!(pricing(0).cost(&r, usage, Tier::Live), 170_000);
    }

    #[test]
    fn ceiling_covers_most_expensive_candidate_and_clamps_output() {
        let cheap = route(185_000, 1_000_000, 50);
        let dear = route(978_000, 455_229, 1000);
        let body = json!({ "messages": [{ "role": "user", "content": "hello" }], "max_tokens": 200 });
        let p = pricing(0);
        let c = p.ceiling(&[cheap.clone(), dear.clone()], &body, Tier::Live);
        assert_eq!(c.completion_caps, vec![50, 200]);
        let worst = Usage { prompt_tokens: c.prompt_bound, completion_tokens: 200, estimated: false };
        assert_eq!(c.micro, p.cost(&dear, worst, Tier::Live));
        assert!(c.prompt_bound >= "hello".len() as u64);
    }

    #[test]
    fn media_and_extra_fields_raise_the_prompt_bound() {
        let text = json!({ "messages": [{ "role": "user", "content": "hi" }] });
        let image = json!({ "messages": [{ "role": "user", "content": [
            { "type": "image_url", "image_url": { "url": "https://x/y.png" } }
        ] }] });
        assert!(prompt_token_bound(&image) > prompt_token_bound(&text) + MEDIA_TOKEN_BOUND - 1);
        let docs = json!({ "messages": [{ "role": "user", "content": "hi" }], "documents": ["x".repeat(5000)] });
        assert!(prompt_token_bound(&docs) > 5000, "unknown fields forwarded upstream still count");
    }

    #[test]
    fn huge_n_cannot_wrap_the_ceiling() {
        let r = route(185_000, 1_000_000, 8192);
        let body = json!({ "messages": [], "max_tokens": 8192, "n": 1u64 << 51 });
        assert!(pricing(0).ceiling(&[r], &body, Tier::Live).micro > 1_000_000);
    }
}
