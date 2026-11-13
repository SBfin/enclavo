use super::*;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::testing_env;

const USD: u128 = 1_000_000;

fn acc(name: &str) -> AccountId {
    name.parse().unwrap()
}

fn ctx(caller: &str, now_ms: u64) {
    testing_env!(VMContextBuilder::new()
        .predecessor_account_id(acc(caller))
        .block_timestamp(now_ms * 1_000_000)
        .build());
}

/// A 7-day auction for `units` IU/day at a $0.50 reserve, open for 1000 ms from t=0.
fn setup(units: u64) -> Contract {
    ctx("operator.near", 0);
    let mut c = Contract::new(acc("operator.near"), acc("usdc.near"));
    let id = c.publish_auction("glm@near-ai".into(), units, 7, U128(USD / 2), 1_000_000, 1_600, 1_000);
    assert_eq!(id, 0);
    c
}

fn fund(c: &mut Contract, who: &str, amount: u128, msg: &str, now_ms: u64) {
    ctx("usdc.near", now_ms);
    let _ = c.ft_on_transfer(acc(who), U128(amount), msg.into());
}

fn bid(c: &mut Contract, who: &str, units: u64, price: u128, now_ms: u64) -> u32 {
    fund(c, who, units as u128 * 7 * price, "", now_ms);
    ctx(who, now_ms);
    c.place_bid(0, units, U128(price))
}

fn balance(c: &Contract, who: &str) -> u128 {
    c.balance_of(acc(who)).0
}

/// Unlocked balances plus live escrow must always equal what the contract was paid.
fn assert_solvent(c: &Contract, accounts: &[&str]) {
    let unlocked: u128 = accounts.iter().map(|a| balance(c, a)).sum();
    let escrow: u128 = c.get_bids(0).iter().map(|b| b.escrow.0).sum();
    assert_eq!(unlocked + escrow, c.total_liabilities);
}

#[test]
fn undersubscribed_auction_clears_at_reserve() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 4, USD, 10);
    bid(&mut c, "bob.near", 3, 2 * USD, 20);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);

    let auction = c.get_auction(0).unwrap();
    assert_eq!(auction.status, AuctionStatus::Cleared);
    assert_eq!(auction.clearing_price, Some(U128(USD / 2)));
    assert_eq!(auction.filled_units, 7);
    // Both pay the reserve, not their own maximum.
    assert_eq!(balance(&c, "alice.near"), 4 * 7 * USD - 4 * 7 * USD / 2);
    assert_eq!(balance(&c, "bob.near"), 3 * 7 * 2 * USD - 3 * 7 * USD / 2);
    assert_eq!(balance(&c, "operator.near"), 7 * 7 * USD / 2);
    assert!(c.get_bids(0).iter().all(|b| b.status == BidStatus::Filled && b.escrow.0 == 0));
    assert_solvent(&c, &["alice.near", "bob.near", "operator.near"]);
}

#[test]
fn oversubscribed_auction_clears_at_marginal_bid_with_partial_fill() {
    let mut c = setup(10);
    bid(&mut c, "low.near", 5, USD, 10);
    bid(&mut c, "high.near", 6, 3 * USD, 20);
    bid(&mut c, "mid.near", 6, 2 * USD, 30);
    ctx("anyone.near", 2_000);
    c.clear_auction(0);

    // high takes 6, mid takes the last 4 and sets the price, low gets nothing.
    let auction = c.get_auction(0).unwrap();
    assert_eq!(auction.clearing_price, Some(U128(2 * USD)));
    assert_eq!(auction.filled_units, 10);
    let bids = c.get_bids(0);
    assert_eq!((bids[0].status, bids[0].filled_units), (BidStatus::Outbid, 0));
    assert_eq!((bids[1].status, bids[1].filled_units), (BidStatus::Filled, 6));
    assert_eq!((bids[2].status, bids[2].filled_units), (BidStatus::Partial, 4));
    assert_eq!(balance(&c, "low.near"), 5 * 7 * USD);
    assert_eq!(balance(&c, "high.near"), 6 * 7 * USD);
    assert_eq!(balance(&c, "mid.near"), 2 * 7 * 2 * USD);
    assert_eq!(balance(&c, "operator.near"), 10 * 7 * 2 * USD);

    let won = c.entitlements_of(acc("mid.near"), None);
    assert_eq!(won.len(), 1);
    assert_eq!((won[0].units, won[0].paid.0, won[0].route_id.as_str()), (4, 4 * 7 * 2 * USD, "glm@near-ai"));
    assert_eq!((won[0].starts_at_ms, won[0].ends_at_ms), (2_000, 2_000 + 7 * 86_400_000));
    assert!(c.entitlements_of(acc("low.near"), None).is_empty());
    assert_eq!(c.list_entitlements(None, None).len(), 2);
    assert_solvent(&c, &["low.near", "high.near", "mid.near", "operator.near"]);
}

