use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context;
use enclavo_gateway::{
    AppState, MIGRATOR, app,
    catalog::Catalog,
    config::Config,
    drain, metrics_app, spawn_background,
    x402::facilitator::{FacilitatorAuth, HttpFacilitator},
};
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if std::env::var("LOG_FORMAT").as_deref() == Ok("json") {
        fmt().with_env_filter(filter).json().init();
    } else {
        fmt().with_env_filter(filter).init();
    }

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let check_only = args.first().is_some_and(|a| a == "--check");
    if check_only {
        args.remove(0);
    }
    let path = args
        .first()
        .cloned()
        .or_else(|| std::env::var("ENCLAVO_CONFIG").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("enclavo.toml"));
    let cfg = Config::load(&path)?;
    if check_only {
        // Validates the file and the catalog without touching the database or the network.
        let catalog = Catalog::from_config(&cfg)?;
        cfg.server.listen.parse::<SocketAddr>().context("server.listen")?;
        if let Some(addr) = &cfg.server.metrics_listen {
            addr.parse::<SocketAddr>().context("server.metrics_listen")?;
        }
        println!(
            "{}: ok ({} providers, {} routes, {} payment networks)",
            path.display(),
            catalog.providers.len(),
            catalog.routes.len(),
            cfg.payments.networks.len()
        );
        return Ok(());
    }

    let db = PgPoolOptions::new()
        .max_connections(cfg.database.max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&cfg.database.resolve_url()?)
        .await
        .context("connecting to Postgres")?;
    MIGRATOR.run(&db).await.context("running migrations")?;

    let facilitator = HttpFacilitator::new(
        &cfg.payments.facilitator_url,
        FacilitatorAuth::from_config(&cfg.payments)?,
        Duration::from_secs(cfg.payments.facilitator_timeout_secs),
    )?;
    let metrics = PrometheusBuilder::new().install_recorder().context("installing the metrics recorder")?;

    let listen: SocketAddr = cfg.server.listen.parse().context("server.listen")?;
    let metrics_listen = cfg.server.metrics_listen.as_deref().map(str::parse::<SocketAddr>).transpose()?;
    let drain_timeout = Duration::from_secs(cfg.server.drain_timeout_secs);
    let state = AppState::new(cfg, db, Arc::new(facilitator), Some(metrics))?;
    let background = spawn_background(state.clone());

    if let Some(addr) = metrics_listen {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!(%addr, "metrics listening");
        let router = metrics_app(state.clone());
        tokio::spawn(async move { axum::serve(listener, router).await });
    }

    let signal_state = state.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutdown started; draining open requests");
        signal_state.shutdown.cancel();
    });

    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, routes = state.catalog.routes.len(), "enclavo gateway listening");
    let server = axum::serve(listener, app(state.clone()).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(state.shutdown.clone().cancelled_owned());

    // Open streams keep their connections, so the server returns once they finish or time runs out.
    let started = tokio::time::Instant::now();
    tokio::select! {
        result = server => result?,
        _ = async { state.shutdown.cancelled().await; tokio::time::sleep(drain_timeout).await } => {
            tracing::warn!("drain timeout reached with connections still open");
        }
    }
    let left = drain_timeout.saturating_sub(started.elapsed()).max(Duration::from_secs(1));
    if !drain(&state, left).await {
        tracing::warn!("exiting with work still running; holds and settlements recover through the sweepers");
    }
    for task in background {
        task.abort();
    }
    state.db.close().await;
    tracing::info!("stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}
