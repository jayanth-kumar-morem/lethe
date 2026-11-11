#!/usr/bin/env node
// LETHE command line for sellers and buyers (testnet).
//   node cli.js config
//   node cli.js list    --as SELLER --file memo.md --title "..." --teaser "..." --ask 1 --payout solana:USDC:<addr>
//   node cli.js inspect --as BUYER --listing 0 --budget 2 --need "..." --notes notes.md
//   node cli.js show    --request N
//   node cli.js quote   --amount 1.2 --payout solana:USDC:<addr>
import fs from "node:fs";
import { seal, open, buyerBoxKeys, sha256hex } from "./lib/keys.js";
import { account, view, call, near, toYocto } from "./lib/near.js";
import { dryQuote } from "./lib/intents.js";

const CONTRACT = process.env.LETHE_CONTRACT;
const [cmd, ...rest] = process.argv.slice(2);
const opt = {};
for (let i = 0; i < rest.length; i += 2) opt[rest[i].replace(/^--/, "")] = rest[i + 1];
const say = (...a) => console.log(...a);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if (!CONTRACT) { console.error("set LETHE_CONTRACT"); process.exit(1); }

async function quoteFor(amountYocto, payout) {
  const q = await dryQuote({ amountYocto, chain: payout.chain, asset: payout.asset, recipient: payout.address });
  if (q.ok) say(`  NEAR Intents 1Click dry quote: ${near(amountYocto)} NEAR → ≈ ${q.amountOutFormatted} ${payout.asset} on ${payout.chain} (${payout.address.slice(0, 6)}…), ~${q.timeEstimate}s, no funds moved`);
  else say(`  NEAR Intents 1Click dry quote unavailable (${q.status || ""} ${q.error}); seller is paid in NEAR on testnet`);
  return q;
}

if (cmd === "config") {
  say(JSON.stringify(await view(CONTRACT, "get_config"), null, 2));
} else if (cmd === "list") {
  const cfg = await view(CONTRACT, "get_config");
  const plaintext = fs.readFileSync(opt.file);
  let [chain, asset, address] = (opt.payout || "near:NEAR:" + opt.as).split(":");
  if (address === "chainsig" && chain === "solana") {
    // The seller's Solana address is derived from its NEAR account by NEAR Chain Signatures (MPC,
    // EdDSA domain): only seller's NEAR account can ever sign for it.
    const mpc = process.env.MPC_CONTRACT || "v1.signer-prod.testnet";
    const pk = await view(mpc, "derived_public_key", { path: "lethe-payout", predecessor: opt.as, domain_id: 1 });
    address = pk.replace(/^ed25519:/, "");
    say(`  payout address ${address} on Solana, derived from ${opt.as} via Chain Signatures (${mpc})`);
  }
  const sealed = await seal(plaintext, cfg.enclave_box_pk);
  const id = await call(account(opt.as), CONTRACT, "list", {
    title: opt.title, teaser: opt.teaser || "", ask: toYocto(opt.ask).toString(),
    content_hash: sha256hex(plaintext), sealed_content: sealed.toString("base64"),
    payout: { chain, asset, address },
  });
  say(`listed #${id}: "${opt.title}" ask ${opt.ask} NEAR · ${plaintext.length} bytes sealed to enclave ${cfg.enclave_box_pk.slice(0, 12)}… · on-chain: only sha256 ${sha256hex(plaintext).slice(0, 16)}… + ciphertext`);
} else if (cmd === "inspect") {
  const cfg = await view(CONTRACT, "get_config");
  const buyer = account(opt.as);
  const box = await buyerBoxKeys(opt.as);
  const notes = opt.notes ? fs.readFileSync(opt.notes) : Buffer.alloc(0);
  const sealedNotes = notes.length ? await seal(notes, cfg.enclave_box_pk) : Buffer.alloc(0);
  const listing = await view(CONTRACT, "get_listing", { id: Number(opt.listing) });
  const [, before] = await view(CONTRACT, "next_ids");
  say(`${opt.as} inspects #${listing.id} "${listing.title}" (ask ${near(listing.ask)} NEAR) with budget ${opt.budget} NEAR`);
  say(`  own notes: ${notes.length} bytes, sealed to the enclave (nobody else can read them)`);
  // The transaction yields until the enclave answers; we don't block the CLI on RPC timeouts.
  const sent = call(buyer, CONTRACT, "inspect", {
    listing_id: listing.id, need: opt.need, sealed_corpus: sealedNotes.toString("base64"), buyer_box_pk: box.pk,
  }, { deposit: toYocto(opt.budget), gasTera: 100n, waitUntil: "INCLUDED" }).catch((e) => say("  (rpc)", e.message));
  await sent;
  let rid;
  for (let i = 0; i < 30 && rid === undefined; i++) {
    for (let id = before; id < before + 10; id++) {
      const r = await view(CONTRACT, "get_request", { id });
      if (r && r.buyer === opt.as && r.listing_id === listing.id) { rid = id; break; }
    }
    if (rid === undefined) await sleep(1000);
  }
  if (rid === undefined) throw new Error("inspect transaction not found");
  let req = await view(CONTRACT, "get_request", { id: rid });
  say(`  request #${rid} is yielding on-chain since block ${req.opened_block}: the transaction waits for the enclave`);
  for (let i = 0; i < 300 && req.status === "Pending"; i++) { await sleep(1000); req = await view(CONTRACT, "get_request", { id: rid }); }
  if (!req) throw new Error("no settlement seen");
  if (req.status === "TimedOut") { say(`  settled: TimedOut · the enclave did not answer in time · full budget refunded`); process.exit(0); }
  const o = req.outcome || {};
  say(`  settled: ${req.status} · novelty ${(o.novelty_bps / 100).toFixed(1)}% · relevance ${(o.relevance_bps / 100).toFixed(1)}% · valuation ${near(o.valuation || 0)} NEAR`);
  say(`  paid to seller ${near(o.paid || 0)} NEAR · refunded ${near(o.refunded || 0)} NEAR · AI receipt signer ${o.ai_signer}${o.ai_simulated ? " (SIMULATED)" : " (NEAR AI Cloud)"}`);
  if (req.status === "Deal") {
    const memo = await open(Buffer.from(o.delivery, "base64"), box.pkRaw, box.sk);
    const ok = sha256hex(memo) === listing.content_hash;
    say(`  delivery decrypted with ${opt.as}'s key · sha256 matches listing: ${ok}`);
    say("  ---- memo ----\n" + memo.toString("utf8").split("\n").slice(0, 6).map((l) => "  | " + l).join("\n") + "\n  | …");
    await quoteFor(o.paid, listing.payout);
  } else if (req.status === "NoDeal") {
    say(`  forget certificate sha256 ${o.forget_cert_hash}`);
    say(`  ${o.forget_cert}`);
  }
} else if (cmd === "show") {
  say(JSON.stringify(await view(CONTRACT, "get_request", { id: Number(opt.request) }), (k, v) => (k === "sealed_corpus" || k === "delivery") && v ? v.slice(0, 24) + "…" : v, 2));
} else if (cmd === "quote") {
  const [chain, asset, address] = opt.payout.split(":");
  const q = await quoteFor(toYocto(opt.amount), { chain, asset, address });
  if (opt.json) say(JSON.stringify(q, null, 2));
} else {
  say("usage: node cli.js config|list|inspect|show|quote …");
}
