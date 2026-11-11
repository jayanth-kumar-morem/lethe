#!/usr/bin/env node
// LETHE demo console: a small local web UI over the same CLI and enclave service.
//   set -a; . ../scripts/.testnet.env; set +a; BUYER_A=... BUYER_B=... node server.js   → http://localhost:8787
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ENCLAVE_DIR = path.join(HERE, "..", "enclave");
const { view } = await import(path.join(ENCLAVE_DIR, "lib/near.js"));
const C = process.env.LETHE_CONTRACT;
const PORT = Number(process.env.PORT || 8787);
const NEED = "Which execution costs hurt cross-chain intent solvers, and where is the edge?";
const PAYOUT = process.env.SELLER_PAYOUT || "solana:USDC:chainsig";
const clients = new Set();
const send = (ch, line) => { const m = `data: ${JSON.stringify({ ch, line })}\n\n`; for (const c of clients) c.write(m); };

function pipe(proc, ch) {
  let buf = "";
  const on = (d) => { buf += d; const parts = buf.split("\n"); buf = parts.pop(); parts.forEach((l) => l.trim() && send(ch, l)); };
  proc.stdout.on("data", on); proc.stderr.on("data", on);
}
const relayerKey = process.env.LETHE_RELAYER_KEY_FILE ? JSON.parse(fs.readFileSync(process.env.LETHE_RELAYER_KEY_FILE, "utf8")).private_key : undefined;
const svc = spawn("node", ["service.js"], { cwd: ENCLAVE_DIR, env: { ...process.env, LETHE_RELAYER_KEY: relayerKey } });
pipe(svc, "enclave");
process.on("exit", () => svc.kill());

const steps = {
  list: () => ["list", "--as", process.env.SELLER_ID, "--file", "../demo/memo.md", "--title", "Where cross-chain intent solvers lose money",
    "--teaser", "8 findings on solver inventory, overnight spreads and refunds", "--ask", "1", "--payout", PAYOUT],
  buyerA: (lid) => ["inspect", "--as", process.env.BUYER_A, "--listing", lid, "--budget", "2", "--need", NEED, "--notes", "../demo/notes-buyer-a.md"],
  buyerB: (lid) => ["inspect", "--as", process.env.BUYER_B, "--listing", lid, "--budget", "2", "--need", NEED, "--notes", "../demo/notes-buyer-b.md"],
};
function run(step, lid) {
  return new Promise((resolve) => {
    const ch = step === "list" ? "seller" : step;
    const p = spawn("node", ["cli.js", ...steps[step](lid)], { cwd: ENCLAVE_DIR, env: process.env });
    pipe(p, ch);
    p.on("close", (code) => { send(ch, code === 0 ? "✓ done" : `✗ exit ${code}`); resolve(code); });
  });
}

async function state() {
  const cfg = await view(C, "get_config");
  const [nl, nr] = await view(C, "next_ids");
  const listings = await view(C, "get_listings", { from: Math.max(0, nl - 5), limit: 5 });
  const requests = [];
  for (let i = Math.max(0, nr - 6); i < nr; i++) requests.push(await view(C, "get_request", { id: i }));
  const strip = (o) => JSON.parse(JSON.stringify(o, (k, v) => (["sealed_content", "sealed_corpus", "delivery", "yield_id"].includes(k) && v ? `${String(v).slice(0, 20)}… (${String(v).length} b64 chars)` : v)));
  return { contract: C, cfg, listings: strip(listings), requests: strip(requests) };
}

http.createServer(async (req, res) => {
  const u = new URL(req.url, "http://x");
  if (u.pathname === "/") { res.writeHead(200, { "content-type": "text/html; charset=utf-8" }); return res.end(fs.readFileSync(path.join(HERE, "index.html"))); }
  if (u.pathname === "/api/events") {
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive" });
    clients.add(res); req.on("close", () => clients.delete(res)); return;
  }
  if (u.pathname === "/api/state") {
    try { const s = await state(); res.writeHead(200, { "content-type": "application/json" }); return res.end(JSON.stringify(s)); }
    catch (e) { res.writeHead(500); return res.end(e.message); }
  }
  if (u.pathname.startsWith("/api/run/") && req.method === "POST") {
    const step = u.pathname.split("/").pop();
    if (!steps[step]) { res.writeHead(404); return res.end(); }
    const [nl] = await view(C, "next_ids");
    const code = await run(step, String(u.searchParams.get("listing") ?? Math.max(0, nl - 1)));
    res.writeHead(200, { "content-type": "application/json" }); return res.end(JSON.stringify({ code }));
  }
  res.writeHead(404); res.end();
}).listen(PORT, () => console.log(`LETHE console on http://localhost:${PORT} (contract ${C})`));
