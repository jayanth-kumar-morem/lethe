#!/usr/bin/env node
// LETHE enclave service (LOCAL MODE).
// Watches the contract for pending inspections, opens the sealed memo and the buyer's sealed
// notes, scores novelty (deterministic embeddings) and relevance (TEE model with signed receipt,
// or the labelled local stand-in), then either seals the memo to the buyer (deal) or zeroizes
// everything and emits a forget certificate (no deal). The verdict is ed25519-signed and handed
// to the contract via `respond`, which resumes the buyer's yielded transaction.
//
// Honesty note: in local mode this process runs on an ordinary machine, so its attestation is
// SIMULATED. The contract only trusts the key the owner registered for codehash `enclaveCodehash()`.
import { enclaveKeys, open, seal, signDetached, zeroize, sha256hex, enclaveCodehash } from "./lib/keys.js";
import { novelty } from "./lib/novelty.js";
import { relevance, isReal } from "./lib/ai.js";
import { account, view, call, near } from "./lib/near.js";

const CONTRACT = process.env.LETHE_CONTRACT;
const RELAYER = process.env.LETHE_RELAYER || CONTRACT;
if (!CONTRACT) { console.error("set LETHE_CONTRACT"); process.exit(1); }
const once = process.argv.includes("--once");
const log = (...a) => console.log(new Date().toISOString().slice(11, 19), "[enclave]", ...a);

const keys = await enclaveKeys();
const codehash = enclaveCodehash();
const cfg = await view(CONTRACT, "get_config");
if (cfg.enclave_pk !== keys.signPk) {
  console.error(`enclave key ${keys.signPk} is not the one registered on ${CONTRACT} (${cfg.enclave_pk})`);
  process.exit(1);
}
log(`mode: ${isReal() ? "NEAR AI Cloud (real receipts)" : "SIMULATED AI signer (no NEARAI_API_KEY)"} · attestation: SIMULATED (local mode)`);
log(`codehash ${codehash}${cfg.enclave_codehash === codehash ? " (matches registry)" : " (WARNING: registry has " + cfg.enclave_codehash + ")"}`);
const relayer = account(RELAYER, process.env.LETHE_RELAYER_KEY);
const seen = new Set();
const attempts = new Map();
let scanFrom = Number(process.env.LETHE_SCAN_FROM || 0);

async function handle(req) {
  const listing = await view(CONTRACT, "get_listing", { id: req.listing_id });
  log(`request #${req.id}: ${req.buyer} inspects listing #${listing.id} "${listing.title}" · budget ${near(req.budget)} NEAR · ask ${near(listing.ask)} NEAR`);
  const memoBuf = await open(Buffer.from(listing.sealed_content, "base64"), keys.boxPkRaw, keys.boxSk);
  const notesBuf = req.sealed_corpus ? await open(Buffer.from(req.sealed_corpus, "base64"), keys.boxPkRaw, keys.boxSk) : Buffer.alloc(0);
  const memo = memoBuf.toString("utf8"), notes = notesBuf.toString("utf8");
  if (sha256hex(memoBuf) !== listing.content_hash) throw new Error("sealed content does not match listed content_hash");

  const nov = await novelty(memo, notes, req.need);
  for (const c of nov.perClaim) log(`  claim ${c.novelty >= 0.5 ? "NEW " : "KNOWN"} sim=${c.maxSim.toFixed(2)}  ${c.claim.slice(0, 70)}`);
  log(`  novelty vs buyer's own notes: ${(nov.bps / 100).toFixed(1)}%`);
  const receipt = await relevance({ need: req.need, memo, requestId: req.id });
  const rel = JSON.parse(JSON.parse(receipt.response_body).choices[0].message.content.match(/\{[\s\S]*\}/)[0]).relevance;
  log(`  relevance ${(rel * 100).toFixed(1)}% · receipt signed by ${receipt.signing_address}${receipt.simulated ? " (SIMULATED)" : " (NEAR AI Cloud TEE)"}`);

  // Pre-compute the contract's rule so we know which branch to prepare. The contract recomputes it.
  const relBps = Math.round(Math.max(0, Math.min(1, rel)) * 10000);
  const budget = BigInt(req.budget), ask = BigInt(listing.ask);
  const n = BigInt(nov.bps), r = BigInt(relBps);
  const val = (budget / 10000n) * n / 10000n * r + (budget % 10000n) * n / 10000n * r / 10000n; // same as contract
  const deal = val >= ask;
  let delivery = null, forget_cert = null;
  if (deal) {
    delivery = (await seal(memoBuf, req.buyer_box_pk)).toString("base64");
  } else {
    forget_cert = JSON.stringify({
      type: "lethe/forget-certificate/v1",
      contract: CONTRACT, request_id: req.id, listing_id: listing.id, buyer: req.buyer,
      content_hash: listing.content_hash, corpus_commit: sha256hex(Buffer.from(req.sealed_corpus || "", "base64")),
      enclave_codehash: codehash,
      zeroized: ["seller_plaintext", "buyer_notes_plaintext", "claim_embeddings", "model_context"],
      retained: ["scores", "receipt_hashes"],
      attestation: "simulated-local-mode",
    });
  }
  await zeroize(memoBuf, notesBuf);
  const verdict = JSON.stringify({
    request_id: req.id, listing_id: listing.id, buyer: req.buyer, novelty_bps: nov.bps,
    ai_receipt: { text: receipt.text, signature: receipt.signature, response_body: receipt.response_body },
    delivery, forget_cert,
  });
  const signature = await signDetached(verdict, keys.signSk);
  await call(relayer, CONTRACT, "respond", { verdict_json: verdict, signature }, { gasTera: 150n });
  log(`  verdict signed and submitted: ${deal ? "DEAL → memo sealed to buyer" : "NO DEAL → plaintext zeroized, forget certificate " + sha256hex(Buffer.from(forget_cert)).slice(0, 16) + "…"}`);
}

async function tick() {
  const pending = await view(CONTRACT, "get_pending", { from: scanFrom, limit: 50 });
  for (const r of pending) {
    if (seen.has(r.id)) continue;
    seen.add(r.id);
    try { await handle(r); } catch (e) {
      const n = (attempts.get(r.id) || 0) + 1; attempts.set(r.id, n);
      log(`request #${r.id} failed (attempt ${n}): ${e.message}`);
      if (n < 3) seen.delete(r.id); // after 3 failures stay silent: the yield times out and the buyer is refunded
    }
  }
  const [, nextReq] = await view(CONTRACT, "next_ids");
  if (!pending.length) scanFrom = Math.max(scanFrom, nextReq - 1, 0);
}

log(`watching ${CONTRACT} as relayer ${RELAYER}`);
do {
  try { await tick(); } catch (e) { log("tick error:", e.message); }
  if (!once) await new Promise((r) => setTimeout(r, 1500));
} while (!once);
