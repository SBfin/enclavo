//! Local stand-in for a provider and an x402 facilitator. See `enclavo_gateway::devmock`.

use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let addr: SocketAddr = std::env::var("MOCK_LISTEN").unwrap_or_else(|_| "0.0.0.0:4000".into()).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "mock provider and facilitator listening");
    axum::serve(listener, enclavo_gateway::devmock::router()).await?;
    Ok(())
}
