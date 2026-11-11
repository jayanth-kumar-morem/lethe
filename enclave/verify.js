#!/usr/bin/env node
// Read-only audit of a LETHE deployment. Needs no keys: for every settled request it re-checks,
// off-chain, what the contract checked on-chain.
//   node verify.js <contract-id>
import { verifyMessage } from "ethers";
import { view } from "./lib/near.js";
import { sha256hex } from "./lib/keys.js";

const id = process.argv[2] || process.env.LETHE_CONTRACT;
if (!id) { console.error("usage: node verify.js <contract-id>"); process.exit(1); }
const cfg = await view(id, "get_config");
const signers = new Map(cfg.ai_signers.map((s) => [s.address, s]));
const [, nReq] = await view(id, "next_ids");
let bad = 0;
console.log(`LETHE audit of ${id}: enclave ${cfg.enclave_pk.slice(0, 16)}… codehash ${cfg.enclave_codehash}`);
for (let i = 0; i < nReq; i++) {
  const r = await view(id, "get_request", { id: i });
  if (!r || !r.outcome) { console.log(`#${i} ${r ? r.status : "missing"} (no outcome)`); continue; }
  const o = r.outcome, checks = [];
  const rec = verifyMessage(o.ai_receipt_text, "0x" + o.ai_receipt_sig).toLowerCase();
  checks.push(["AI receipt recovers to recorded signer", rec === o.ai_signer]);
  checks.push(["signer is registered", signers.has(rec)]);
  checks.push(["simulated flag matches registry", signers.get(rec)?.simulated === o.ai_simulated]);
  const budget = BigInt(r.budget), n = BigInt(o.novelty_bps), rl = BigInt(o.relevance_bps);
  const val = (budget / 10000n) * n / 10000n * rl + (budget % 10000n) * n / 10000n * rl / 10000n;
  checks.push(["valuation = budget × novelty × relevance", val.toString() === o.valuation]);
  checks.push(["paid + refunded = budget", (BigInt(o.paid) + BigInt(o.refunded)).toString() === r.budget]);
  if (r.status === "NoDeal") checks.push(["forget certificate hash", sha256hex(Buffer.from(o.forget_cert)) === o.forget_cert_hash]);
  if (r.status === "Deal") checks.push(["sealed delivery present", !!o.delivery]);
  const ok = checks.every(([, v]) => v);
  if (!ok) bad++;
  console.log(`#${i} ${r.status.padEnd(7)} novelty ${(o.novelty_bps / 100).toFixed(1)}% relevance ${(o.relevance_bps / 100).toFixed(1)}% ${o.ai_simulated ? "[simulated AI signer]" : "[NEAR AI Cloud signer]"} ${ok ? "OK" : "FAIL"}`);
  for (const [k, v] of checks) if (!v) console.log(`    ✗ ${k}`);
}
process.exit(bad ? 1 : 0);
