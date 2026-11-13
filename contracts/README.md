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

Publish an auction as the operator (amounts in micro-USD, duration in milliseconds):

```sh
near contract call-function as-transaction enclavo-auction.testnet publish_auction json-args \
  '{"route_id":"z-ai/glm-5.3-flash@near-ai","units":100,"term_days":7,"reserve_price":"500000","tokens_per_iu":1000000,"prompt_weight_bps":1600,"duration_ms":86400000}' \
  prepaid-gas '30 Tgas' attached-deposit '0 NEAR' sign-as enclavo-ops.testnet \
  network-config testnet sign-with-legacy-keychain send
```

`route_id` and `tokens_per_iu` must match the gateway's route for the model (`GET /v1/models`),
and `prompt_weight_bps` is its input price divided by its output price, in basis points. The
contract does not know the catalog. The operator key is held by the
person who deployed the contracts, not stored in this repository.

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

The dashboard's **On-chain** page (`/dashboard/onchain`) does all of this from a NEAR wallet:
connect, get test mUSDC, bid, cancel, settle, withdraw and authorize a key. The gateway's
`/v1/status` tells the page which contract and RPC to use. Wallets come from
[NEAR Connect](https://github.com/hot-dao/near-connect); on testnet that includes Meteor, Intear
and a NEAR CLI option that prints the command to sign.

The page was exercised end to end on testnet with the Intear web wallet approving each
transaction, and with the NEAR CLI option. Meteor and Nightly are offered by the connector but
have not been tried.

`scripts/use-capacity.sh` runs the key steps for a testnet buyer against a local gateway.

Views: `key_hash_of`, `get_config`, `list_auctions`, `get_auction`, `get_bids`, `bids_of`, `balance_of`,
`entitlements_of`, `list_entitlements`. Every state change emits an `EVENT_JSON` log
(`deposit`, `withdraw`, `withdraw_failed`, `auction_published`, `bid_placed`, `bid_cancelled`,
`bid_outbid`, `auction_cleared`, `key_authorized`, `key_revoked`). Clearing emits a single
`auction_cleared` event that names the range of entitlement ids it created.

An auction holds at most 100 open bids. When the book is full, a new bid must outprice the weakest
one, which is returned to its owner.

## Scope and roadmap

This is a testnet MVP. What it deliberately leaves for later:

- **Sealed bids.** Bids are public on chain today. The plan is to seal and clear them inside the
  same TEEs that serve the inference.
- **Delivery-linked payout.** The operator is credited at clearing. Next step: proceeds vest over
  the term and the unserved part is refundable.
- **Tradable capacity.** Entitlements are records owned by the winner, not transferable tokens yet.
- **Market rules.** Minimum bid size, a cancel freeze before the close and other anti-manipulation
  rules are not enforced; the clearing rule matches the off-chain market one to one.
- **Operations.** The contract pays its own storage and has no upgrade or pause switch.

Every auction is bounded so it always settles in one transaction: 100 open bids, 400 recorded bids.
The worst case (100 winners out of 400 recorded bids) burns about 52 Tgas of the 300 Tgas limit.

The dashboard's Market page runs the off-chain auctions; the contract has its own On-chain page.

## Build and test

Requires Rust with the `wasm32-unknown-unknown` target, a C toolchain and
[`cargo-near`](https://github.com/near/cargo-near).

```sh
cargo near build non-reproducible-wasm --manifest-path auction/Cargo.toml
cargo near build non-reproducible-wasm --manifest-path mock-usdc/Cargo.toml
cargo test                      # unit tests + sandbox end-to-end tests (need the wasm files; ~4 min)
scripts/testnet-demo.sh         # one live 60-second auction on testnet
scripts/use-capacity.sh         # authorize a key and spend the won capacity through the gateway
```
