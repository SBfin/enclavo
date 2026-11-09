use std::{collections::HashSet, path::Path};

use anyhow::{Context, bail, ensure};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub pricing: PricingConfig,
    #[serde(default)]
    pub router: RouterConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
    #[serde(default)]
    pub batch: BatchConfig,
    pub payments: PaymentsConfig,
    pub providers: Vec<ProviderConfig>,
    pub routes: Vec<RouteConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default)]
    pub mode: RuntimeMode,
    pub listen: String,
    /// Base URL clients use to reach the gateway; it goes into x402 resource descriptions.
    pub public_url: String,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// Header a trusted proxy sets to the client's address (e.g. `fly-client-ip`), used for
    /// per-IP rate limits. Unset: the TCP peer address. Never name a header clients can set.
    #[serde(default)]
    pub client_ip_header: Option<String>,
    /// Separate address for `/metrics`, kept off the public listener (e.g. `0.0.0.0:9091`).
    #[serde(default)]
    pub metrics_listen: Option<String>,
    /// On shutdown, how long to wait for open streams and running batch items to finish.
    #[serde(default = "default_drain_timeout")]
    pub drain_timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    #[default]
    Live,
    Test,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// Literal URL; `url_env` wins when both are set and the variable exists.
    pub url: Option<String>,
    #[serde(default = "default_database_url_env")]
    pub url_env: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

