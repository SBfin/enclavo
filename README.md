# Enclavo

Private inference marketplace with a Next.js dashboard and an OpenAI-compatible Rust gateway.

## Local development

Add your Privy credentials to `.env` (see `.env.example`), then run:

```sh
docker compose up --build -d
docker compose ps
```

| Service | URL | Response |
| --- | --- | --- |
| Dashboard | http://localhost:3001 | Web UI; sign in with Privy |
| Gateway | http://localhost:8088/v1/models | JSON model catalog |
| Readiness | http://localhost:8088/readyz | Database and gateway readiness |
| Metrics | http://localhost:9091/metrics | Prometheus text metrics |
| Mock providers | http://localhost:4000/healthz | `ok` |

The gateway, metrics, and mock root URLs redirect to the endpoints above. They are API services;
the dashboard is the web UI. Postgres is available on `localhost:5433` with database, user, and
password `enclavo`. The dashboard's market catalog and bids are prototypes: its catalog is static
and bids are stored in memory, separately from gateway billing.

All four services start by default. Stop any existing `npm run dev` process using port 3001
before starting the Docker dashboard. The first start installs dashboard dependencies and builds
the Rust binaries, which can take a few minutes. Inspect startup with `docker compose logs -f`.
To run only the backend: `docker compose up --build -d postgres mock gateway`.

The local gateway uses mock inference and mock payments; no provider keys, wallet, or real money
are needed. Its cheapest route deliberately returns 503 so failover can be exercised.
Config changes in `gateway/deploy/local.toml` need `docker compose restart gateway`;
Rust changes need `docker compose up --build -d gateway mock`.

## Verification

```sh
npm ci
npm run lint
npm run build
TEST_DATABASE_ADMIN_URL=postgres://enclavo:enclavo@localhost:5433/postgres cargo test --manifest-path gateway/Cargo.toml --locked
gateway/scripts/smoke.sh
```

The smoke script needs `curl` and `jq`; it verifies mock payments, replay rejection, prepaid credit,
streaming, provider failover, batch billing, and metrics. Tests create isolated Postgres databases.

Stop services with `docker compose down`. Add `-v` only to erase local database and dependency volumes.
For frontend-only development, use `npm run dev`. Gateway deployment configs remain in
`gateway/deploy/` and `gateway/fly.*.toml`; replace their placeholders before deploying.
