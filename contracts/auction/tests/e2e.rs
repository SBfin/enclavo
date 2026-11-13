//! End-to-end flow against a local sandbox with the real token contract.
//! Build both wasm files first: `cargo near build non-reproducible-wasm` in `auction/` and `mock-usdc/`.
use near_workspaces::operations::Function;
use near_workspaces::types::{Gas, NearToken};
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

#[tokio::test]
async fn rejected_bids_refund_and_failed_withdrawals_restore() -> anyhow::Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let token = worker.dev_deploy(&wasm("enclavo_mock_usdc")).await?;
    let auction = worker.dev_deploy(&wasm("enclavo_auction")).await?;
    let operator = worker.dev_create_account().await?;
    let alice = worker.dev_create_account().await?;
    let mallory = worker.dev_create_account().await?;

    token.call("new").transact().await?.into_result()?;
    auction
        .call("new")
        .args_json(json!({"owner": operator.id(), "token": token.id()}))
        .transact()
        .await?
        .into_result()?;
    for (account, amount) in [(auction.id(), 0), (alice.id(), 100 * USD)] {
        token
            .call("mint")
            .args_json(json!({"account_id": account, "amount": amount.to_string()}))
            .transact()
            .await?
            .into_result()?;
    }
    operator
        .call(auction.id(), "publish_auction")
        .args_json(json!({"route_id": "z-ai/glm-5.3-flash@near-ai", "units": 10, "term_days": 7,
            "reserve_price": (USD / 2).to_string(), "tokens_per_iu": 1_000_000, "prompt_weight_bps": 1600,
            "duration_ms": 3_600_000}))
        .transact()
        .await?
        .into_result()?;

    // Only the operator publishes.
    assert!(mallory
        .call(auction.id(), "publish_auction")
        .args_json(json!({"route_id": "free@near-ai", "units": 1, "term_days": 7, "reserve_price": "1",
            "tokens_per_iu": 1, "prompt_weight_bps": 0, "duration_ms": 1_000}))
        .transact()
        .await?
        .is_failure());

    // A transfer with an unreadable message is returned in full.
    let kept: String = alice
        .call(token.id(), "ft_transfer_call")
        .args_json(json!({"receiver_id": auction.id(), "amount": (10 * USD).to_string(), "msg": "bid please"}))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .json()?;
    assert_eq!(kept, "0");
    assert_eq!(token_balance(&token, alice.id().as_str()).await?, 100 * USD);
    assert_eq!(escrow_balance(&auction, &alice).await?, 0);

    // A plain deposit funds a bid placed directly, and cancelling it unlocks the escrow.
    alice
        .call(token.id(), "ft_transfer_call")
        .args_json(json!({"receiver_id": auction.id(), "amount": (50 * USD).to_string(), "msg": ""}))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let index: u32 = alice
        .call(auction.id(), "place_bid")
        .args_json(json!({"auction_id": 0, "units": 5, "max_price": USD.to_string()}))
        .transact()
        .await?
        .json()?;
    assert_eq!(escrow_balance(&auction, &alice).await?, 15 * USD);
    // Nobody else can cancel it, and locked funds cannot leave.
    assert!(mallory
        .call(auction.id(), "cancel_bid")
        .args_json(json!({"auction_id": 0, "index": index}))
        .transact()
        .await?
        .is_failure());
    assert!(alice
        .call(auction.id(), "withdraw")
        .args_json(json!({"amount": (16 * USD).to_string()}))
        .max_gas()
        .transact()
        .await?
        .is_failure());
    alice
        .call(auction.id(), "cancel_bid")
        .args_json(json!({"auction_id": 0, "index": index}))
        .transact()
        .await?
        .into_result()?;
    assert_eq!(escrow_balance(&auction, &alice).await?, 50 * USD);

    // The withdrawal callback is not callable from outside.
    assert!(mallory
        .call(auction.id(), "on_withdraw")
        .args_json(json!({"account": mallory.id(), "amount": (50 * USD).to_string()}))
        .transact()
        .await?
        .is_failure());
    assert_eq!(escrow_balance(&auction, &mallory).await?, 0);

    // Alice leaves the token contract, so paying her out fails and the balance comes back.
    alice
        .call(token.id(), "storage_unregister")
        .args_json(json!({"force": true}))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;
    let sent: String = alice.call(auction.id(), "withdraw").args_json(json!({})).max_gas().transact().await?.json()?;
    assert_eq!(sent, "0");
    assert_eq!(escrow_balance(&auction, &alice).await?, 50 * USD);
    assert_eq!(token_balance(&token, auction.id().as_str()).await?, 50 * USD);

    // Once she is registered again the same withdrawal goes through.
    token.call("mint").args_json(json!({"account_id": alice.id(), "amount": "0"})).transact().await?.into_result()?;
    let sent: String = alice.call(auction.id(), "withdraw").args_json(json!({})).max_gas().transact().await?.json()?;
    assert_eq!(sent, (50 * USD).to_string());
    assert_eq!(token_balance(&token, alice.id().as_str()).await?, 50 * USD);
    assert_eq!(token_balance(&token, auction.id().as_str()).await?, 0);
    let config: Value = auction.view("get_config").await?.json()?;
    assert_eq!(config["total_liabilities"], "0");

    // A wallet authorizes a gateway key by hash and can take it back.
    let hash = "5e".repeat(32);
    alice.call(auction.id(), "authorize_key").args_json(json!({"key_hash": hash})).transact().await?.into_result()?;
    let stored: Option<String> =
        auction.view("key_hash_of").args_json(json!({"account_id": alice.id()})).await?.json()?;
    assert_eq!(stored, Some(hash));
    alice.call(auction.id(), "revoke_key").transact().await?.into_result()?;
    let stored: Option<String> =
        auction.view("key_hash_of").args_json(json!({"account_id": alice.id()})).await?.json()?;
    assert_eq!(stored, None);
    Ok(())
}

