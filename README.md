# Enclavo

A capacity-auction market backed by a Rust inference gateway and NEAR AI. Market is the default
page, Playground is the second tab, and API Keys manages application access to the same account.

## Run locally

Add `NEXT_PUBLIC_PRIVY_APP_ID`, `PRIVY_APP_SECRET`, and a funded `NEAR_AI_API_KEY` to `.env`
(see `.env.example`), then run:

```sh
docker compose up --build -d
```

Open http://localhost:3001. Anyone can browse Market; placing bids, using Playground, and managing
keys require Privy sign-in. The default stack uses live NEAR AI inference, not test providers.

All ready confidential chat models with supported flat token pricing are discovered from NEAR AI
at startup. Their real IDs, descriptions, context windows, capabilities, and separate input/output
prices populate Market and Playground. Embedding, reranking, audio/image-only, unavailable,
non-confidential, and unsupported tier-priced models are excluded. `NEAR_AI_MODEL` selects a
preferred default; optional `NEAR_AI_MODELS` restricts the catalog to a comma-separated allowlist.
Restart the gateway to refresh provider availability and prices.

## Customer flow

1. Browse models and published capacity auctions in **Market**.
2. Sign in and open **API Keys**. Connect an existing operator-issued spending key before creating
   a new account, or create a named key to initialize an empty account. Keys are displayed in full
   once, stored only as hashes, and can be individually revoked. Every key shares the account's balance.
3. Obtain prepaid credit from the operator. API Keys shows the account ID to use when requesting funds.
4. Bid for IU/day over a 7-, 30-, or 90-day term. The full maximum cost is reserved atomically.
   Cancellation before close returns the reserve. Closing auctions clear in price/time priority at
   one price: reserve price when supply covers demand, otherwise the marginal winning bid. Partial
   fills create proportional contracts and unused reserves are released.
5. Won capacity appears in **Your capacity**. Open it in **Playground**, choose the purchased contract
   under **Spend from**, and send a prompt. Requests consume daily capacity without a second cash charge.
   Playground can also consume ordinary prepaid account credit while signed in; no key needs pasting.
6. For an application, create a key and use the API endpoint shown in **API Keys**. List owned contracts
   with `GET /api/v1/contracts`; include `capacity_contract_id` in a chat request to consume one.

One IU is one million output-token equivalents per day. Input consumption uses the model's contracted
input/output ratio. Capacity resets every 24 hours from the contract's start; unused capacity does not
roll over. Ownership, remaining quota, expiry, concurrent reservations, and failed-request recovery are
enforced in Postgres for both Playground and external API requests.

## Operator setup

Only the operator, with database access, can issue credit or publish capacity. These commands are not
public HTTP endpoints. Credit is an operator-funded spending allowance; the operator pays the NEAR AI
invoice. It is not a deposit, token transfer, or settled blockchain payment.

Create an allowance and return its initial spending key:

```sh
docker compose exec gateway enclavo-admin create-account --credit-usd 1
```

Add credit to an existing signed-in customer's account:

```sh
docker compose exec gateway enclavo-admin credit-account --account-id ACCOUNT_UUID --credit-usd 10
```

Publish an auction only for capacity the operator commits to fulfill. For example, this offers one
IU/day for seven days, with a $3.50 minimum total purchase, and closes after 24 hours:

```sh
docker compose exec gateway enclavo-admin publish-auction \
  --model z-ai/glm-5.3-flash --units 1 --days 7 \
  --min-price-usd 0.50 --duration-seconds 86400
```

The reserve must cover the model's current token cost. No supply, buyer balances, bids, or fills are
seeded automatically. A background worker clears closed auctions; the database transaction commits
charges, contracts, and refunds together. Previously published offers remain operator commitments
if provider prices change. There is no automatic supplier purchase or GPU reservation upstream.

## API and services

| Service | URL |
| --- | --- |
| Market | http://localhost:3001/dashboard/market |
| Playground | http://localhost:3001/dashboard/playground |
| API Keys | http://localhost:3001/dashboard/api-keys |
| Same-origin inference API | http://localhost:3001/api/v1 |
| Gateway catalog / mode | http://localhost:8088/v1/models · http://localhost:8088/v1/status |
| Public auction listings | http://localhost:8088/v1/market/auctions |
| Readiness / metrics | http://localhost:8088/readyz · http://localhost:9091/metrics |

Chat, streaming, account, contracts, files, and batches use gateway bearer keys. Dashboard account,
key, bid, and Playground operations verify the Privy token in Next.js and forward only the verified
identity over an authenticated server-to-server channel. A dedicated `ENCLAVO_CONTROL_TOKEN` can be
configured; otherwise a domain-separated token is derived from `PRIVY_APP_SECRET` server-side. Browser
requests cannot set the trusted identity or access the channel secret. Anonymous account operations fail
closed. Customer accounts and all market state persist across restarts.

Postgres is on `localhost:5433` with database, user, and password `enclavo`. Frontend-only development
uses `npm run dev` and `http://127.0.0.1:8088` unless `ENCLAVO_GATEWAY_URL` is set. `/dashboard/router`
redirects to Playground; `/` and `/dashboard` redirect to Market.

## Verification

```sh
npm run lint
npx next typegen
npx tsc --noEmit
node --test tests/gateway-proxy.test.mjs
docker compose exec -T web npm run build
TEST_DATABASE_ADMIN_URL=postgres://enclavo:enclavo@localhost:5433/postgres cargo test --manifest-path gateway/Cargo.toml --locked
```

Tests cover key ownership/revocation, auction reservations, uniform clearing, partial fills, refunds,
concurrent budgets, contract consumption through both interfaces, expiry, daily resets, and failed
providers. They use isolated databases and explicit test providers, not billable inference.

For the original test-only payment smoke suite:

```sh
docker compose -f compose.yaml -f compose.test.yaml up --build -d
gateway/scripts/smoke.sh
docker compose up --build -d --remove-orphans
```

Live and test modes use separate database volumes. The test override exposes mocks on port 4000.
`gateway/scripts/live-smoke.sh` instead makes two billable NEAR AI requests; it requires a funded
`ENCLAVO_API_KEY` and must be run only with an approved budget. Set `BASE=http://localhost:3001/api`
to include the frontend proxy. Smoke scripts need `curl` and `jq`.

## Current limits

The provider advertises TEE capabilities, but Enclavo does not yet verify hardware quotes, TLS
bindings, or response signatures. It reports verification as unavailable. There are no invented
benchmark scores or benchmark-normalized cross-model IU conversion; the capacity units are model-specific. Image-specific pricing and encrypted client-to-model transport are not implemented.
The live preset uses operator-issued credit; public self-service deposits and real on-chain settlement
are not enabled. Custom x402/deployment templates need separate funded-client validation.

`docker compose down` stops services. Add `-v` only to erase data and dependency volumes. Older `pgdata`
volumes are retained but are not used by the separated live/test modes.
