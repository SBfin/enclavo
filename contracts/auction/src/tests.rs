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

#[test]
fn demand_equal_to_supply_clears_at_reserve() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 6, 3 * USD, 10);
    bid(&mut c, "bob.near", 4, 2 * USD, 20);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    let auction = c.get_auction(0).unwrap();
    assert_eq!((auction.clearing_price, auction.filled_units), (Some(U128(USD / 2)), 10));
    assert_eq!(balance(&c, "operator.near"), 10 * 7 * USD / 2);
}

#[test]
fn bids_at_the_reserve_and_in_the_last_millisecond_are_accepted() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 1, USD / 2, 999);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    assert_eq!(c.get_bids(0)[0].status, BidStatus::Filled);
    // The whole escrow was spent, so nothing comes back.
    assert_eq!(balance(&c, "alice.near"), 0);
}

#[test]
fn late_clearing_starts_the_term_when_it_settles() {
    ctx("operator.near", 0);
    let mut c = Contract::new(acc("operator.near"), acc("usdc.near"));
    c.publish_auction("qwen@near-ai".into(), 5, 90, U128(USD), 2_000_000, 0, 1_000);
    fund(&mut c, "alice.near", 5 * 90 * USD, "", 10);
    ctx("alice.near", 10);
    c.place_bid(0, 5, U128(USD));
    ctx("anyone.near", 50_000);
    c.clear_auction(0);
    let won = c.get_entitlement(0).unwrap();
    assert_eq!((won.starts_at_ms, won.ends_at_ms), (50_000, 50_000 + 90 * 86_400_000));
    assert_eq!((won.tokens_per_iu, won.prompt_weight_bps, won.bid_index), (2_000_000, 0, 0));
    assert_eq!(c.get_auction(0).unwrap().cleared_at_ms, Some(50_000));
}

#[test]
fn a_bidder_can_hold_several_bids_and_win_with_each() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 3, 2 * USD, 10);
    bid(&mut c, "alice.near", 4, USD, 20);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    let won = c.entitlements_of(acc("alice.near"), None);
    // Newest first, one entitlement per winning bid.
    assert_eq!(won.iter().map(|e| (e.id, e.units)).collect::<Vec<_>>(), vec![(1, 4), (0, 3)]);
    assert_eq!(c.entitlements_of(acc("alice.near"), Some(1)).len(), 1);
    assert_eq!(c.bids_of(acc("alice.near"), None).iter().map(|b| b.index).collect::<Vec<_>>(), vec![1, 0]);
}

