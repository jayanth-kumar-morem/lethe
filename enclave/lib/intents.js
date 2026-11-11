// NEAR Intents 1Click: a dry quote (no deposit address is funded, nothing executes) for routing a
// seller's NEAR proceeds to the asset/chain the seller asked for. Mainnet-only API (HACKATHON.md Q-007).
const BASE = process.env.ONECLICK_BASE || "https://1click.chaindefuser.com";
const ASSETS = {
  "solana:USDC": "nep141:sol-5ce3bf3a31af18be40ba30f721101b4341690186.omft.near",
  "near:USDC": "nep141:17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1",
  "ethereum:USDC": "nep141:eth-0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48.omft.near",
};
export async function dryQuote({ amountYocto, chain, asset, recipient, refundTo = process.env.ONECLICK_REFUND_TO || "ac8baad98f54be6967f896dd87a2bd27fbca89f27c3bc1c20481fdb642016a1d" }) {
  const destinationAsset = ASSETS[`${chain}:${asset}`];
  if (!destinationAsset) return { ok: false, error: `no route configured for ${asset} on ${chain}` };
  const body = {
    dry: true, swapType: "EXACT_INPUT", slippageTolerance: 100,
    originAsset: "nep141:wrap.near", depositType: "ORIGIN_CHAIN", destinationAsset,
    amount: String(amountYocto), refundTo, refundType: "ORIGIN_CHAIN",
    recipient, recipientType: "DESTINATION_CHAIN",
    deadline: new Date(Date.now() + 30 * 60 * 1000).toISOString(),
  };
  const headers = { "Content-Type": "application/json" };
  if (process.env.ONECLICK_JWT) headers.Authorization = `Bearer ${process.env.ONECLICK_JWT}`;
  try {
    const r = await fetch(`${BASE}/v0/quote`, { method: "POST", headers, body: JSON.stringify(body) });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) return { ok: false, status: r.status, error: j.message || "quote failed", request: body };
    const q = j.quote || {};
    return { ok: true, amountOutFormatted: q.amountOutFormatted, amountOutUsd: q.amountOutUsd, timeEstimate: q.timeEstimate, raw: j, request: body };
  } catch (e) {
    return { ok: false, error: e.message, request: body };
  }
}
