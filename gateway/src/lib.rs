pub mod api;
pub mod batch;
pub mod billing;
pub mod catalog;
pub mod config;
pub mod devmock;
pub mod error;
pub mod ledger;
pub mod limits;
pub mod pricing;
pub mod router;
pub mod upstream;
pub mod x402;

use std::{sync::Arc, time::Duration};

use axum::{
    Router as HttpRouter,
    extract::DefaultBodyLimit,
    http::HeaderName,
    middleware,
    response::Redirect,
    routing::{get, post},
};
use metrics_exporter_prometheus::PrometheusHandle;
use sqlx::PgPool;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower_http::{
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

use crate::{
    catalog::Catalog,
    config::Config,
    limits::RateLimits,
    pricing::Pricing,
    router::Router,
    upstream::Upstream,
    x402::{Payments, facilitator::Facilitator},
};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub struct AppState {
    pub cfg: Config,
    pub catalog: Catalog,
    pub router: Arc<Router>,
    pub upstream: Upstream,
    pub pricing: Pricing,
    pub payments: Payments,
    pub facilitator: Arc<dyn Facilitator>,
    pub db: PgPool,
    pub limits: RateLimits,
    pub metrics: Option<PrometheusHandle>,
    /// Work that must finish before the process exits: request handlers, stream pumps, batch items.
    pub tasks: TaskTracker,
    /// Cancelled when shutdown starts: stop taking new work, report not ready, drain.
    pub shutdown: CancellationToken,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(
        cfg: Config,
        db: PgPool,
        facilitator: Arc<dyn Facilitator>,
        metrics: Option<PrometheusHandle>,
    ) -> anyhow::Result<Shared> {
        let catalog = Catalog::from_config(&cfg)?;
        let router = Arc::new(Router::new(cfg.router.clone(), &catalog));
        Ok(Arc::new(Self {
            pricing: Pricing::new(&cfg.pricing, cfg.batch.discount_bps),
            payments: Payments::new(cfg.payments.clone(), &cfg.server.public_url),
            limits: RateLimits::new(&cfg.limits, cfg.server.client_ip_header.clone()),
            tasks: TaskTracker::new(),
            shutdown: CancellationToken::new(),
            upstream: Upstream::new()?,
            catalog,
            router,
            facilitator,
            db,
            metrics,
            cfg,
        }))
    }
}

/// Router for the private metrics listener.
pub fn metrics_app(state: Shared) -> HttpRouter {
    HttpRouter::new()
        .route("/", get(|| async { Redirect::temporary("/metrics") }))
        .route("/metrics", get(api::ops::metrics))
        .with_state(state)
}

pub fn app(state: Shared) -> HttpRouter {
    let request_id = HeaderName::from_static("x-request-id");
    let max_body = state.cfg.server.max_body_bytes;
    let max_file = state.cfg.batch.max_file_bytes + 64 * 1024;

    let files = HttpRouter::new()
        .route("/v1/files", post(api::files::upload).get(api::files::list))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(max_file));

    let api = HttpRouter::new()
        .route("/v1/models", get(api::models::list))
        .route("/v1/chat/completions", post(api::chat::completions))
        .route("/v1/credits/topup", post(api::account::top_up))
        .route("/v1/account", get(api::account::show))
        .route("/v1/files/{id}", get(api::files::retrieve))
        .route("/v1/files/{id}/content", get(api::files::content))
        .route("/v1/batches", post(api::batches::create).get(api::batches::list))
        .route("/v1/batches/{id}", get(api::batches::retrieve))
        .route("/v1/batches/{id}/cancel", post(api::batches::cancel))
        .layer(DefaultBodyLimit::max(max_body))
        .merge(files)
        .layer(middleware::from_fn_with_state(state.clone(), limits::middleware));

    let mut public = HttpRouter::new().merge(api);
    if state.cfg.server.metrics_listen.is_none() {
        public = public.route("/metrics", get(api::ops::metrics));
    }
    public
        .route("/", get(|| async { Redirect::temporary("/v1/models") }))
        .route("/healthz", get(api::ops::healthz))
        .route("/readyz", get(api::ops::readyz))
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .with_state(state)
}

/// Background work: batch scheduling, settlement retries and hold sweeping.
pub fn spawn_background(state: Shared) -> Vec<tokio::task::JoinHandle<()>> {
    let mut tasks = Vec::new();
    if state.cfg.batch.enabled {
        tasks.push(tokio::spawn(batch::run(state.clone())));
    }
    tasks.push(tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = state.shutdown.cancelled() => return,
            }
            if let Err(err) = billing::retry_pending_settlements(&state).await {
                tracing::error!(error = %err, "settlement retry pass failed");
            }
            match ledger::release_expired_holds(&state.db).await {
                Ok(0) => {}
                Ok(n) => tracing::warn!(released = n, "released expired request holds"),
                Err(err) => tracing::error!(error = %err, "hold sweep failed"),
            }
            state.limits.prune();
        }
    }));
    tasks
}

/// Waits for tracked work (request handlers, stream pumps, batch items) to finish, up to
/// `timeout`. Call after `shutdown` is cancelled. Returns whether everything finished.
pub async fn drain(state: &Shared, timeout: Duration) -> bool {
    state.shutdown.cancel();
    state.tasks.close();
    tokio::time::timeout(timeout, state.tasks.wait()).await.is_ok()
}
