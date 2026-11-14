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

## On-chain auctions (NEAR testnet)

`contracts/` holds a NEAR version of the auction. It runs beside the off-chain Market above; the
two do not share auctions, bids or balances.

- **Contract** (`enclavo-auction.testnet`): bids are escrowed in a stablecoin, cleared with the
  same uniform-price rule, and each winner receives an on-chain entitlement. Unspent escrow is
  withdrawable. The settlement token is a testnet mock (`enclavo-musdc.testnet`) with an open faucet.
- **Gateway mirror** (`gateway/src/chain.rs`): with `ENCLAVO_AUCTION_CONTRACT` set, the gateway
  reads finalized contract state every ten seconds and turns entitlements into capacity
  contracts. A wallet authorizes an API key by publishing its SHA-256 hash on chain; the gateway
  then serves that wallet's capacity to the key. No Enclavo account or Privy sign-in is involved.
- **On-chain page** (`/dashboard/onchain`): connect a NEAR wallet, get test mUSDC, bid, cancel,
  settle a closed auction, withdraw, and authorize an API key.

Docker Compose and the Fly staging manifest point at the testnet deployment by default; set
`ENCLAVO_AUCTION_CONTRACT=` (empty) in `.env` to run without the chain. Production leaves it unset.
The operator publishes on-chain auctions with the NEAR CLI, not `enclavo-admin`. Contract
interface, scripts and limits are in [contracts/README.md](contracts/README.md).

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
| On-chain market | http://localhost:3001/dashboard/onchain |
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
node --test tests/*.test.mjs
docker compose exec -T web npm run build
TEST_DATABASE_ADMIN_URL=postgres://enclavo:enclavo@localhost:5433/postgres cargo test --manifest-path gateway/Cargo.toml --locked
```

The contracts have their own unit and sandbox tests; see [contracts/README.md](contracts/README.md).
`gateway/tests/chain.rs` covers the mirror against a stand-in NEAR RPC.

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

## Staging deployment on Fly.io

Deploy two applications in Frankfurt (`fra`) and a separate managed PostgreSQL cluster in the
same organization/region. The public Next.js application serves Market, Playground, API Keys and
`/api/v1`. It reaches the Rust gateway over Fly's private IPv6 network; only the gateway holds
the provider key and database credentials. NEAR AI runs the models; no GPU deployment is needed.
The gateway stays running for auction clearing, batches and recovery. Its API, metrics and database
have no public service ports. See [Fly private networking](https://fly.io/docs/networking/private-networking/)
and [managed PostgreSQL](https://fly.io/mpg/).

The templates are `fly.staging.toml` for web and `gateway/fly.staging.toml` for the gateway.
`fly.production.toml` and its gateway counterpart use separate production app names. Start with
staging; use a separate Privy app, provider credential, control token and database for production.
These files provision nothing until deployment commands are run. Fly infrastructure and NEAR usage
are billable; no auction is published and no customer credit is issued during deployment.

### Rehearse the production images locally

Set `STAGING_CONTROL_TOKEN` in `.env` to a new random value (`openssl rand -hex 32`), alongside
the existing Privy and NEAR configuration. Add `http://localhost:3002` to the Privy app's allowed
origins to exercise sign-in. Then run:

```sh
docker compose -f compose.staging.yaml up --build --wait --wait-timeout 180
node scripts/check-staging.mjs http://localhost:3002
```

This starts the production standalone web image at `http://localhost:3002`, with no source mounts,
runtime package installation or development server. Only web has a host port. The rehearsal uses
its own `staging_pgdata` volume, never the development account ledger. Startup reads NEAR's public
model catalog; the checks make no inference requests. To stop it while retaining data:

```sh
docker compose -f compose.staging.yaml down
```

### Configure and deploy staging

1. Install [flyctl](https://fly.io/docs/flyctl/install/) and sign in with `fly auth login`.
   Select the organization that will own billing. The following steps create paid resources.
2. Choose globally unique web/gateway names. Update `app` in both staging manifests,
   `ENCLAVO_GATEWAY_URL` in the web manifest, and `ENCLAVO_PUBLIC_URL` in the gateway manifest.
   Use the resulting public HTTPS origin in the staging Privy app's allowed origins.
3. Create the two apps and managed database. Replace names and `YOUR_ORG` below with your choices:

```sh
fly apps create enclavo-web-staging --org YOUR_ORG
fly apps create enclavo-gateway-staging --org YOUR_ORG
fly mpg create --org YOUR_ORG
```

Choose Frankfurt and the intended database plan in the prompt. Retrieve the database's **direct**
TLS connection string from its connection settings. Set that as `DATABASE_URL` on the gateway.
The automatic `fly mpg attach` URL is pooled; do not use a transaction-pooler connection here.
SQLx migrations take session-level advisory locks, and both startup and the operator CLI run them.
The application's own pool is capped at ten connections per gateway instance.

Create two local secret files outside the repository, restrict them with `chmod 600`, and populate:

| Variable | Web secret file | Gateway secret file |
| --- | --- | --- |
| `NEXT_PUBLIC_PRIVY_APP_ID` | Staging public app ID | — |
| `PRIVY_APP_SECRET` | Staging Privy secret | — |
| `ENCLAVO_CONTROL_TOKEN` | New random token | Same token |
| `NEAR_AI_API_KEY` | — | Funded staging provider key |
| `DATABASE_URL` | — | Direct managed PostgreSQL TLS URL |

Use `KEY=value` format, without `export`. Import only the appropriate file into each app; do not
upload the development `.env` wholesale. Importing with `--stage` avoids premature restarts:

```sh
fly secrets import --stage --app enclavo-web-staging < /secure/path/web.env
fly secrets import --stage --app enclavo-gateway-staging < /secure/path/gateway.env
fly config validate --strict -c fly.staging.toml
```

Deploy the gateway first, from its build directory:

```sh
cd gateway
fly config validate --strict -c fly.staging.toml
fly deploy -c fly.staging.toml --ha=false --no-public-ips
cd ..
fly deploy -c fly.staging.toml --ha=false --build-arg NEXT_PUBLIC_PRIVY_APP_ID=YOUR_STAGING_PRIVY_APP_ID
```

The public Privy app ID is required at **build time** and must match the web runtime configuration;
changing it requires a rebuild. No secret belongs in build arguments. The gateway release command
runs `enclavo-gateway --migrate` before replacing the service and stops deployment on failure.
Startup also checks migrations. Use additive, backward-compatible migrations while old and new
versions overlap. Both services run as non-root users; the gateway binds `[::]:8080` so private
IPv6 requests and IPv4 health probes reach the same listener.

Allocate public addresses **only for web** if they are not already present:

```sh
fly ips allocate-v6 --app enclavo-web-staging
fly ips allocate-v4 --shared --app enclavo-web-staging
fly checks list --app enclavo-gateway-staging
fly checks list --app enclavo-web-staging
node scripts/check-staging.mjs https://enclavo-web-staging.fly.dev
```

Do not add `http_service`, public service ports or public addresses to the gateway. Fly's top-level
gateway checks report health but do not filter `.internal` DNS traffic. The web service's readiness
probe checks the gateway's database readiness before accepting public traffic. This initial pilot
uses one instance of each app; it does not promise uninterrupted deployments or machine failover.
After changing names/domains, update all corresponding URLs and repeat the checks.

### Verify and recover

The read-only smoke command checks pages, assets, model discovery, readiness, public auctions and
anonymous rejection of private account operations. Then verify Privy sign-in, create/copy/revoke a
key, and confirm that the revoked key is rejected. Paid inference and actual auction publication
are separate operator actions. Do not open paid bidding until the funded customer flow has passed.

Keep the previous web/gateway image references from `fly releases --image` for rollback. Redeploy a
known-good image with `fly deploy -c fly.staging.toml --image IMAGE_REFERENCE` from the matching
directory. The web image retains its original public Privy app ID. Image rollback does not undo
database migrations; restore data separately when needed.

Before inviting customers, confirm managed backups and perform a restore drill into a **new**
cluster using `fly mpg restore` (see [backup/restore commands](https://fly.io/docs/flyctl/mpg/)).
Use `fly mpg connect` on the restored cluster and compare account balances, bids, contracts and
usage with read-only queries. Do not start the normal gateway against a recovery copy: its workers
resume batches and auction settlement automatically. Never overwrite the live ledger during a drill.
Cloud backup recovery, DNS, private networking and live Privy sign-in require validation after
actual provisioning. Platform-side `fly config validate --strict` also requires an authenticated
Fly account; local manifest parsing is not a substitute for that check.

## Current limits

The provider advertises TEE capabilities, but Enclavo does not yet verify hardware quotes, TLS
bindings, or response signatures. It reports verification as unavailable. There are no invented
benchmark scores or benchmark-normalized cross-model IU conversion; the capacity units are model-specific. Image-specific pricing and encrypted client-to-model transport are not implemented.
The off-chain Market uses operator-issued credit; it has no public self-service deposits. On-chain
settlement exists only on NEAR testnet with a mock token, and the operator is paid at clearing with
no refund path if capacity is not delivered. Custom x402 templates need separate funded-client validation.

`docker compose down` stops services. Add `-v` only to erase data and dependency volumes. Older `pgdata`
volumes are retained but are not used by the separated live/test modes.
