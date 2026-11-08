#!/usr/bin/env bash
# Runs every gateway flow against a running local stack (`docker compose up --build -d` from the
# repo root) and stops at the first thing that doesn't behave. Needs curl and jq.
#
#   gateway/scripts/smoke.sh                     # against http://localhost:8088
#   BASE=https://… METRICS=… gateway/scripts/smoke.sh

set -euo pipefail

BASE=${BASE:-http://localhost:8088}
METRICS=${METRICS:-http://localhost:9091/metrics}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

pass() { printf '  \033[32mok\033[0m  %s\n' "$1"; }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; [ -n "${2:-}" ] && printf '       %s\n' "$2"; exit 1; }
step() { printf '\n%s\n' "$1"; }

# request METHOD PATH [curl args…] -> status in $STATUS, headers in $TMP/h, body in $TMP/b
request() {
  local method=$1 path=$2
  shift 2
  STATUS=$(curl -sS -X "$method" "$BASE$path" -D "$TMP/h" -o "$TMP/b" -w '%{http_code}' "$@")
}
header() { grep -i "^$1:" "$TMP/h" | head -1 | cut -d' ' -f2- | tr -d '\r'; }
body() { jq -r "$1" "$TMP/b"; }
expect_status() { [ "$STATUS" = "$1" ] || fail "$2: expected HTTP $1, got $STATUS" "$(head -c 300 "$TMP/b")"; }
b64decode() { base64 -d 2>/dev/null || base64 -D; }

# A payment answering an offer. The local facilitator accepts any signature except "bad".
# Each one carries a fresh nonce, as a real signed authorization would.
payment_for() {
  jq -cn --argjson offer "$1" --arg sig "$2" --arg nonce "$(nonce)" \
    '{x402Version: 2, accepted: $offer,
      payload: {signature: $sig, authorization: {from: "0x00000000000000000000000000000000000000aa", nonce: $nonce}}}' |
    base64 | tr -d '\n'
}
offer() { header payment-required | b64decode | jq -c '.accepts[0]'; }
nonce() { echo "smoke-$(date +%s)-$RANDOM$RANDOM"; }

CHAT='{"model":"glm-5.3-flash","messages":[{"role":"user","content":"Hello from the smoke test"}],"max_tokens":64}'
JSON=(-H 'content-type: application/json')

step "Health"
request GET / -L
expect_status 200 "gateway home redirects to model list"
pass "gateway home opens the model list"
request GET /readyz
expect_status 200 "readiness"
pass "gateway is ready (database reachable)"
request GET /v1/models
expect_status 200 "model list"
pass "$(body '.data | length') models listed: $(body '[.data[].id] | join(", ")')"

step "Pay per request (x402 upto)"
request POST /v1/chat/completions "${JSON[@]}" -d "$CHAT"
expect_status 402 "unpaid request"
OFFER=$(offer)
[ "$(jq -r .scheme <<<"$OFFER")" = upto ] || fail "offer should use the upto scheme" "$OFFER"
pass "unpaid request gets 402, offering up to $(jq -r .amount <<<"$OFFER") atomic USDC on $(jq -r .network <<<"$OFFER")"

PAYMENT=$(payment_for "$OFFER" "$(nonce)")
request POST /v1/chat/completions "${JSON[@]}" -H "payment-signature: $PAYMENT" -d "$CHAT"
expect_status 200 "paid request"
[ "$(header x-enclavo-route)" = "glm-5.3-flash@mock-fast" ] || fail "expected failover to mock-fast, got $(header x-enclavo-route)"
pass "answered by $(header x-enclavo-route) after the cheaper mock-down route failed"
COST=$(header x-enclavo-cost-micro-usd)
RECEIPT=$(header payment-response | b64decode)
[ "$(jq -r .success <<<"$RECEIPT")" = true ] || fail "settlement receipt" "$RECEIPT"
pass "charged $COST micro-USD of the $(jq -r .amount <<<"$OFFER") authorized; tx $(jq -r .transaction <<<"$RECEIPT" | cut -c1-18)…"
pass "reply: $(body '.choices[0].message.content' | cut -c1-70)…"

request POST /v1/chat/completions "${JSON[@]}" -H "payment-signature: $PAYMENT" -d "$CHAT"
expect_status 409 "replayed payment"
pass "replaying the same payment is refused (409)"

request POST /v1/chat/completions "${JSON[@]}" -H "payment-signature: $(payment_for "$OFFER" bad)" -d "$CHAT"
expect_status 402 "invalid signature"
pass "a payment the facilitator rejects gets 402 and runs nothing"

