# Enclavo contracts (NEAR)

On-chain version of the capacity auction in `gateway/src/market.rs`: bids are escrowed in a
stablecoin, cleared at one uniform price, and winners receive an on-chain capacity entitlement.

| Crate | Purpose |
| --- | --- |
| `auction/` | Auctions, bid escrow, clearing, entitlements, withdrawals |
| `mock-usdc/` | Testnet-only 6-decimal NEP-141 token with an open faucet (`mint`) |

## Testnet deployment

| Account | Role |
| --- | --- |
| `enclavo-auction.testnet` | Auction contract |
| `enclavo-musdc.testnet` | Mock USDC (mUSDC, 6 decimals) |
| `enclavo-ops.testnet` | Operator: publishes auctions, receives proceeds |

## How it works

Amounts are the token's atomic units; with 6 decimals that equals the gateway's micro-USD.
Prices are per IU-day, and a bid escrows `units * term_days * max_price`.

1. The operator calls `publish_auction` with the route, IU/day, term (7, 30 or 90 days), reserve
   price and bidding window.
2. A buyer bids with one `ft_transfer_call` on the token, passing
   `{"auction_id":0,"units":8,"max_price":"1000000"}` as `msg`. A rejected bid returns the transfer.
   An empty `msg` only deposits; `place_bid` then bids from that balance.
3. `cancel_bid` unlocks the escrow any time before the close.
4. After the close anyone calls `clear_auction`. Bids fill in price/time order at one price: the
   reserve when supply covers demand, otherwise the marginal winning bid. Each fill creates an
   `Entitlement`; unspent escrow is unlocked and proceeds are credited to the operator.
5. `withdraw` sends unlocked balance back to the wallet.

## Spending won capacity

The gateway mirrors the contract when `ENCLAVO_AUCTION_CONTRACT` is set (`gateway/src/chain.rs`).
It only makes view calls against finalized blocks, so it holds no chain key.

1. Every entitlement becomes a gateway capacity contract owned by an account tied to the
   winner's NEAR account. No sign-up is involved.
2. The winner creates an API key locally and calls `authorize_key` with its SHA-256 hash. The key
   never touches the chain. Calling again rotates it; `revoke_key` removes it.
3. Within about ten seconds the gateway accepts that key. `GET /v1/contracts` lists the wallet's
   entitlements, and a chat request with `capacity_contract_id` draws on one at no cash cost.

`scripts/use-capacity.sh` runs these steps for a testnet buyer against a local gateway.

Views: `key_hash_of`, `get_config`, `list_auctions`, `get_auction`, `get_bids`, `bids_of`, `balance_of`,
`entitlements_of`, `list_entitlements`. Every state change emits an `EVENT_JSON` log
(`auction_published`, `bid_placed`, `bid_cancelled`, `bid_outbid`, `entitlement_created`,
`auction_cleared`, `key_authorized`, `key_revoked`).

An auction holds at most 100 open bids. When the book is full, a new bid must outprice the weakest
one, which is returned to its owner.

## Limits

- Bids are public on chain; this is not a sealed-bid auction.
- Proceeds are paid to the operator at clearing. There is no delivery-linked release or refund if
  the operator fails to serve the capacity.
- Entitlements are records, not transferable tokens.
- The dashboard still shows only the off-chain auctions; bidding on chain needs a NEAR wallet
  and the CLI for now.
- The contract pays its own storage and has no upgrade or pause path.

## Build and test

Requires Rust with the `wasm32-unknown-unknown` target, a C toolchain and
[`cargo-near`](https://github.com/near/cargo-near).

```sh
cargo near build non-reproducible-wasm --manifest-path auction/Cargo.toml
cargo near build non-reproducible-wasm --manifest-path mock-usdc/Cargo.toml
cargo test                      # unit tests + sandbox end-to-end test (needs the wasm files)
scripts/testnet-demo.sh         # one live 60-second auction on testnet
scripts/use-capacity.sh         # authorize a key and spend the won capacity through the gateway
```
