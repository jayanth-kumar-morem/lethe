export OPENSSL_LIB_DIR ?= /usr/lib/x86_64-linux-gnu
export OPENSSL_INCLUDE_DIR ?= /usr/include
# The live testnet deployment used in the demo video (read-only checks need no keys).
LETHE_CONTRACT ?= lethe-380029.testnet

.PHONY: build test test-contract test-sandbox test-enclave verify deploy demo console
build:
	cd contract && cargo near build non-reproducible-wasm
	cd enclave && npm ci
test: test-contract test-sandbox test-enclave
test-contract:
	cd contract && cargo test --lib
test-sandbox: build
	cd contract && cargo test --test sandbox -- --nocapture
test-enclave:
	cd enclave && npm ci && npm test
# Re-check every settlement of a live deployment from public RPC data: AI receipt signatures,
# signer registry, price rule, refunds, forget-certificate hashes. No keys needed.
verify:
	cd enclave && npm ci --silent && node verify.js $(LETHE_CONTRACT)
# Your own deployment: OWNER=<faucet-funded>.testnet make deploy
deploy: build
	./scripts/deploy-testnet.sh
# BUYER_A=... BUYER_B=... make demo   (after make deploy)
demo:
	./scripts/demo.sh
console:
	set -a; . scripts/.testnet.env; set +a; node web/server.js
