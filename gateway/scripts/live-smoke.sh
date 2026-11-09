#!/usr/bin/env bash
# Real-provider check. Never creates test payments; requires an explicitly funded gateway account.
set -euo pipefail
BASE=${BASE:-http://localhost:8088}
: "${ENCLAVO_API_KEY:?Set ENCLAVO_API_KEY to a funded gateway account key}"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
AUTH=(-H "authorization: Bearer $ENCLAVO_API_KEY")
JSON=(-H 'content-type: application/json')
curl -fsS "$BASE/v1/status" > "$WORK/status"
jq -e '.mode == "live" and (.providers | index("near-ai") != null)' "$WORK/status" >/dev/null
curl -fsS "$BASE/v1/models" > "$WORK/models"
MODEL=$(jq -r '.data[] | select(.owned_by == "near-ai") | .id' "$WORK/models" | head -1)
[ -n "$MODEL" ] || { echo 'No live NEAR AI route'; exit 1; }
curl -fsS "${AUTH[@]}" "$BASE/v1/account" > "$WORK/before"
CHAT=$(jq -cn --arg model "$MODEL" '{model:$model,messages:[{role:"user",content:"Reply with one short sentence explaining what an inference gateway does."}],max_tokens:128,reasoning_effort:"low"}')
curl -fsS --max-time 180 "${AUTH[@]}" "${JSON[@]}" -D "$WORK/headers" -d "$CHAT" "$BASE/v1/chat/completions" > "$WORK/answer"
jq -e '.choices[0].message.content | type == "string" and length > 0' "$WORK/answer" >/dev/null
grep -qi '^x-enclavo-provider: near-ai' "$WORK/headers"
COST=$(awk 'tolower($1)=="x-enclavo-cost-micro-usd:" {gsub("\r", "", $2); print $2}' "$WORK/headers")
[ -n "$COST" ] && [ "$COST" -gt 0 ]
curl -fsS --max-time 180 -N "${AUTH[@]}" "${JSON[@]}" -d "$(jq -c '.stream=true' <<< "$CHAT")" "$BASE/v1/chat/completions" > "$WORK/stream"
grep -q '^data: \[DONE\]' "$WORK/stream"
STREAM_COST=$(awk '/^: enclavo-cost-micro-usd / {print $3}' "$WORK/stream" | tr -d '\r')
[ -n "$STREAM_COST" ] && [ "$STREAM_COST" -gt 0 ]
curl -fsS "${AUTH[@]}" "$BASE/v1/account" > "$WORK/after"
BEFORE=$(jq -r .balance_micro_usd "$WORK/before")
AFTER=$(jq -r .balance_micro_usd "$WORK/after")
[ "$((BEFORE - AFTER))" = "$((COST + STREAM_COST))" ]
printf 'Live NEAR AI JSON and streaming requests passed; charged %s + %s micro-USD; balance %s.\n' "$COST" "$STREAM_COST" "$AFTER"
jq -r '.choices[0].message.content' "$WORK/answer"