#[test]
fn equal_prices_fill_in_arrival_order() {
    let mut c = setup(5);
    bid(&mut c, "first.near", 4, USD, 10);
    bid(&mut c, "second.near", 4, USD, 20);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    let bids = c.get_bids(0);
    assert_eq!((bids[0].filled_units, bids[1].filled_units), (4, 1));
    assert_eq!(c.get_auction(0).unwrap().clearing_price, Some(U128(USD)));
}

#[test]
fn auction_without_bids_clears_empty() {
    let mut c = setup(5);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    let auction = c.get_auction(0).unwrap();
    assert_eq!((auction.status, auction.clearing_price, auction.filled_units), (AuctionStatus::Cleared, None, 0));
    assert_eq!(balance(&c, "operator.near"), 0);
}

#[test]
fn transfer_with_bid_message_escrows_and_keeps_change() {
    let mut c = setup(10);
    let msg = r#"{"auction_id":0,"units":2,"max_price":"1000000"}"#;
    fund(&mut c, "alice.near", 20 * USD, msg, 10);
    let bids = c.bids_of(acc("alice.near"), None);
    assert_eq!((bids.len(), bids[0].escrow.0), (1, 14 * USD));
    assert_eq!(balance(&c, "alice.near"), 6 * USD);
    assert_solvent(&c, &["alice.near"]);
}

#[test]
fn cancel_releases_escrow_and_is_idempotent() {
    let mut c = setup(10);
    let index = bid(&mut c, "alice.near", 2, USD, 10);
    assert_eq!(balance(&c, "alice.near"), 0);
    ctx("alice.near", 500);
    c.cancel_bid(0, index);
    c.cancel_bid(0, index);
    assert_eq!(balance(&c, "alice.near"), 14 * USD);
    assert_eq!(c.get_auction(0).unwrap().open_bids, 0);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    assert_eq!(c.get_auction(0).unwrap().filled_units, 0);
    assert_eq!(balance(&c, "alice.near"), 14 * USD);
}

#[test]
fn full_book_evicts_its_weakest_bid_for_a_better_one() {
    let mut c = setup(1_000);
    for i in 0..MAX_OPEN_BIDS {
        bid(&mut c, &format!("b{i}.near"), 1, USD, 10 + i as u64);
    }
    // The latest of the equally priced bids is the weakest.
    let last = format!("b{}.near", MAX_OPEN_BIDS - 1);
    bid(&mut c, "whale.near", 1, 2 * USD, 500);
    assert_eq!(c.get_bids(0)[MAX_OPEN_BIDS as usize - 1].status, BidStatus::Outbid);
    assert_eq!(balance(&c, &last), 7 * USD);
    assert_eq!(c.get_auction(0).unwrap().open_bids, MAX_OPEN_BIDS);
}

#[test]
#[should_panic(expected = "The order book is full")]
fn full_book_rejects_a_bid_that_does_not_outprice_it() {
    let mut c = setup(1_000);
    for i in 0..=MAX_OPEN_BIDS {
        bid(&mut c, &format!("b{i}.near"), 1, USD, 10);
    }
}

