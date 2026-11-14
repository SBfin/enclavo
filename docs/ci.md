# CI and staging deploys

`.github/workflows/staging.yml` runs on every push and pull request.

| Job | What it runs |
| --- | --- |
| Checks / web | Node 24: `npm ci`, `eslint`, `next typegen` + `tsc --noEmit`, `node --test tests/*.test.mjs`, `npm run build` (testnet) |
| Checks / contracts | Stable Rust: builds both wasm files with `cargo near`, then `cargo test` (unit + sandbox end-to-end) |
| Checks / gateway | Stable Rust: `cargo test` against a Postgres 17 service container |
| Deploy staging | Only on pushes to `main`, after all checks pass: `flyctl deploy --remote-only` for `gateway/fly.staging.toml`, then `fly.staging.toml` |

Deploys share the `fly-staging` concurrency group, so they never overlap; a newer push waits for the
running deploy to finish. The gateway's release command runs database migrations before the web
app is updated.

What stays manual:

- Production (`fly.production.toml`, `gateway/fly.production.toml`).
- NEAR contract upgrades: they need wallet keys, which never go into GitHub.
- Fly secrets (`fly secrets import`) and infrastructure; see the README's staging section.

## One-time setup: Fly deploy tokens

A Fly deploy token is limited to one app, so staging needs one per app. Run these with your own
Fly login (`fly auth login`):

```sh
fly tokens create deploy -a enclavo-gateway-staging -n github-actions
fly tokens create deploy -a enclavo-web-staging -n github-actions
```

Each command prints a token starting with `FlyV1 `. Copy the whole string, including that prefix,
and add it under GitHub → **Settings → Secrets and variables → Actions → New repository secret**:

| Secret | Value |
| --- | --- |
| `FLY_API_TOKEN_GATEWAY` | Token for `enclavo-gateway-staging` |
| `FLY_API_TOKEN_WEB` | Token for `enclavo-web-staging` |

Or with the GitHub CLI, which prompts for the value so it stays out of shell history:

```sh
gh secret set FLY_API_TOKEN_GATEWAY
gh secret set FLY_API_TOKEN_WEB
```

The deploy job uses the `staging` GitHub environment, which GitHub creates on the first deploy.
You can add protection rules there (for example required reviewers) without changing the
workflow. To revoke a token, list it with `fly tokens list -a APP` and run `fly tokens revoke ID`.