step "Prepaid credit (x402 exact top-up)"
request POST /v1/credits/topup "${JSON[@]}" -d '{"amount_usd":"5"}'
expect_status 402 "unpaid top-up"
TOPUP_OFFER=$(offer)
request POST /v1/credits/topup "${JSON[@]}" -H "payment-signature: $(payment_for "$TOPUP_OFFER" "$(nonce)")" -d '{"amount_usd":"5"}'
expect_status 200 "paid top-up"
KEY=$(body .api_key)
[ "$(body .balance_micro_usd)" = 5000000 ] || fail "balance after top-up" "$(cat "$TMP/b")"
pass "topped up \$5, got API key ${KEY:0:10}…"
AUTH=(-H "authorization: Bearer $KEY")

request POST /v1/chat/completions "${JSON[@]}" "${AUTH[@]}" -d "$CHAT"
expect_status 200 "prepaid request"
PREPAID_COST=$(header x-enclavo-cost-micro-usd)
request GET /v1/account "${AUTH[@]}"
[ "$(body .balance_micro_usd)" = $((5000000 - PREPAID_COST)) ] || fail "balance after a prepaid request" "$(cat "$TMP/b")"
[ "$(body .held_micro_usd)" = 0 ] || fail "nothing should stay held" "$(cat "$TMP/b")"
pass "prepaid request cost $PREPAID_COST; balance $(body .balance_micro_usd), nothing held"

step "Streaming"
STREAM=$(jq -c '.stream = true | .model = "glm-5.3-max"' <<<"$CHAT")
curl -sS -N "$BASE/v1/chat/completions" "${JSON[@]}" "${AUTH[@]}" -d "$STREAM" >"$TMP/stream"
grep -q '^data: \[DONE\]' "$TMP/stream" || fail "stream should end with [DONE]" "$(tail -5 "$TMP/stream")"
TEXT=$(grep '^data: {' "$TMP/stream" | sed 's/^data: //' | jq -rj '.choices[0].delta.content // empty')
STREAM_COST=$(grep '^: enclavo-cost-micro-usd' "$TMP/stream" | awk '{print $3}')
[ -n "$STREAM_COST" ] || fail "stream should carry its cost before [DONE]"
pass "streamed $(grep -c '^data: {' "$TMP/stream") chunks from the slow route, cost $STREAM_COST: ${TEXT:0:60}…"

step "Batch (30% off)"
for i in 1 2 3; do
  jq -cn --arg i "$i" --argjson body "$CHAT" '{custom_id: ("req-" + $i), method: "POST", url: "/v1/chat/completions", body: $body}'
done >"$TMP/batch.jsonl"
request POST /v1/files "${AUTH[@]}" -F purpose=batch -F "file=@$TMP/batch.jsonl"
expect_status 200 "file upload"
FILE_ID=$(body .id)
request POST /v1/batches "${JSON[@]}" "${AUTH[@]}" \
  -d "{\"input_file_id\":\"$FILE_ID\",\"endpoint\":\"/v1/chat/completions\",\"completion_window\":\"24h\"}"
expect_status 200 "batch create"
BATCH_ID=$(body .id)
pass "batch $BATCH_ID created; $(body .enclavo.ceiling_micro_usd) micro-USD held as its ceiling"
for _ in $(seq 1 60); do
  request GET "/v1/batches/$BATCH_ID" "${AUTH[@]}"
  [ "$(body .status)" = completed ] && break
  sleep 0.5
done
[ "$(body .status)" = completed ] || fail "batch did not complete" "$(cat "$TMP/b")"
pass "completed: $(body '.request_counts | "\(.completed)/\(.total) ok"'), charged $(body .enclavo.charged_micro_usd) (live would be $((3 * PREPAID_COST)))"
request GET "/v1/files/$(body .output_file_id)/content" "${AUTH[@]}"
[ "$(wc -l <"$TMP/b" | tr -d ' ')" = 3 ] || fail "output file should have 3 lines" "$(cat "$TMP/b")"
pass "output file has 3 answers"
request GET /v1/account "${AUTH[@]}"
[ "$(body .held_micro_usd)" = 0 ] || fail "the batch hold should be released" "$(cat "$TMP/b")"
pass "hold released; final balance $(body .balance_micro_usd) micro-USD over $(body .requests) requests"

step "Operations"
curl -fsSL "${METRICS%/metrics}/" >"$TMP/metrics" || fail "metrics home redirect is broken"
curl -sS "$METRICS" >"$TMP/metrics" || fail "metrics endpoint unreachable at $METRICS"
grep -q '^enclavo_settlements_total' "$TMP/metrics" || fail "settlement metrics missing"
pass "metrics: $(grep -c '^enclavo_' "$TMP/metrics") enclavo series on the private port"
request GET /metrics
[ "$STATUS" = 404 ] || fail "/metrics must not be public (got $STATUS)"
pass "/metrics is not served on the public port"

printf '\n\033[32mAll flows passed.\033[0m\n'
