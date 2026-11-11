// Deterministic, buyer-relative novelty. Runs inside the enclave on plaintext that never leaves it.
// The seller's memo is split into claims; each claim is embedded (all-MiniLM-L6-v2, CPU, local)
// and compared with every sentence of the buyer's own private notes. A claim the buyer already
// "knows" (high cosine similarity) contributes little; an unseen claim contributes fully.
// No LLM is involved here, so text inside the memo cannot talk this score up.
import { pipeline, env as hf } from "@huggingface/transformers";

hf.allowRemoteModels = process.env.LETHE_OFFLINE ? false : true;
export const EMBED_MODEL = "Xenova/all-MiniLM-L6-v2";
let embedder;
async function embed(texts) {
  embedder ||= await pipeline("feature-extraction", EMBED_MODEL, { dtype: "fp32" });
  const out = await embedder(texts, { pooling: "mean", normalize: true });
  const [n, d] = out.dims;
  return Array.from({ length: n }, (_, i) => out.data.slice(i * d, (i + 1) * d));
}
const dot = (a, b) => { let s = 0; for (let i = 0; i < a.length; i++) s += a[i] * b[i]; return s; };

export function claims(text) {
  return text
    .replace(/^#.*$/gm, "")
    .split(/(?<=[.!?])\s+|\n+/)
    .map((s) => s.replace(/^[-*\d.)\s]+/, "").trim())
    .filter((s) => s.split(/\s+/).length >= 5);
}

// A claim with similarity >= KNOWN to some buyer sentence counts as already known (0 novelty);
// <= UNSEEN counts as fully new; linear in between.
export const KNOWN = 0.78, UNSEEN = 0.45;

// Claims are weighted by how close they are to the buyer's need, so a seller cannot inflate
// novelty by padding the memo with fresh but irrelevant sentences.
export async function novelty(memo, buyerNotes, need) {
  const c = claims(memo);
  const b = claims(buyerNotes || "");
  if (!c.length) return { bps: 0, perClaim: [] };
  const ce = await embed(c);
  const ne = need ? (await embed([need]))[0] : null;
  const be = b.length ? await embed(b) : [];
  const perClaim = c.map((claim, i) => {
    let best = 0, bestJ = -1;
    be.forEach((v, j) => { const s = dot(ce[i], v); if (s > best) { best = s; bestJ = j; } });
    const n = Math.max(0, Math.min(1, (KNOWN - best) / (KNOWN - UNSEEN)));
    const weight = ne ? Math.max(0.05, dot(ce[i], ne)) : 1;
    return { claim, maxSim: +best.toFixed(3), novelty: +n.toFixed(3), weight: +weight.toFixed(3), closest: bestJ >= 0 ? b[bestJ] : null };
  });
  const wsum = perClaim.reduce((s, x) => s + x.weight, 0);
  const mean = perClaim.reduce((s, x) => s + x.novelty * x.weight, 0) / wsum;
  return { bps: Math.round(mean * 10000), perClaim };
}

/** Cosine similarity of the buyer's public need with the memo (used by the simulated scorer). */
export async function needSimilarity(need, memo) {
  const [a, ...rest] = await embed([need, ...claims(memo)]);
  const sims = rest.map((v) => dot(a, v)).sort((x, y) => y - x);
  const top = sims.slice(0, 3);
  return top.reduce((s, x) => s + x, 0) / Math.max(1, top.length);
}