/// Worst case for settlement: the book holds every bid it can record, and a full set of distinct
/// winners clears in one transaction. A stand-in account plays the token so the bidders need no setup.
#[tokio::test]
async fn full_order_book_clears_in_one_transaction() -> anyhow::Result<()> {
    const OPEN: u64 = 100;
    const TOTAL: u64 = 400;
    const PER_TX: u64 = 10;
    let worker = near_workspaces::sandbox().await?;
    let auction = worker.dev_deploy(&wasm("enclavo_auction")).await?;
    let operator = worker.dev_create_account().await?;
    let feeder = worker.dev_create_account().await?;
    auction
        .call("new")
        .args_json(json!({"owner": operator.id(), "token": feeder.id()}))
        .transact()
        .await?
        .into_result()?;
    let duration_ms = 150_000u64;
    let started = std::time::Instant::now();
    operator
        .call(auction.id(), "publish_auction")
        .args_json(json!({"route_id": "z-ai/glm-5.3-flash@near-ai", "units": 1_000, "term_days": 7,
            "reserve_price": USD.to_string(), "tokens_per_iu": 1_000_000, "prompt_weight_bps": 1600,
            "duration_ms": duration_ms}))
        .transact()
        .await?
        .into_result()?;

    // Rising prices: once the book is full every new bid evicts the weakest one.
    let transfer = |k: u64| {
        let price = USD as u64 + k;
        Function::new("ft_on_transfer")
            .args_json(json!({"sender_id": format!("bidder-{k}.test.near"), "amount": (7 * price).to_string(),
                "msg": json!({"auction_id": 0, "units": 1, "max_price": price.to_string()}).to_string()}))
            .gas(Gas::from_tgas(30))
    };
    let mut bid_gas = Gas::from_gas(0);
    for chunk in 0..TOTAL / PER_TX {
        let mut batch = feeder.batch(auction.id());
        for k in chunk * PER_TX..(chunk + 1) * PER_TX {
            batch = batch.call(transfer(k));
        }
        let outcome = batch.transact().await?;
        bid_gas = outcome.total_gas_burnt;
        outcome.into_result()?;
    }
    println!("last batch of {PER_TX} bids into a full book: {bid_gas}");
    assert!(feeder.batch(auction.id()).call(transfer(TOTAL)).transact().await?.is_failure());

    let open: Value = auction.view("get_auction").args_json(json!({"auction_id": 0})).await?.json()?;
    assert_eq!((open["open_bids"].as_u64(), open["open_bid_units"].as_u64()), (Some(OPEN), Some(OPEN)));
    // Evicted bidders already have their escrow back.
    let evicted: String =
        auction.view("balance_of").args_json(json!({"account_id": "bidder-0.test.near"})).await?.json()?;
    assert_eq!(evicted, (7 * USD).to_string());

    let left = duration_ms.saturating_sub(started.elapsed().as_millis() as u64);
    tokio::time::sleep(std::time::Duration::from_millis(left + 3_000)).await;
    worker.fast_forward(5).await?;
    let outcome =
        feeder.call(auction.id(), "clear_auction").args_json(json!({"auction_id": 0})).max_gas().transact().await?;
    println!("clearing {OPEN} winners out of {TOTAL} recorded bids: {}", outcome.total_gas_burnt);
    assert!(outcome.total_gas_burnt < Gas::from_tgas(200), "clearing must leave headroom under the 300 Tgas limit");
    outcome.into_result()?;

    let cleared: Value = auction.view("get_auction").args_json(json!({"auction_id": 0})).await?.json()?;
    assert_eq!((cleared["status"].as_str(), cleared["filled_units"].as_u64()), (Some("cleared"), Some(OPEN)));
    // Supply covers demand, so all hundred winners pay the reserve and get the difference back.
    assert_eq!(cleared["clearing_price"], USD.to_string());
    let config: Value = auction.view("get_config").await?.json()?;
    assert_eq!(config["entitlements"].as_u64(), Some(OPEN));
    let winner = format!("bidder-{}.test.near", TOTAL - 1);
    let refund: String = auction.view("balance_of").args_json(json!({"account_id": winner})).await?.json()?;
    assert_eq!(refund, (7 * (TOTAL - 1)).to_string());
    let proceeds: String = auction.view("balance_of").args_json(json!({"account_id": operator.id()})).await?.json()?;
    assert_eq!(proceeds, (OPEN as u128 * 7 * USD).to_string());
    Ok(())
}