#[test]
fn auctions_are_independent_and_entitlement_ids_are_global() {
    let mut c = setup(2);
    ctx("operator.near", 5);
    let second = c.publish_auction("qwen@near-ai".into(), 3, 30, U128(USD), 1_000_000, 0, 5_000);
    assert_eq!(second, 1);
    bid(&mut c, "alice.near", 2, USD, 10);
    fund(&mut c, "bob.near", 3 * 30 * 2 * USD, r#"{"auction_id":1,"units":3,"max_price":"2000000"}"#, 20);
    assert_eq!((c.get_bids(0).len(), c.get_bids(1).len()), (1, 1));

    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    // The second auction is still open and keeps its escrow.
    assert_eq!(c.get_auction(1).unwrap().status, AuctionStatus::Open);
    assert_eq!(c.get_bids(1)[0].escrow.0, 3 * 30 * 2 * USD);
    ctx("anyone.near", 5_005);
    c.clear_auction(1);

    let all = c.list_entitlements(None, None);
    assert_eq!(all.iter().map(|e| (e.id, e.auction_id)).collect::<Vec<_>>(), vec![(0, 0), (1, 1)]);
    assert_eq!(c.list_entitlements(Some(1), None)[0].route_id, "qwen@near-ai");
    assert!(c.list_entitlements(Some(2), None).is_empty());
    assert_eq!(c.list_auctions(None, None).iter().map(|a| a.id).collect::<Vec<_>>(), vec![1, 0]);
    assert_eq!(c.list_auctions(Some(1), Some(5)).iter().map(|a| a.id).collect::<Vec<_>>(), vec![0]);
    // Bob's bid cleared at the reserve, half of what he escrowed.
    assert_eq!(balance(&c, "bob.near"), 3 * 30 * USD);
    assert_eq!(balance(&c, "operator.near"), 2 * 7 * USD / 2 + 3 * 30 * USD);
}

#[test]
fn cancelled_bid_frees_room_for_a_new_one() {
    let mut c = setup(5);
    let first = bid(&mut c, "alice.near", 5, 3 * USD, 10);
    ctx("alice.near", 20);
    c.cancel_bid(0, first);
    // The released escrow funds a cheaper bid without another transfer.
    let second = c.place_bid(0, 5, U128(USD));
    assert_eq!(balance(&c, "alice.near"), 5 * 7 * 2 * USD);
    let auction = c.get_auction(0).unwrap();
    assert_eq!((auction.open_bids, auction.open_bid_units), (1, 5));
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    let bids = c.get_bids(0);
    assert_eq!(bids[first as usize].status, BidStatus::Cancelled);
    assert_eq!((bids[second as usize].status, bids[second as usize].charged.0), (BidStatus::Filled, 5 * 7 * USD / 2));
    assert_solvent(&c, &["alice.near", "operator.near"]);
}

#[test]
#[should_panic(expected = "Only open bids can be cancelled")]
fn evicted_bid_cannot_be_cancelled_again() {
    let mut c = setup(1_000);
    for i in 0..MAX_OPEN_BIDS {
        bid(&mut c, &format!("b{i}.near"), 1, USD, 10);
    }
    bid(&mut c, "whale.near", 1, 2 * USD, 500);
    let last = format!("b{}.near", MAX_OPEN_BIDS - 1);
    ctx(&last, 600);
    c.cancel_bid(0, MAX_OPEN_BIDS - 1);
}

#[test]
fn full_book_evicts_the_cheapest_bid_first() {
    let mut c = setup(1_000);
    bid(&mut c, "cheap.near", 3, USD / 2, 5);
    for i in 1..MAX_OPEN_BIDS {
        bid(&mut c, &format!("b{i}.near"), 1, USD, 10);
    }
    bid(&mut c, "whale.near", 2, 2 * USD, 500);
    assert_eq!(c.get_bids(0)[0].status, BidStatus::Outbid);
    assert_eq!(balance(&c, "cheap.near"), 3 * 7 * USD / 2);
    let auction = c.get_auction(0).unwrap();
    assert_eq!((auction.open_bids, auction.open_bid_units), (MAX_OPEN_BIDS, MAX_OPEN_BIDS as u64 - 1 + 2));
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    assert_eq!(c.get_auction(0).unwrap().filled_units, MAX_OPEN_BIDS as u64 + 1);
    assert!(c.entitlements_of(acc("cheap.near"), None).is_empty());
}

#[test]
#[should_panic(expected = "closed to new bids")]
fn an_auction_records_a_bounded_number_of_bids() {
    let mut c = setup(10);
    for _ in 0..=MAX_TOTAL_BIDS {
        let index = bid(&mut c, "alice.near", 1, USD, 10);
        c.cancel_bid(0, index);
    }
}

#[test]
#[should_panic(expected = "Auction not found")]
fn bid_on_unknown_auction_is_rejected() {
    let mut c = setup(10);
    fund(&mut c, "alice.near", 7 * USD, "", 10);
    ctx("alice.near", 10);
    c.place_bid(7, 1, U128(USD));
}

#[test]
#[should_panic(expected = "Quantity must be")]
fn zero_unit_bid_is_rejected() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 0, USD, 10);
}

#[test]
#[should_panic(expected = "notional is too large")]
fn overflowing_bid_is_rejected() {
    let mut c = setup(10);
    ctx("alice.near", 10);
    c.place_bid(0, MAX_UNITS, U128(u128::MAX));
}

#[test]
#[should_panic(expected = "no longer accepting bids")]
fn bid_on_cleared_auction_is_rejected() {
    let mut c = setup(10);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    bid(&mut c, "alice.near", 1, USD, 1_001);
}

#[test]
#[should_panic(expected = "msg must be empty or a bid JSON")]
fn malformed_transfer_message_is_rejected() {
    let mut c = setup(10);
    fund(&mut c, "alice.near", 7 * USD, "bid please", 10);
}

