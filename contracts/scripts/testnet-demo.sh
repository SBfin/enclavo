#!/usr/bin/env bash
# Runs one short auction on testnet end to end: publish, two bids, clear, withdraw.
# Needs near-cli-rs with keys for the operator and two bidders in the legacy keychain.
set -euo pipefail

AUCTION=${AUCTION:-enclavo-auction.testnet}
TOKEN=${TOKEN:-enclavo-musdc.testnet}
OPERATOR=${OPERATOR:-enclavo-ops.testnet}
ALICE=${ALICE:-enclavo-alice.testnet}
BOB=${BOB:-enclavo-bob.testnet}
DURATION_MS=${DURATION_MS:-60000}

call() { # signer contract method json [deposit]
  near contract call-function as-transaction "$2" "$3" json-args "$4" prepaid-gas '100 Tgas' \
    attached-deposit "${5:-0 NEAR}" sign-as "$1" network-config testnet sign-with-legacy-keychain send 2>&1 |
    grep -E 'Transaction ID|EVENT_JSON|rror|anic' || true
}
view() { # contract method json
  near contract call-function as-read-only "$1" "$2" json-args "$3" network-config testnet now 2>/dev/null |
    sed '/^$/,$d'
}

for account in "$AUCTION" "$OPERATOR"; do
  call "$OPERATOR" "$TOKEN" mint "{\"account_id\":\"$account\",\"amount\":\"0\"}"
done
for account in "$ALICE" "$BOB"; do
  call "$account" "$TOKEN" mint "{\"account_id\":\"$account\",\"amount\":\"1000000000\"}"
done

id=$(view "$AUCTION" get_config '{}' | sed -n 's/.*"auctions": \([0-9]*\).*/\1/p')
echo "== publish auction $id: 10 IU/day of GLM 5.3 Flash for 7 days, reserve \$0.50, open ${DURATION_MS} ms"
call "$OPERATOR" "$AUCTION" publish_auction "{\"route_id\":\"z-ai/glm-5.3-flash@near-ai\",\"units\":10,\"term_days\":7,\"reserve_price\":\"500000\",\"tokens_per_iu\":1000000,\"prompt_weight_bps\":1600,\"duration_ms\":$DURATION_MS}"

echo "== alice bids 8 IU/day at \$1.00, bob bids 6 IU/day at \$0.80"
call "$ALICE" "$TOKEN" ft_transfer_call "{\"receiver_id\":\"$AUCTION\",\"amount\":\"56000000\",\"msg\":\"{\\\"auction_id\\\":$id,\\\"units\\\":8,\\\"max_price\\\":\\\"1000000\\\"}\"}" '1 yoctoNEAR'
call "$BOB" "$TOKEN" ft_transfer_call "{\"receiver_id\":\"$AUCTION\",\"amount\":\"33600000\",\"msg\":\"{\\\"auction_id\\\":$id,\\\"units\\\":6,\\\"max_price\\\":\\\"800000\\\"}\"}" '1 yoctoNEAR'

echo "== waiting for the close"
sleep $((DURATION_MS / 1000 + 5))
call "$BOB" "$AUCTION" clear_auction "{\"auction_id\":$id}"
view "$AUCTION" get_auction "{\"auction_id\":$id}"

echo "== entitlements and refunds"
view "$AUCTION" entitlements_of "{\"account_id\":\"$BOB\",\"limit\":1}"
for account in "$ALICE" "$BOB" "$OPERATOR"; do
  echo "$account unlocked: $(view "$AUCTION" balance_of "{\"account_id\":\"$account\"}")"
  call "$account" "$AUCTION" withdraw '{}'
done