#[test]
#[should_panic(expected = "does not cover this bid")]
fn bid_needs_full_escrow() {
    let mut c = setup(10);
    fund(&mut c, "alice.near", 13 * USD, "", 10);
    ctx("alice.near", 10);
    c.place_bid(0, 2, U128(USD));
}

#[test]
#[should_panic(expected = "below the reserve")]
fn bid_below_reserve_is_rejected() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 1, USD / 2 - 1, 10);
}

#[test]
#[should_panic(expected = "no longer accepting bids")]
fn bid_after_close_is_rejected() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 1, USD, 1_000);
}

#[test]
#[should_panic(expected = "cannot be cancelled after the auction closes")]
fn cancel_after_close_is_rejected() {
    let mut c = setup(10);
    let index = bid(&mut c, "alice.near", 1, USD, 10);
    ctx("alice.near", 1_000);
    c.cancel_bid(0, index);
}

#[test]
#[should_panic(expected = "Bid not found")]
fn cancelling_someone_elses_bid_is_rejected() {
    let mut c = setup(10);
    let index = bid(&mut c, "alice.near", 1, USD, 10);
    ctx("mallory.near", 20);
    c.cancel_bid(0, index);
}

#[test]
#[should_panic(expected = "still accepting bids")]
fn clearing_early_is_rejected() {
    let mut c = setup(10);
    ctx("anyone.near", 999);
    c.clear_auction(0);
}

#[test]
#[should_panic(expected = "already cleared")]
fn clearing_twice_is_rejected() {
    let mut c = setup(10);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    c.clear_auction(0);
}

#[test]
#[should_panic(expected = "Only the operator")]
fn only_operator_publishes() {
    let mut c = setup(10);
    ctx("mallory.near", 10);
    c.publish_auction("glm@near-ai".into(), 1, 7, U128(1), 1, 0, 1_000);
}

#[test]
#[should_panic(expected = "Only the settlement token")]
fn other_tokens_are_refused() {
    let mut c = setup(10);
    ctx("fake-usdc.near", 10);
    let _ = c.ft_on_transfer(acc("alice.near"), U128(USD), String::new());
}

#[test]
fn failed_withdrawal_restores_the_balance() {
    let mut c = setup(10);
    fund(&mut c, "alice.near", 5 * USD, "", 10);
    ctx("alice.near", 20);
    let _ = c.withdraw(Some(U128(2 * USD)));
    assert_eq!(balance(&c, "alice.near"), 3 * USD);
    c.on_withdraw(acc("alice.near"), U128(2 * USD), Err(PromiseError::Failed));
    assert_eq!(balance(&c, "alice.near"), 5 * USD);
    assert_solvent(&c, &["alice.near"]);
    let _ = c.withdraw(None);
    c.on_withdraw(acc("alice.near"), U128(5 * USD), Ok(()));
    assert_eq!((balance(&c, "alice.near"), c.total_liabilities), (0, 0));
}

#[test]
fn wallet_authorizes_replaces_and_revokes_a_gateway_key() {
    let mut c = setup(10);
    let first = "ab".repeat(32);
    ctx("alice.near", 10);
    c.authorize_key(first.to_uppercase());
    assert_eq!(c.key_hash_of(acc("alice.near")), Some(first));
    assert_eq!(c.key_hash_of(acc("bob.near")), None);
    c.authorize_key("cd".repeat(32));
    assert_eq!(c.key_hash_of(acc("alice.near")), Some("cd".repeat(32)));
    c.revoke_key();
    assert_eq!(c.key_hash_of(acc("alice.near")), None);
}

#[test]
#[should_panic(expected = "64 hex characters")]
fn malformed_key_hash_is_rejected() {
    let mut c = setup(10);
    ctx("alice.near", 10);
    c.authorize_key("zz".repeat(32));
}