#[test]
#[should_panic(expected = "does not cover this bid")]
fn transfer_smaller_than_its_bid_is_rejected() {
    let mut c = setup(10);
    fund(&mut c, "alice.near", 13 * USD, r#"{"auction_id":0,"units":2,"max_price":"1000000"}"#, 10);
}

#[test]
#[should_panic(expected = "balance too low")]
fn escrowed_funds_cannot_be_withdrawn() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 2, USD, 10);
    let _ = c.withdraw(Some(U128(USD)));
}

#[test]
#[should_panic(expected = "Nothing to withdraw")]
fn empty_balance_cannot_be_withdrawn() {
    let mut c = setup(10);
    ctx("alice.near", 10);
    let _ = c.withdraw(None);
}

#[test]
fn operator_withdraws_proceeds_after_clearing() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 4, USD, 10);
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    ctx("operator.near", 1_001);
    let _ = c.withdraw(None);
    c.on_withdraw(acc("operator.near"), U128(14 * USD), Ok(()));
    assert_eq!(balance(&c, "operator.near"), 0);
    // Only alice's refund is still owed.
    assert_eq!(c.total_liabilities, balance(&c, "alice.near"));
    assert_eq!(balance(&c, "alice.near"), 14 * USD);
}

#[test]
#[should_panic(expected = "7, 30 or 90-day term")]
fn unsupported_term_is_rejected() {
    let mut c = setup(10);
    c.publish_auction("glm@near-ai".into(), 1, 14, U128(USD), 1, 0, 1_000);
}

#[test]
#[should_panic(expected = "Reserve price must be positive")]
fn free_capacity_is_rejected() {
    let mut c = setup(10);
    c.publish_auction("glm@near-ai".into(), 1, 7, U128(0), 1, 0, 1_000);
}

#[test]
#[should_panic(expected = "Auction duration")]
fn overlong_auction_is_rejected() {
    let mut c = setup(10);
    c.publish_auction("glm@near-ai".into(), 1, 7, U128(USD), 1, 0, MAX_DURATION_MS + 1);
}

#[test]
fn revoking_without_a_key_does_nothing() {
    let mut c = setup(10);
    ctx("alice.near", 10);
    c.revoke_key();
    assert_eq!(c.key_hash_of(acc("alice.near")), None);
}

#[test]
fn config_reports_counts_and_liabilities() {
    let mut c = setup(10);
    bid(&mut c, "alice.near", 2, USD, 10);
    let config = c.get_config();
    assert_eq!((config["owner"].as_str(), config["token"].as_str()), (Some("operator.near"), Some("usdc.near")));
    assert_eq!((config["auctions"].as_u64(), config["entitlements"].as_u64()), (Some(1), Some(0)));
    assert_eq!(config["total_liabilities"], (14 * USD).to_string());
}

