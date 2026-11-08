# Enclavo

A Next.js dashboard connected to a Rust inference gateway. The default stack calls NEAR AI;
it does not start fake inference providers or a fake payment facilitator.

## Run real inference

1. Add `NEXT_PUBLIC_PRIVY_APP_ID`, `PRIVY_APP_SECRET`, and a funded `NEAR_AI_API_KEY` to `.env`
   (see `.env.example`). Optionally set `NEAR_AI_MODEL`; the default is `z-ai/glm-5.3-flash`.
2. Start the stack:

   ```sh
   docker compose up --build -d
   docker compose logs gateway
   ```

   At startup, the gateway checks NEAR AI's model catalog for the exact model ID, readiness,
   text output, advertised attestation support, and input/output token prices. Missing credentials
   or an unavailable model stop startup. Prices refresh on restart; there is no silent mock fallback.
3. Issue an operator-funded spending allowance:

   ```sh
   docker compose exec gateway enclavo-admin create-account --credit-usd 1
   ```

   Save the returned `ek_…` API key. The command creates a persistent account, stores only the key's
   hash, and records the grant in Postgres. **This is a spending allowance funded by the operator,
   who pays the NEAR AI invoice; it is not a deposit, transfer, or blockchain payment.** The command
   requires database access and has no public HTTP equivalent.
4. Open http://localhost:3001, sign in, and enter the gateway key in Router. Send a prompt to stream
   a real model response, record token usage, debit credit, and display the charge and remaining balance.
   The gateway key stays in page memory. The NEAR AI provider key stays on the server.

The Market page reads current routes and token prices from the gateway. Capacity auctions and
future-delivery contracts are not implemented, so the UI does not accept bids or claim to fill orders.
Privy sign-in and gateway spending keys are separate credentials.

## Endpoints

| Service | URL |
| --- | --- |
| Dashboard | http://localhost:3001 |
| Gateway catalog | http://localhost:8088/v1/models |
| Gateway mode and funding | http://localhost:8088/v1/status |
| Readiness | http://localhost:8088/readyz |
| Prometheus metrics | http://localhost:9091/metrics |
| Same-origin inference API | http://localhost:3001/api/v1 |

The frontend API proxies models, chat, streaming, account, credit, files, and batch requests to
Rust, preserving bearer tokens, payment headers, status codes, and streams. It has no static
catalog fallback. Postgres is on `localhost:5433` with database, user, and password `enclavo`.

For a real API request, set a funded gateway key in `ENCLAVO_API_KEY`:

```sh
curl http://localhost:3001/api/v1/chat/completions \
  -H "Authorization: Bearer $ENCLAVO_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"enclavo/auto","messages":[{"role":"user","content":"Explain confidential computing."}],"max_tokens":512,"reasoning_effort":"low","stream":true}'
```

`gateway/scripts/live-smoke.sh` sends two **billable** NEAR AI requests and checks JSON, streaming,
provider identity, charges, and the resulting account balance. Set `BASE=http://localhost:3001/api`
to include the frontend proxy in that test. This script never simulates a payment.

## Verification and test-only mocks

```sh
npm run lint
npx next typegen
npx tsc --noEmit
node --test tests/gateway-proxy.test.mjs
docker compose exec -T web npm run build
TEST_DATABASE_ADMIN_URL=postgres://enclavo:enclavo@localhost:5433/postgres cargo test --manifest-path gateway/Cargo.toml --locked
```

Mock inference and fake settlement exist only for explicit regression tests:

```sh
docker compose -f compose.yaml -f compose.test.yaml up --build -d
gateway/scripts/smoke.sh
# Restore live inference and remove the test-only mock container:
docker compose up --build -d --remove-orphans
```

In test mode `/v1/status` and the UI explicitly identify simulated inference. The test override
exposes mocks on port 4000 and uses a separate database volume, so test balances cannot fund live
requests. `curl` and `jq` are required by the smoke scripts. Previous `pgdata` volumes are retained
but are not used by either new mode.

## Current limits

NEAR AI advertises hardware attestation for the selected model, but Enclavo does not yet verify
hardware quotes, TLS bindings, or response signatures. It reports proof verification as unavailable
and does not mark the live route as cryptographically verified. There are no fabricated benchmark
scores. Image-specific pricing and encrypted client-to-model transport are not implemented.

The default live preset uses prepaid operator allowances; it does not advertise blockchain settlement.
The gateway includes x402 integration code and custom deployment templates, but real on-chain
settlement has not been validated. Configuring those templates requires real recipients, supported
facilitator schemes, credentials, and a separate funded-client test.

`docker compose down` stops services. Add `-v` only to erase database and frontend dependency volumes.
For frontend-only development, run `npm run dev`; it uses the gateway at `http://127.0.0.1:8088` unless
`ENCLAVO_GATEWAY_URL` is set. Stop any existing server on port 3001 before starting Docker.
