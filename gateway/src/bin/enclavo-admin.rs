//! Operator-only account funding and publication of real capacity inventory.
use anyhow::{Context, ensure};
use enclavo_gateway::{MIGRATOR, api::account::parse_usd, catalog::Catalog, config::Config, ledger, market};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        !args.is_empty(),
        "Commands: create-account --credit-usd AMOUNT; publish-auction --model MODEL --units N --days 7|30|90 --min-price-usd PRICE --duration-seconds SECONDS"
    );
    let db = PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("DATABASE_URL").context("DATABASE_URL is required")?)
        .await?;
    MIGRATOR.run(&db).await?;
    match args[0].as_str() {
        "create-account" => {
            ensure!(
                args.len() == 3 && args[1] == "--credit-usd",
                "Usage: enclavo-admin create-account --credit-usd AMOUNT"
            );
            let amount = parse_usd(&json!(args[2])).context("Use a USD amount with at most six decimals")?;
            let (id, key) = ledger::grant_operator_credit(&db, amount).await?;
            println!(
                "{}",
                json!({"account_id":id,"api_key":key,"credited_micro_usd":amount,"funding_source":"operator","note":"The operator pays the provider invoice. This is not a blockchain payment. Store the key now."})
            );
        }
        "credit-account" => {
            ensure!(
                args.len() == 5 && args[1] == "--account-id" && args[3] == "--credit-usd",
                "Usage: enclavo-admin credit-account --account-id UUID --credit-usd AMOUNT"
            );
            let account = args[2].parse()?;
            let amount = parse_usd(&json!(args[4])).context("Invalid credit amount")?;
            ledger::grant_existing_credit(&db, account, amount).await?;
            println!("{}", json!({"account_id":account,"credited_micro_usd":amount,"funding_source":"operator"}));
        }
        "publish-auction" => {
            ensure!(
                args.len() == 11,
                "Usage: enclavo-admin publish-auction --model MODEL --units N --days 7|30|90 --min-price-usd PRICE --duration-seconds SECONDS"
            );
            let options = args[1..]
                .chunks_exact(2)
                .map(|pair| (pair[0].as_str(), pair[1].as_str()))
                .collect::<std::collections::HashMap<_, _>>();
            let get = |key: &str| options.get(key).copied().with_context(|| format!("Missing {key}"));
            let preset = std::env::var("ENCLAVO_CONFIG").unwrap_or_else(|_| "near-ai".into());
            let config = if preset == "near-ai" {
                enclavo_gateway::near::load().await?
            } else {
                Config::load(std::path::Path::new(&preset))?
            };
            let catalog = Catalog::from_config(&config)?;
            let model = get("--model")?;
            let route = catalog
                .routes
                .iter()
                .find(|route| route.id == model || route.model == model)
                .context("Model is absent from the configured live catalog")?;
            let price = parse_usd(&json!(get("--min-price-usd")?)).context("Invalid minimum price")?;
            let id = market::publish(
                &db,
                route,
                get("--units")?.parse()?,
                get("--days")?.parse()?,
                price.try_into()?,
                get("--duration-seconds")?.parse()?,
            )
            .await?;
            println!(
                "{}",
                json!({"auction_id":id,"route_id":route.id,"note":"Published operator-backed capacity. Winning bids create enforceable daily token entitlements."})
            );
        }
        _ => anyhow::bail!("Unknown operator command"),
    }
    db.close().await;
    Ok(())
}