/// Small deterministic generator, so a failing round can be replayed from its number.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// Random books of bids and cancellations, settled by the contract and by an independent model.
#[test]
fn random_auctions_match_the_reference_model() {
    const BIDDERS: usize = 6;
    let reserve = USD / 2;
    for round in 0..120u64 {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (round + 1));
        ctx("operator.near", 0);
        near_sdk::mock::with_mocked_blockchain(|b| drop(b.take_storage()));
        let supply = 1 + rng.below(40);
        let mut c = setup(supply);
        let names: Vec<String> = (0..BIDDERS).map(|i| format!("b{i}.near")).collect();

        // (bidder, units, price, still open), in arrival order like the contract's book.
        let mut book: Vec<(usize, u64, u128, bool)> = Vec::new();
        let mut funded = [0u128; BIDDERS];
        for step in 0..rng.below(30) {
            let now = 10 + step;
            let open: Vec<usize> = (0..book.len()).filter(|&i| book[i].3).collect();
            if !open.is_empty() && rng.below(4) == 0 {
                let i = open[rng.below(open.len() as u64) as usize];
                ctx(&names[book[i].0], now);
                c.cancel_bid(0, i as u32);
                book[i].3 = false;
            } else {
                let who = rng.below(BIDDERS as u64) as usize;
                let units = 1 + rng.below(15);
                // Quarter-dollar steps make equal prices common.
                let price = reserve + rng.below(9) as u128 * USD / 4;
                let index = bid(&mut c, &names[who], units, price, now);
                assert_eq!(index as usize, book.len());
                book.push((who, units, price, true));
                funded[who] += units as u128 * 7 * price;
            }
            let escrow: u128 = c.get_bids(0).iter().map(|b| b.escrow.0).sum();
            let unlocked: u128 = names.iter().map(|n| balance(&c, n)).sum();
            assert_eq!(escrow + unlocked, c.total_liabilities, "round {round}");
        }

        ctx("anyone.near", 1_000);
        c.clear_auction(0);

        // Reference: walk price levels from the top, earlier bids first within a level.
        let mut prices: Vec<u128> = book.iter().filter(|b| b.3).map(|b| b.2).collect();
        prices.sort_unstable_by(|a, b| b.cmp(a));
        prices.dedup();
        let demand: u64 = book.iter().filter(|b| b.3).map(|b| b.1).sum();
        let mut fills = vec![0u64; book.len()];
        let mut left = supply;
        let mut marginal = reserve;
        for level in prices {
            for (i, b) in book.iter().enumerate() {
                if b.3 && b.2 == level && left > 0 {
                    fills[i] = left.min(b.1);
                    left -= fills[i];
                    marginal = level;
                }
            }
        }
        let price = if demand > supply { marginal } else { reserve };

        let auction = c.get_auction(0).unwrap();
        let filled: u64 = fills.iter().sum();
        assert_eq!(auction.filled_units, filled.min(supply), "round {round}");
        assert_eq!(auction.filled_units, demand.min(supply), "round {round}");
        assert_eq!(auction.clearing_price, (filled > 0).then_some(U128(price)), "round {round}");

        let bids = c.get_bids(0);
        let mut spent = [0u128; BIDDERS];
        for (i, b) in book.iter().enumerate() {
            let bid = &bids[i];
            assert_eq!(bid.filled_units, fills[i], "round {round} bid {i}");
            assert_eq!(bid.charged.0, fills[i] as u128 * 7 * price, "round {round} bid {i}");
            assert_eq!(bid.escrow.0, 0, "round {round} bid {i}");
            if b.3 {
                // Nobody pays above their limit; a bid above the price fills fully, one below gets nothing.
                assert!(fills[i] == 0 || price <= b.2, "round {round} bid {i}");
                assert!(b.2 <= price || fills[i] == b.1, "round {round} bid {i}");
                assert!(b.2 >= price || fills[i] == 0, "round {round} bid {i}");
            } else {
                assert_eq!(bid.status, BidStatus::Cancelled, "round {round} bid {i}");
            }
            spent[b.0] += bid.charged.0;
        }
        for (i, name) in names.iter().enumerate() {
            assert_eq!(balance(&c, name), funded[i] - spent[i], "round {round} {name}");
        }
        let proceeds: u128 = spent.iter().sum();
        assert_eq!(balance(&c, "operator.near"), proceeds, "round {round}");
        assert_eq!(c.total_liabilities, funded.iter().sum::<u128>(), "round {round}");

        let won = c.list_entitlements(None, Some(100));
        assert_eq!(won.len(), fills.iter().filter(|&&f| f > 0).count(), "round {round}");
        assert_eq!(won.iter().map(|e| e.units).sum::<u64>(), filled, "round {round}");
        assert_eq!(won.iter().map(|e| e.paid.0).sum::<u128>(), proceeds, "round {round}");
    }
}

/// Regression: a log per winner used to overrun the receipt log limits and make a busy auction
/// impossible to clear.
#[test]
fn full_book_of_winners_clears_with_a_single_event() {
    let mut c = setup(1_000);
    for i in 0..MAX_OPEN_BIDS {
        bid(&mut c, &format!("b{i}.near"), 1, USD, 10);
    }
    ctx("anyone.near", 1_000);
    c.clear_auction(0);
    let logs = near_sdk::test_utils::get_logs();
    assert_eq!(logs.len(), 1);
    assert!(logs[0].contains(r#""event":"auction_cleared""#) && logs[0].contains(r#""entitlements":100"#));
    assert_eq!(c.list_entitlements(None, Some(100)).len(), MAX_OPEN_BIDS as usize);
    assert_eq!(c.get_auction(0).unwrap().filled_units, MAX_OPEN_BIDS as u64);
}
