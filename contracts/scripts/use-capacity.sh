#!/usr/bin/env bash
# Spends capacity won on chain: the wallet authorizes a fresh gateway key by its hash, then that
# key lists the wallet's entitlements and sends one prompt against the first active one.
# Needs near-cli-rs with the buyer's key in the legacy keychain, curl, jq and openssl.
set -euo pipefail

BUYER=${BUYER:-enclavo-alice.testnet}
AUCTION=${AUCTION:-enclavo-auction.testnet}
BASE=${BASE:-http://localhost:8088}

key="ek_$(openssl rand -hex 32)"
hash=$(printf %s "$key" | openssl dgst -sha256 -r | cut -d' ' -f1)
echo "== $BUYER authorizes key hash $hash on $AUCTION"
near contract call-function as-transaction "$AUCTION" authorize_key json-args "{\"key_hash\":\"$hash\"}" \
  prepaid-gas '30 Tgas' attached-deposit '0 NEAR' sign-as "$BUYER" network-config testnet \
  sign-with-legacy-keychain send 2>&1 | grep -E 'Transaction ID|rror|anic' || true

echo "== waiting for the gateway to mirror it"
for _ in $(seq 30); do
  contracts=$(curl -s -H "Authorization: Bearer $key" "$BASE/v1/contracts")
  [ "$(jq -r '.contracts | length' <<<"$contracts" 2>/dev/null || echo 0)" != 0 ] && break
  sleep 3
done
jq '.contracts[] | {id, chain_entitlement_id, route_id, units, active, daily_tokens, remaining_tokens}' <<<"$contracts"

id=$(jq -r '[.contracts[] | select(.active)][0].id' <<<"$contracts")
route=$(jq -r '[.contracts[] | select(.active)][0].route_id' <<<"$contracts")
echo "== one prompt on $route, paid from entitlement $id"
curl -s -D /dev/stderr -H "Authorization: Bearer $key" -H 'content-type: application/json' "$BASE/v1/chat/completions" \
  -d "{\"model\":\"$route\",\"capacity_contract_id\":\"$id\",\"max_tokens\":40,\"messages\":[{\"role\":\"user\",\"content\":\"Reply with exactly: capacity delivered\"}]}" \
  2> >(grep -i 'x-enclavo-cost' >&2) | jq '{model, reply: .choices[0].message.content, usage}'
curl -s -H "Authorization: Bearer $key" "$BASE/v1/contracts" | jq --arg id "$id" '.contracts[] | select(.id == $id) | {used_tokens, remaining_tokens}'