impl DatabaseConfig {
    pub fn resolve_url(&self) -> anyhow::Result<String> {
        if let Ok(url) = std::env::var(&self.url_env) {
            return Ok(url);
        }
        self.url.clone().with_context(|| format!("set database.url or the {} environment variable", self.url_env))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PricingConfig {
    /// Margin added to every route price, in basis points.
    pub markup_bps: u32,
    /// Completion budget used for the payment ceiling when a request sets no max_tokens.
    pub default_max_tokens: u64,
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self { markup_bps: 0, default_max_tokens: 4096 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RouterConfig {
    pub max_attempts: usize,
    pub first_byte_timeout_secs: u64,
    pub failure_threshold: u32,
    pub cooldown_secs: u64,
    pub max_cooldown_secs: u64,
    /// Weight of the newest sample in the latency moving average.
    pub ewma_alpha: f64,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            first_byte_timeout_secs: 30,
            failure_threshold: 3,
            cooldown_secs: 10,
            max_cooldown_secs: 300,
            ewma_alpha: 0.2,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LimitsConfig {
    pub per_key_rps: u32,
    pub per_key_burst: u32,
    pub per_ip_rps: u32,
    pub per_ip_burst: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self { per_key_rps: 20, per_key_burst: 40, per_ip_rps: 10, per_ip_burst: 20 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BatchConfig {
    pub enabled: bool,
    pub discount_bps: u32,
    pub max_concurrency: usize,
    /// Batch work pauses while this many live requests are in flight.
    pub live_busy_threshold: usize,
    pub poll_interval_ms: u64,
    pub max_item_attempts: i32,
    pub completion_window_secs: i64,
    pub max_file_bytes: usize,
    pub max_items: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            discount_bps: 3000,
            max_concurrency: 8,
            live_busy_threshold: 64,
            poll_interval_ms: 500,
            max_item_attempts: 3,
            completion_window_secs: 86_400,
            max_file_bytes: 50 * 1024 * 1024,
            max_items: 50_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentsConfig {
    pub facilitator_url: String,
    /// Environment variable holding a full `Authorization` header value for the facilitator.
    pub facilitator_auth_env: Option<String>,
    /// How to authenticate to the facilitator: `none`, `static` (the header in
    /// `facilitator_auth_env`) or `cdp` (a signed JWT per call from a Coinbase CDP API key).
    #[serde(default)]
    pub facilitator_auth: FacilitatorAuthKind,
    #[serde(default = "default_cdp_key_id_env")]
    pub cdp_key_id_env: String,
    #[serde(default = "default_cdp_key_secret_env")]
    pub cdp_key_secret_env: String,
    #[serde(default = "default_facilitator_timeout")]
    pub facilitator_timeout_secs: u64,
    #[serde(default = "default_service_name")]
    pub service_name: String,
    #[serde(default = "default_max_timeout")]
    pub max_timeout_secs: u64,
    #[serde(default = "default_min_topup")]
    pub min_topup_micro: u64,
    #[serde(default = "default_max_topup")]
    pub max_topup_micro: u64,
    #[serde(default = "default_settle_retries")]
    pub max_settle_attempts: i32,
    pub networks: Vec<NetworkConfig>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FacilitatorAuthKind {
    #[default]
    None,
    Static,
    Cdp,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Exact,
    Upto,
}

impl Scheme {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scheme::Exact => "exact",
            Scheme::Upto => "upto",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PaymentFlow {
    /// verify → run → settle
    Authorization,
    /// settle (deposit) → run → settle (final charge)
    Escrow,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// CAIP-2 network id, e.g. `eip155:8453` or `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp`.
    pub network: String,
    pub asset: String,
    pub pay_to: String,
    /// Token decimals; must be at least 6 so a micro-USD maps to whole atomic units.
    #[serde(default = "default_decimals")]
    pub asset_decimals: u32,
    /// Schemes this network accepts: `upto` for pay-per-request, `exact` for top-ups.
    pub schemes: Vec<Scheme>,
    #[serde(default = "default_flow")]
    pub upto_flow: PaymentFlow,
    /// Scheme-specific `extra` (e.g. EIP-712 domain name and version), sent as-is.
    #[serde(default)]
    pub extra: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub id: String,
    pub base_url: String,
    pub api_key_env: Option<String>,
    #[serde(default = "default_provider_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_provider_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    #[serde(default)]
    pub input_micro_usd_per_iu: Option<u64>,
    #[serde(default)]
    pub metadata: Value,
    pub id: String,
    /// Model family name clients can ask for, e.g. `glm-5.3-flash`.
    pub model: String,
    pub display_name: String,
    pub provider: String,
    pub upstream_model: String,
    pub attestation: String,
    #[serde(default)]
    pub attested: bool,
    /// Intelligence score; routes at equal score are interchangeable per IU.
    pub score: u32,
    /// Tokens per day that make up 1 IU on this route.
    pub tokens_per_iu: u64,
    /// Price of 1 IU in micro-USD.
    pub micro_usd_per_iu: u64,
    #[serde(default = "default_max_output")]
    pub max_output_tokens: u64,
    /// Weight of a prompt token relative to a completion token, in basis points.
    #[serde(default = "default_prompt_weight")]
    pub prompt_weight_bps: u32,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        let mut providers = HashSet::new();
        for p in &self.providers {
            ensure!(providers.insert(p.id.as_str()), "duplicate provider id {}", p.id);
            ensure!(p.max_concurrency > 0, "provider {} needs max_concurrency > 0", p.id);
        }
        let mut routes = HashSet::new();
        for r in &self.routes {
            ensure!(routes.insert(r.id.as_str()), "duplicate route id {}", r.id);
            ensure!(providers.contains(r.provider.as_str()), "route {} names unknown provider {}", r.id, r.provider);
            ensure!(r.tokens_per_iu > 0, "route {} needs tokens_per_iu > 0", r.id);
            ensure!(r.max_output_tokens > 0, "route {} needs max_output_tokens > 0", r.id);
            ensure!(!r.id.starts_with("iu:") && r.id != "enclavo/auto", "route id {} is reserved", r.id);
        }
        if self.routes.is_empty() {
            bail!("configure at least one route");
        }
        for n in &self.payments.networks {
            ensure!(!n.pay_to.contains("REPLACE"), "network {} still has a placeholder pay_to address", n.network);
            ensure!(n.asset_decimals >= 6, "network {} needs asset_decimals >= 6", n.network);
            ensure!(n.network.contains(':'), "network {} must be a CAIP-2 id", n.network);
            ensure!(!n.schemes.is_empty(), "network {} accepts no scheme", n.network);
        }
        ensure!(self.router.max_attempts > 0, "router.max_attempts must be > 0");
        ensure!(self.batch.discount_bps <= 10_000, "batch.discount_bps must be <= 10000");
        ensure!(
            (0.0..=1.0).contains(&self.router.ewma_alpha) && self.router.ewma_alpha > 0.0,
            "router.ewma_alpha must be in (0, 1]"
        );
        Ok(())
    }
}

fn default_max_body_bytes() -> usize {
    10 * 1024 * 1024
}
fn default_drain_timeout() -> u64 {
    280
}
fn default_database_url_env() -> String {
    "DATABASE_URL".into()
}
fn default_max_connections() -> u32 {
    20
}
fn default_cdp_key_id_env() -> String {
    "CDP_API_KEY_ID".into()
}
fn default_cdp_key_secret_env() -> String {
    "CDP_API_KEY_SECRET".into()
}
fn default_facilitator_timeout() -> u64 {
    30
}
fn default_service_name() -> String {
    "Enclavo".into()
}
fn default_max_timeout() -> u64 {
    300
}
fn default_min_topup() -> u64 {
    1_000_000
}
fn default_max_topup() -> u64 {
    10_000_000_000
}
fn default_settle_retries() -> i32 {
    8
}
fn default_decimals() -> u32 {
    6
}
fn default_flow() -> PaymentFlow {
    PaymentFlow::Authorization
}
fn default_provider_concurrency() -> usize {
    64
}
fn default_provider_timeout() -> u64 {
    600
}
fn default_max_output() -> u64 {
    8192
}
fn default_prompt_weight() -> u32 {
    10_000
}
