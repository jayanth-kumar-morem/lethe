#!/usr/bin/env bash
# Deploy LETHE to NEAR testnet and wire the demo world.
# Needs: near-cli-rs, node, a built wasm (cd contract && cargo near build non-reproducible-wasm),
# and a faucet-funded testnet account whose key is in ~/.near-credentials (legacy keychain):
#   near account create-account sponsor-by-faucet-service $OWNER autogenerate-new-keypair save-to-legacy-keychain network-config testnet create
# The contract is deployed to $OWNER itself; seller and enclave relayer are sub-accounts.
set -euo pipefail
: "${OWNER:?set OWNER (e.g. lethe-123456.testnet)}"
NET=testnet
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$HERE/.."
WASM="$ROOT/contract/target/near/lethe.wasm"
SELLER="seller.$OWNER"; ENCLAVE="enclave.$OWNER"
cd "$ROOT/enclave"

sub() { near account create-account fund-myself "$1" "$2" autogenerate-new-keypair save-to-legacy-keychain sign-as "$OWNER" network-config $NET sign-with-legacy-keychain send; }
call() { near contract call-function as-transaction "$OWNER" "$1" json-args "$2" prepaid-gas '100.0 Tgas' attached-deposit '0 NEAR' sign-as "$OWNER" network-config $NET sign-with-legacy-keychain send; }

read -r SIGN_PK BOX_PK CODEHASH SIM_ADDR < <(node -e '
import("./lib/keys.js").then(async k => { const e = await k.enclaveKeys(); const a = await import("./lib/ai.js");
console.log(e.signPk, e.boxPk, k.enclaveCodehash(), a.simSigner().address.toLowerCase()); })')

near account view-account-summary "$SELLER" network-config $NET now >/dev/null 2>&1 || sub "$SELLER" '1 NEAR'
near account view-account-summary "$ENCLAVE" network-config $NET now >/dev/null 2>&1 || sub "$ENCLAVE" '1 NEAR'

near contract deploy "$OWNER" use-file "$WASM" with-init-call new \
  json-args "{\"owner\":\"$OWNER\",\"enclave_pk\":\"$SIGN_PK\",\"enclave_box_pk\":\"$BOX_PK\",\"enclave_codehash\":\"$CODEHASH\"}" \
  prepaid-gas '100.0 Tgas' attached-deposit '0 NEAR' network-config $NET sign-with-legacy-keychain send

# Local stand-in AI signer: always registered as simulated=true, visible in every settlement.
call add_ai_signer "{\"address\":\"$SIM_ADDR\",\"label\":\"local stand-in (no NEAR AI key)\",\"simulated\":true}"

# Real NEAR AI Cloud signers: taken from the PUBLIC attestation report of the model, after the
# TDX quote + NVIDIA GPU attestation were verified off-chain (scripts/verify-nearai-attestation.sh).
MODEL="${LETHE_MODEL:-Qwen/Qwen3.8-27B}"
for ADDR in $(curl -s "https://cloud-api.near.ai/v1/attestation/report?model=$MODEL&signing_algo=ecdsa" | node -e '
let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const j=JSON.parse(s);console.log((j.model_attestations||[]).map(a=>a.signing_address.toLowerCase()).join(" "))})'); do
  call add_ai_signer "{\"address\":\"$ADDR\",\"label\":\"NEAR AI Cloud $MODEL (TEE, attestation verified off-chain)\",\"simulated\":false}"
done

# The enclave relayer gets a function-call key that can ONLY call `respond` on LETHE.
SECRETS="${LETHE_SECRETS:-$HOME/.lethe-secrets}"; mkdir -p "$SECRETS"; chmod 700 "$SECRETS"
KEYFILE="$SECRETS/$ENCLAVE.fc-key.json"
if [ ! -f "$KEYFILE" ]; then
  node -e 'import("near-api-js").then(({KeyPair})=>{const k=KeyPair.fromRandom("ed25519");console.log(JSON.stringify({public_key:k.getPublicKey().toString(),private_key:k.toString()}))})' > "$KEYFILE"
  chmod 600 "$KEYFILE"
fi
PUB=$(node -e "console.log(JSON.parse(require('fs').readFileSync('$KEYFILE','utf8')).public_key)")
near account add-key "$ENCLAVE" grant-function-call-access --allowance '0.5 NEAR' \
  --contract-account-id "$OWNER" --function-names 'respond' \
  use-manually-provided-public-key "$PUB" network-config $NET sign-with-legacy-keychain send || true

cat > "$HERE/.testnet.env" <<ENV
LETHE_CONTRACT=$OWNER
LETHE_RELAYER=$ENCLAVE
LETHE_RELAYER_KEY_FILE=$KEYFILE
SELLER_ID=$SELLER
ENV
echo "Deployed LETHE: https://testnet.nearblocks.io/address/$OWNER"
