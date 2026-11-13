//! Escrowed capacity auctions with uniform-price clearing, mirroring `gateway/src/market.rs`.
//!
//! Money is the settlement token's atomic unit everywhere. With 6-decimal USDC that is exactly
//! the gateway's micro-USD. Prices are per IU-day; a bid escrows `units * term_days * max_price`.
#![allow(clippy::too_many_arguments)]

use near_sdk::json_types::U128;
use near_sdk::serde_json::{self, json, Value};
use near_sdk::store::{LookupMap, Vector};
use near_sdk::{
    env, ext_contract, near, require, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError, PromiseOrValue,
};

/// Open bids one auction keeps. A full book only admits a bid that outprices its weakest one.
pub const MAX_OPEN_BIDS: u32 = 100;
/// Every bid ever recorded in one auction, so clearing always fits in a single transaction.
pub const MAX_TOTAL_BIDS: u32 = 400;
pub const MAX_UNITS: u64 = 1_000_000;
pub const MAX_DURATION_MS: u64 = 7 * 86_400_000;
const DAY_MS: u64 = 86_400_000;
const GAS_FT_TRANSFER: Gas = Gas::from_tgas(15);
const GAS_ON_WITHDRAW: Gas = Gas::from_tgas(10);

#[near(serializers = [borsh])]
#[derive(BorshStorageKey)]
enum Key {
    Balances,
    Auctions,
    Bids,
    AccountBids,
    Entitlements,
    AccountEntitlements,
    ApiKeys,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuctionStatus {
    Open,
    Cleared,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BidStatus {
    Open,
    Filled,
    Partial,
    Outbid,
    Cancelled,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Auction {
    pub id: u64,
    /// Gateway route the capacity is served on, e.g. `z-ai/glm-5.3-flash@near-ai`.
    pub route_id: String,
    /// IU/day on offer.
    pub units: u64,
    pub term_days: u32,
    pub reserve_price: U128,
    pub tokens_per_iu: u64,
    pub prompt_weight_bps: u32,
    pub created_at_ms: u64,
    pub ends_at_ms: u64,
    pub status: AuctionStatus,
    pub cleared_at_ms: Option<u64>,
    pub clearing_price: Option<U128>,
    pub filled_units: u64,
    pub open_bids: u32,
    pub open_bid_units: u64,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Bid {
    pub auction_id: u64,
    /// Position in the auction's book; earlier bids win ties.
    pub index: u32,
    pub bidder: AccountId,
    pub units: u64,
    pub max_price: U128,
    /// Still locked for this bid. Zero once the bid is cancelled, outbid or cleared.
    pub escrow: U128,
    pub created_at_ms: u64,
    pub status: BidStatus,
    pub filled_units: u64,
    pub charged: U128,
}

/// A won block of daily capacity. The gateway meters usage against it off-chain.
#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Entitlement {
    pub id: u64,
    pub auction_id: u64,
    pub bid_index: u32,
    pub owner: AccountId,
    pub route_id: String,
    pub units: u64,
    pub tokens_per_iu: u64,
    pub prompt_weight_bps: u32,
    pub starts_at_ms: u64,
    pub ends_at_ms: u64,
    pub paid: U128,
}

#[near(serializers = [json])]
struct BidMsg {
    auction_id: u64,
    units: u64,
    max_price: U128,
}

#[ext_contract(ext_ft)]
pub trait FungibleToken {
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    /// Operator that publishes capacity and receives auction proceeds.
    owner: AccountId,
    /// NEP-141 settlement token.
    token: AccountId,
    /// Unlocked funds per account: deposits, released escrow, and the owner's proceeds.
    balances: LookupMap<AccountId, u128>,
    auctions: Vector<Auction>,
    bids: LookupMap<u64, Vec<Bid>>,
    account_bids: LookupMap<AccountId, Vec<(u64, u32)>>,
    entitlements: Vector<Entitlement>,
    account_entitlements: LookupMap<AccountId, Vec<u64>>,
    /// Everything the contract owes: unlocked balances plus live bid escrow.
    total_liabilities: u128,
}

fn notional(units: u64, term_days: u32, price: u128) -> u128 {
    (units as u128)
        .checked_mul(term_days as u128)
        .and_then(|n| n.checked_mul(price))
        .unwrap_or_else(|| env::panic_str("Order notional is too large"))
}

/// Authorized key hashes live under their own storage prefix rather than in the contract struct,
/// so adding them did not change the layout of already deployed state.
fn api_keys() -> LookupMap<AccountId, [u8; 32]> {
    LookupMap::new(Key::ApiKeys)
}

fn hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(value.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn emit(event: &str, data: Value) {
    env::log_str(&format!(
        "EVENT_JSON:{}",
        json!({"standard": "enclavo-auction", "version": "1.0.0", "event": event, "data": [data]})
    ));
}

#[near]
impl Contract {
    #[init]
    pub fn new(owner: AccountId, token: AccountId) -> Self {
        Self {
            owner,
            token,
            balances: LookupMap::new(Key::Balances),
            auctions: Vector::new(Key::Auctions),
            bids: LookupMap::new(Key::Bids),
            account_bids: LookupMap::new(Key::AccountBids),
            entitlements: Vector::new(Key::Entitlements),
            account_entitlements: LookupMap::new(Key::AccountEntitlements),
            total_liabilities: 0,
        }
    }

    // ----- operator -----

    /// Offers `units` IU/day of `route_id` for `term_days`, accepting bids for `duration_ms`.
    pub fn publish_auction(
        &mut self,
        route_id: String,
        units: u64,
        term_days: u32,
        reserve_price: U128,
        tokens_per_iu: u64,
        prompt_weight_bps: u32,
        duration_ms: u64,
    ) -> u64 {
        require!(env::predecessor_account_id() == self.owner, "Only the operator can publish auctions");
        require!(!route_id.is_empty() && route_id.len() <= 128, "route_id must be 1-128 bytes");
        require!(
            (1..=MAX_UNITS).contains(&units) && [7, 30, 90].contains(&term_days),
            "Use 1-1,000,000 IU/day and a 7, 30 or 90-day term"
        );
        require!(reserve_price.0 > 0, "Reserve price must be positive");
        require!(tokens_per_iu > 0, "tokens_per_iu must be positive");
        require!((1..=MAX_DURATION_MS).contains(&duration_ms), "Auction duration must be 1 ms to 7 days");
        notional(units, term_days, reserve_price.0);
        let id = self.auctions.len() as u64;
        let now = env::block_timestamp_ms();
        let auction = Auction {
            id,
            route_id,
            units,
            term_days,
            reserve_price,
            tokens_per_iu,
            prompt_weight_bps,
            created_at_ms: now,
            ends_at_ms: now + duration_ms,
            status: AuctionStatus::Open,
            cleared_at_ms: None,
            clearing_price: None,
            filled_units: 0,
            open_bids: 0,
            open_bid_units: 0,
        };
        emit("auction_published", json!(auction));
        self.auctions.push(auction);
        self.bids.insert(id, Vec::new());
        id
    }

    // ----- funds -----

    /// NEP-141 receiver. An empty `msg` credits the sender's balance; a JSON
    /// `{"auction_id":0,"units":10,"max_price":"500000"}` credits it and places that bid at once.
    /// A rejected bid aborts the call, so the token contract returns the whole transfer.
    pub fn ft_on_transfer(&mut self, sender_id: AccountId, amount: U128, msg: String) -> PromiseOrValue<U128> {
        require!(env::predecessor_account_id() == self.token, "Only the settlement token is accepted");
        self.credit(&sender_id, amount.0);
        self.total_liabilities += amount.0;
        if !msg.trim().is_empty() {
            let bid: BidMsg =
                serde_json::from_str(&msg).unwrap_or_else(|_| env::panic_str("msg must be empty or a bid JSON"));
            self.internal_place_bid(sender_id, bid.auction_id, bid.units, bid.max_price.0);
        }
        PromiseOrValue::Value(U128(0))
    }

    /// Places a bid funded from the caller's unlocked balance. Returns its index in the book.
    pub fn place_bid(&mut self, auction_id: u64, units: u64, max_price: U128) -> u32 {
        self.internal_place_bid(env::predecessor_account_id(), auction_id, units, max_price.0)
    }

    /// Withdraws a bid before the auction closes and unlocks its escrow.
    pub fn cancel_bid(&mut self, auction_id: u64, index: u32) {
        let caller = env::predecessor_account_id();
        let now = env::block_timestamp_ms();
        let auction = self.auctions.get_mut(auction_id as u32).unwrap_or_else(|| env::panic_str("Auction not found"));
        let book = self.bids.get_mut(&auction_id).unwrap();
        let bid = book.get_mut(index as usize).unwrap_or_else(|| env::panic_str("Bid not found"));
        require!(bid.bidder == caller, "Bid not found");
        if bid.status == BidStatus::Cancelled {
            return;
        }
        require!(
            auction.status == AuctionStatus::Open && now < auction.ends_at_ms,
            "Bids cannot be cancelled after the auction closes"
        );
        require!(bid.status == BidStatus::Open, "Only open bids can be cancelled");
        let released = bid.escrow.0;
        bid.status = BidStatus::Cancelled;
        bid.escrow = U128(0);
        auction.open_bids -= 1;
        auction.open_bid_units -= bid.units;
        emit("bid_cancelled", json!({"auction_id": auction_id, "index": index, "bidder": caller}));
        self.credit(&caller, released);
    }

    /// Settles a closed auction. Anyone may call it once bidding has ended.
    ///
    /// Eligible bids fill in price/time priority at one price: the reserve when supply covers
    /// demand, otherwise the marginal winning bid. Winners receive entitlements, every bidder
    /// gets back whatever their escrow did not pay for, and proceeds go to the operator.
    pub fn clear_auction(&mut self, auction_id: u64) {
        let now = env::block_timestamp_ms();
        let auction = self.auctions.get(auction_id as u32).unwrap_or_else(|| env::panic_str("Auction not found"));
        require!(auction.status == AuctionStatus::Open, "Auction is already cleared");
        require!(now >= auction.ends_at_ms, "Auction is still accepting bids");
        let mut auction = auction.clone();
        let mut book = self.bids.remove(&auction_id).unwrap();
        let reserve = auction.reserve_price.0;

        let mut order: Vec<usize> = (0..book.len()).filter(|&i| book[i].status == BidStatus::Open).collect();
        order.sort_by(|&a, &b| book[b].max_price.0.cmp(&book[a].max_price.0).then(a.cmp(&b)));

        let mut remaining = auction.units;
        let mut demand: u128 = 0;
        let mut marginal = reserve;
        let mut fills = Vec::with_capacity(order.len());
        for &i in &order {
            let eligible = book[i].max_price.0 >= reserve;
            if eligible {
                demand += book[i].units as u128;
            }
            let fill = if eligible { remaining.min(book[i].units) } else { 0 };
            remaining -= fill;
            if fill > 0 {
                marginal = book[i].max_price.0;
            }
            fills.push(fill);
        }
        let price = if demand > auction.units as u128 { marginal } else { reserve };

        let mut proceeds: u128 = 0;
        for (&i, fill) in order.iter().zip(fills) {
            let bid = &mut book[i];
            let charged = notional(fill, auction.term_days, price);
            let escrow = bid.escrow.0;
            require!(charged <= escrow, "Bid escrow does not cover its fill");
            bid.status = if fill == 0 {
                BidStatus::Outbid
            } else if fill < bid.units {
                BidStatus::Partial
            } else {
                BidStatus::Filled
            };
            bid.filled_units = fill;
            bid.charged = U128(charged);
            bid.escrow = U128(0);
            proceeds += charged;
            let bidder = bid.bidder.clone();
            if fill > 0 {
                let entitlement = Entitlement {
                    id: self.entitlements.len() as u64,
                    auction_id,
                    bid_index: bid.index,
                    owner: bidder.clone(),
                    route_id: auction.route_id.clone(),
                    units: fill,
                    tokens_per_iu: auction.tokens_per_iu,
                    prompt_weight_bps: auction.prompt_weight_bps,
                    starts_at_ms: now,
                    ends_at_ms: now + auction.term_days as u64 * DAY_MS,
                    paid: U128(charged),
                };
                emit("entitlement_created", json!(entitlement));
                let mut owned = self.account_entitlements.get(&bidder).cloned().unwrap_or_default();
                owned.push(entitlement.id);
                self.account_entitlements.insert(bidder.clone(), owned);
                self.entitlements.push(entitlement);
            }
            self.credit(&bidder, escrow - charged);
        }
        let owner = self.owner.clone();
        self.credit(&owner, proceeds);

        auction.status = AuctionStatus::Cleared;
        auction.cleared_at_ms = Some(now);
        auction.filled_units = auction.units - remaining;
        auction.clearing_price = (auction.filled_units > 0).then_some(U128(price));
        auction.open_bids = 0;
        auction.open_bid_units = 0;
        emit(
            "auction_cleared",
            json!({"auction_id": auction_id, "clearing_price": auction.clearing_price,
                   "filled_units": auction.filled_units, "proceeds": U128(proceeds)}),
        );
        self.auctions.replace(auction_id as u32, auction);
        self.bids.insert(auction_id, book);
    }

    /// Sends unlocked balance back to the caller's wallet. Omit `amount` to withdraw everything.
    pub fn withdraw(&mut self, amount: Option<U128>) -> Promise {
        let account = env::predecessor_account_id();
        let balance = self.balances.get(&account).copied().unwrap_or(0);
        let amount = amount.map(|a| a.0).unwrap_or(balance);
        require!(amount > 0 && amount <= balance, "Nothing to withdraw or balance too low");
        self.balances.insert(account.clone(), balance - amount);
        self.total_liabilities -= amount;
        ext_ft::ext(self.token.clone())
            .with_attached_deposit(NearToken::from_yoctonear(1))
            .with_static_gas(GAS_FT_TRANSFER)
            .ft_transfer(account.clone(), U128(amount), None)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_ON_WITHDRAW)
                    .on_withdraw(account, U128(amount)),
            )
    }

    #[private]
    pub fn on_withdraw(
        &mut self,
        account: AccountId,
        amount: U128,
        #[callback_result] result: Result<(), PromiseError>,
    ) -> U128 {
        if result.is_ok() {
            return amount;
        }
        self.credit(&account, amount.0);
        self.total_liabilities += amount.0;
        U128(0)
    }

    // ----- gateway access -----

    /// Lets the caller's wallet authorize one gateway API key by its SHA-256 hash (64 hex chars).
    /// The key itself never touches the chain. The gateway then accepts it for this account's
    /// entitlements; calling again replaces the previous key.
    pub fn authorize_key(&mut self, key_hash: String) {
        let hash: [u8; 32] = hex_32(&key_hash).unwrap_or_else(|| env::panic_str("key_hash must be 64 hex characters"));
        let account = env::predecessor_account_id();
        api_keys().insert(account.clone(), hash);
        emit("key_authorized", json!({"account_id": account, "key_hash": key_hash.to_lowercase()}));
    }

    /// Withdraws the caller's gateway key authorization.
    pub fn revoke_key(&mut self) {
        let account = env::predecessor_account_id();
        if api_keys().remove(&account).is_some() {
            emit("key_revoked", json!({"account_id": account}));
        }
    }

    // ----- views -----

    pub fn key_hash_of(&self, account_id: AccountId) -> Option<String> {
        api_keys().get(&account_id).map(|hash| hash.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn get_config(&self) -> Value {
        json!({"owner": self.owner, "token": self.token, "max_open_bids": MAX_OPEN_BIDS,
               "auctions": self.auctions.len(), "entitlements": self.entitlements.len(),
               "total_liabilities": U128(self.total_liabilities)})
    }

    pub fn balance_of(&self, account_id: AccountId) -> U128 {
        U128(self.balances.get(&account_id).copied().unwrap_or(0))
    }

    pub fn get_auction(&self, auction_id: u64) -> Option<Auction> {
        self.auctions.get(auction_id as u32).cloned()
    }

    /// Newest first.
    pub fn list_auctions(&self, from_index: Option<u64>, limit: Option<u64>) -> Vec<Auction> {
        let skip = from_index.unwrap_or(0) as usize;
        self.auctions.iter().rev().skip(skip).take(limit.unwrap_or(50).min(100) as usize).cloned().collect()
    }

    pub fn get_bids(&self, auction_id: u64) -> Vec<Bid> {
        self.bids.get(&auction_id).cloned().unwrap_or_default()
    }

    /// Newest first.
    pub fn bids_of(&self, account_id: AccountId, limit: Option<u64>) -> Vec<Bid> {
        let refs = self.account_bids.get(&account_id).cloned().unwrap_or_default();
        refs.iter()
            .rev()
            .take(limit.unwrap_or(50).min(100) as usize)
            .map(|(auction, index)| self.bids.get(auction).unwrap()[*index as usize].clone())
            .collect()
    }

    pub fn get_entitlement(&self, entitlement_id: u64) -> Option<Entitlement> {
        self.entitlements.get(entitlement_id as u32).cloned()
    }

    /// Newest first.
    pub fn entitlements_of(&self, account_id: AccountId, limit: Option<u64>) -> Vec<Entitlement> {
        let ids = self.account_entitlements.get(&account_id).cloned().unwrap_or_default();
        ids.iter()
            .rev()
            .take(limit.unwrap_or(50).min(100) as usize)
            .map(|id| self.entitlements.get(*id as u32).unwrap().clone())
            .collect()
    }

    /// Entitlements with an id at or above `from_id`, oldest first. The gateway pages through this.
    pub fn list_entitlements(&self, from_id: Option<u64>, limit: Option<u64>) -> Vec<Entitlement> {
        let skip = from_id.unwrap_or(0) as usize;
        self.entitlements.iter().skip(skip).take(limit.unwrap_or(50).min(100) as usize).cloned().collect()
    }
}

impl Contract {
    fn credit(&mut self, account: &AccountId, amount: u128) {
        if amount > 0 {
            let balance = self.balances.get(account).copied().unwrap_or(0);
            self.balances.insert(account.clone(), balance + amount);
        }
    }

    fn internal_place_bid(&mut self, bidder: AccountId, auction_id: u64, units: u64, max_price: u128) -> u32 {
        let now = env::block_timestamp_ms();
        let auction = self.auctions.get_mut(auction_id as u32).unwrap_or_else(|| env::panic_str("Auction not found"));
        require!(
            auction.status == AuctionStatus::Open && now < auction.ends_at_ms,
            "This auction is no longer accepting bids"
        );
        require!((1..=MAX_UNITS).contains(&units), "Quantity must be 1-1,000,000 IU/day");
        require!(max_price >= auction.reserve_price.0, "Bid is below the reserve price");
        let book = self.bids.get_mut(&auction_id).unwrap();
        require!((book.len() as u32) < MAX_TOTAL_BIDS, "This auction's order book is closed to new bids");

        let mut released: Option<(AccountId, u128)> = None;
        if auction.open_bids >= MAX_OPEN_BIDS {
            // The book is full: the newcomer must outprice its weakest bid, which is then returned.
            let weakest = book
                .iter_mut()
                .filter(|b| b.status == BidStatus::Open)
                .min_by(|a, b| a.max_price.0.cmp(&b.max_price.0).then(b.index.cmp(&a.index)))
                .unwrap();
            require!(max_price > weakest.max_price.0, "The order book is full; bid above its lowest price");
            weakest.status = BidStatus::Outbid;
            released = Some((weakest.bidder.clone(), weakest.escrow.0));
            weakest.escrow = U128(0);
            auction.open_bids -= 1;
            auction.open_bid_units -= weakest.units;
            emit("bid_outbid", json!({"auction_id": auction_id, "index": weakest.index, "bidder": weakest.bidder}));
        }

        let escrow = notional(units, auction.term_days, max_price);
        let index = book.len() as u32;
        let bid = Bid {
            auction_id,
            index,
            bidder: bidder.clone(),
            units,
            max_price: U128(max_price),
            escrow: U128(escrow),
            created_at_ms: now,
            status: BidStatus::Open,
            filled_units: 0,
            charged: U128(0),
        };
        emit("bid_placed", json!(bid));
        book.push(bid);
        auction.open_bids += 1;
        auction.open_bid_units += units;

        if let Some((account, amount)) = released {
            self.credit(&account, amount);
        }
        let balance = self.balances.get(&bidder).copied().unwrap_or(0);
        require!(balance >= escrow, "Your balance does not cover this bid's maximum cost");
        self.balances.insert(bidder.clone(), balance - escrow);
        let mut refs = self.account_bids.get(&bidder).cloned().unwrap_or_default();
        refs.push((auction_id, index));
        self.account_bids.insert(bidder, refs);
        index
    }
}

#[cfg(test)]
mod tests;
