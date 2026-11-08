//! Local operator CLI. Requires database access; no public account-minting endpoint exists.

use anyhow::{Context, ensure};
use enclavo_gateway::{MIGRATOR, api::account::parse_usd, ledger};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 3 && args[0] == "create-account" && args[1] == "--credit-usd",
        "Usage: enclavo-admin create-account --credit-usd AMOUNT (operator-funded credit; not a blockchain payment)"
    );
    let amount = parse_usd(&json!(args[2])).context("credit must be a USD decimal with at most six decimal places")?;
    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(&std::env::var("DATABASE_URL").context("DATABASE_URL is required")?)
        .await?;
    MIGRATOR.run(&db).await?;
    let (id, key) = ledger::grant_operator_credit(&db, amount).await?;
    println!(
        "{}",
        json!({"account_id": id, "api_key": key, "credited_micro_usd": amount,
        "funding_source": "operator", "note": "The operator pays the NEAR AI invoice. This is not a settled blockchain payment. Store the key now; it will not be shown again."})
    );
    db.close().await;
    Ok(())
}
