//! End-to-end flow against a local sandbox with the real token contract.
//! Build both wasm files first: `cargo near build non-reproducible-wasm` in `auction/` and `mock-usdc/`.
use near_workspaces::types::NearToken;
use near_workspaces::{Account, Contract};
use serde_json::{json, Value};

const USD: u128 = 1_000_000;

fn wasm(name: &str) -> Vec<u8> {
    let dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "../target".into());
    let path = format!("{dir}/near/{name}/{name}.wasm");
    std::fs::read(&path).unwrap_or_else(|_| panic!("{path} is missing; build the contracts first"))
}

async fn token_balance(token: &Contract, account: &str) -> anyhow::Result<u128> {
    let balance: String = token.view("ft_balance_of").args_json(json!({"account_id": account})).await?.json()?;
    Ok(balance.parse()?)
}

async fn escrow_balance(auction: &Contract, account: &Account) -> anyhow::Result<u128> {
    let balance: String = auction.view("balance_of").args_json(json!({"account_id": account.id()})).await?.json()?;
    Ok(balance.parse()?)
}

async fn bid(token: &Contract, auction: &Contract, who: &Account, units: u64, price: u128) -> anyhow::Result<bool> {
    let msg = json!({"auction_id": 0, "units": units, "max_price": price.to_string()}).to_string();
    let amount = (units as u128 * 7 * price).to_string();
    let result = who
        .call(token.id(), "ft_transfer_call")
        .args_json(json!({"receiver_id": auction.id(), "amount": amount, "msg": msg}))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    // ft_transfer_call returns the amount the receiver kept; zero means the bid was rejected and refunded.
    Ok(result.json::<String>()? == amount)
}

#[tokio::test]
async fn bids_escrow_clear_and_withdraw() -> anyhow::Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let token = worker.dev_deploy(&wasm("enclavo_mock_usdc")).await?;
    let auction = worker.dev_deploy(&wasm("enclavo_auction")).await?;
    let operator = worker.dev_create_account().await?;
    let alice = worker.dev_create_account().await?;
    let bob = worker.dev_create_account().await?;
    let carol = worker.dev_create_account().await?;

    token.call("new").transact().await?.into_result()?;
    auction
        .call("new")
        .args_json(json!({"owner": operator.id(), "token": token.id()}))
        .transact()
        .await?
        .into_result()?;
    for account in [auction.id(), operator.id(), alice.id(), bob.id(), carol.id()] {
        let amount = if account == auction.id() || account == operator.id() { 0 } else { 1_000 * USD };
        token
            .call("mint")
            .args_json(json!({"account_id": account, "amount": amount.to_string()}))
            .transact()
            .await?
            .into_result()?;
    }

    // 10 IU/day for 7 days at a $0.50 reserve, open for 12 seconds.
    operator
        .call(auction.id(), "publish_auction")
        .args_json(json!({"route_id": "z-ai/glm-5.3-flash@near-ai", "units": 10, "term_days": 7,
            "reserve_price": (USD / 2).to_string(), "tokens_per_iu": 1_000_000, "prompt_weight_bps": 1600,
            "duration_ms": 12_000}))
        .transact()
        .await?
        .into_result()?;

    assert!(bid(&token, &auction, &alice, 6, 3 * USD).await?);
    assert!(bid(&token, &auction, &bob, 6, 2 * USD).await?);
    assert!(bid(&token, &auction, &carol, 5, USD).await?);
    // A bid below the reserve is refused and the token contract returns the transfer.
    assert!(!bid(&token, &auction, &carol, 1, USD / 4).await?);
    assert_eq!(token_balance(&token, alice.id().as_str()).await?, (1_000 - 126) * USD);
    assert_eq!(token_balance(&token, carol.id().as_str()).await?, (1_000 - 35) * USD);
    assert_eq!(token_balance(&token, auction.id().as_str()).await?, (126 + 84 + 35) * USD);

    // Clearing is refused while bidding is open, then anyone can settle after the close.
    assert!(carol
        .call(auction.id(), "clear_auction")
        .args_json(json!({"auction_id": 0}))
        .transact()
        .await?
        .is_failure());
    tokio::time::sleep(std::time::Duration::from_secs(13)).await;
    worker.fast_forward(5).await?;
    carol
        .call(auction.id(), "clear_auction")
        .args_json(json!({"auction_id": 0}))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let cleared: Value = auction.view("get_auction").args_json(json!({"auction_id": 0})).await?.json()?;
    assert_eq!(cleared["status"], "cleared");
    assert_eq!(cleared["clearing_price"], (2 * USD).to_string());
    assert_eq!(cleared["filled_units"], 10);

    // Alice 6 IU and Bob 4 IU at $2; Carol is outbid. Everyone withdraws what was not spent.
    assert_eq!(escrow_balance(&auction, &alice).await?, 42 * USD);
    assert_eq!(escrow_balance(&auction, &bob).await?, 28 * USD);
    assert_eq!(escrow_balance(&auction, &carol).await?, 35 * USD);
    assert_eq!(escrow_balance(&auction, &operator).await?, 140 * USD);
    for account in [&alice, &bob, &carol, &operator] {
        account.call(auction.id(), "withdraw").args_json(json!({})).max_gas().transact().await?.into_result()?;
        assert_eq!(escrow_balance(&auction, account).await?, 0);
    }
    assert_eq!(token_balance(&token, alice.id().as_str()).await?, (1_000 - 84) * USD);
    assert_eq!(token_balance(&token, bob.id().as_str()).await?, (1_000 - 56) * USD);
    assert_eq!(token_balance(&token, carol.id().as_str()).await?, 1_000 * USD);
    assert_eq!(token_balance(&token, operator.id().as_str()).await?, 140 * USD);
    assert_eq!(token_balance(&token, auction.id().as_str()).await?, 0);

    let won: Value = auction.view("entitlements_of").args_json(json!({"account_id": bob.id()})).await?.json()?;
    assert_eq!(won.as_array().unwrap().len(), 1);
    assert_eq!(won[0]["units"], 4);
    assert_eq!(won[0]["route_id"], "z-ai/glm-5.3-flash@near-ai");
    assert_eq!(won[0]["paid"], (56 * USD).to_string());
    Ok(())
}
