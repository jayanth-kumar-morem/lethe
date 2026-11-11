// Key material for the LETHE enclave and for buyers. In local mode the enclave seed lives in a
// file outside the repo (LETHE_SECRETS, default ~/.lethe-secrets). In a TEE deployment the seed is
// derived inside the enclave and never leaves it; only the public keys are registered on-chain.
import { createRequire } from "node:module";
const sodium = createRequire(import.meta.url)("libsodium-wrappers"); // CJS build (ESM build is broken in 0.7.x)
import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const SECRETS_DIR = process.env.LETHE_SECRETS || path.join(os.homedir(), ".lethe-secrets");
const HERE = path.dirname(fileURLToPath(import.meta.url));

export async function ready() { await sodium.ready; return sodium; }

function secretFile(name) {
  fs.mkdirSync(SECRETS_DIR, { recursive: true, mode: 0o700 });
  return path.join(SECRETS_DIR, name);
}

function loadOrCreateSeed(name) {
  const f = secretFile(name);
  if (!fs.existsSync(f)) fs.writeFileSync(f, JSON.stringify({ seed: randomBytes(32).toString("hex") }), { mode: 0o600 });
  return Buffer.from(JSON.parse(fs.readFileSync(f, "utf8")).seed, "hex");
}

/** Enclave keys: ed25519 (verdict signing) + X25519 (sealed boxes). */
export async function enclaveKeys() {
  await sodium.ready;
  const seed = process.env.LETHE_ENCLAVE_SEED ? Buffer.from(process.env.LETHE_ENCLAVE_SEED, "hex") : loadOrCreateSeed("enclave.json");
  const sign = sodium.crypto_sign_seed_keypair(seed);
  const box = sodium.crypto_box_seed_keypair(Buffer.from(sodium.crypto_generichash(32, Buffer.concat([Buffer.from("lethe-box"), seed]))));
  return {
    signPk: Buffer.from(sign.publicKey).toString("hex"),
    signSk: sign.privateKey,
    boxPk: Buffer.from(box.publicKey).toString("hex"),
    boxSk: box.privateKey,
    boxPkRaw: box.publicKey,
  };
}

/** A buyer's X25519 delivery keypair, one per buyer account, stored outside the repo. */
export async function buyerBoxKeys(accountId) {
  await sodium.ready;
  const seed = loadOrCreateSeed(`buyer-${accountId}.json`);
  const box = sodium.crypto_box_seed_keypair(seed);
  return { pk: Buffer.from(box.publicKey).toString("hex"), sk: box.privateKey, pkRaw: box.publicKey };
}

export async function seal(plaintext, recipientPkHex) {
  await sodium.ready;
  return Buffer.from(sodium.crypto_box_seal(Buffer.from(plaintext), Buffer.from(recipientPkHex, "hex")));
}

export async function open(ciphertext, pkRaw, sk) {
  await sodium.ready;
  return Buffer.from(sodium.crypto_box_seal_open(Buffer.from(ciphertext), pkRaw, sk));
}

export async function signDetached(message, sk) {
  await sodium.ready;
  return Buffer.from(sodium.crypto_sign_detached(Buffer.from(message), sk)).toString("hex");
}

export async function zeroize(...bufs) {
  await sodium.ready;
  for (const b of bufs) if (b && b.length) sodium.memzero(b instanceof Uint8Array ? b : Buffer.from(b));
}

export const sha256hex = (b) => createHash("sha256").update(b).digest("hex");

/** Measurement of the enclave program: sha256 over its source files + lockfile. In a TDX/Shade
 *  deployment this would be the image digest bound into the attestation quote. */
export function enclaveCodehash() {
  const root = path.join(HERE, "..");
  const files = ["service.js", "lib/keys.js", "lib/novelty.js", "lib/ai.js", "lib/near.js", "package-lock.json"];
  const h = createHash("sha256");
  for (const f of files) { h.update(f + "\0"); h.update(fs.readFileSync(path.join(root, f))); }
  return "sha256:" + h.digest("hex");
}
