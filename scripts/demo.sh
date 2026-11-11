#!/usr/bin/env bash
# End-to-end LETHE demo on testnet: seller lists a sealed memo; buyer A (no overlap) inspects and
# buys; buyer B (already knows most of it) inspects, gets no deal, a full refund and a forget
# certificate. Needs scripts/.testnet.env (from deploy-testnet.sh) and BUYER_A / BUYER_B accounts
# with keys in ~/.near-credentials/testnet.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; set -a; . "$HERE/.testnet.env"; set +a
: "${BUYER_A:?set BUYER_A}"; : "${BUYER_B:?set BUYER_B}"
cd "$HERE/../enclave"
export LETHE_RELAYER_KEY=$(node -e "console.log(JSON.parse(require('fs').readFileSync('$LETHE_RELAYER_KEY_FILE','utf8')).private_key)")
node service.js > "${ENCLAVE_LOG:-/tmp/lethe-enclave.log}" 2>&1 & SVC=$!
trap 'kill $SVC 2>/dev/null' EXIT
NEED="Which execution costs hurt cross-chain intent solvers, and where is the edge?"
echo "== 1. Seller lists a sealed memo (wants USDC on Solana)"
LID=$(node cli.js list --as "$SELLER_ID" --file ../demo/memo.md --title "Where cross-chain intent solvers lose money" \
  --teaser "8 findings on solver inventory, overnight spreads and refunds" --ask 1 \
  --payout solana:USDC:chainsig | tee /dev/stderr | sed -n 's/^listed #\([0-9]*\).*/\1/p')
echo; echo "== 2. Buyer A: notes about something else -> novel -> DEAL"
node cli.js inspect --as "$BUYER_A" --listing "$LID" --budget 2 --need "$NEED" --notes ../demo/notes-buyer-a.md
echo; echo "== 3. Buyer B: already runs a solver, notes overlap -> NO DEAL, refund, forget certificate"
node cli.js inspect --as "$BUYER_B" --listing "$LID" --budget 2 --need "$NEED" --notes ../demo/notes-buyer-b.md
echo; echo "== enclave log"; cat "${ENCLAVE_LOG:-/tmp/lethe-enclave.log}"
