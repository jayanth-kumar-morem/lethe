// Minimal NEAR helpers (near-api-js v7). Keys come from the legacy keychain written by near-cli-rs
// (~/.near-credentials/<network>/<account>.json) or from an explicit private key.
import { Account, JsonRpcProvider } from "near-api-js";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const NETWORK = process.env.NEAR_NETWORK || "testnet";
export const RPC = process.env.NEAR_RPC || (NETWORK === "mainnet" ? "https://rpc.mainnet.near.org" : "https://rpc.testnet.fastnear.com");
export const provider = new JsonRpcProvider({ url: RPC });
export const TGAS = 10n ** 12n;
export const YOCTO = 10n ** 24n;

export function keyFor(accountId) {
  const f = path.join(os.homedir(), ".near-credentials", NETWORK, `${accountId}.json`);
  if (!fs.existsSync(f)) throw new Error(`no key for ${accountId} in ${f}`);
  return JSON.parse(fs.readFileSync(f, "utf8")).private_key;
}

export function account(accountId, privateKey) {
  return new Account(accountId, provider, privateKey || keyFor(accountId));
}

export const view = (contractId, method, args = {}) => provider.callFunction({ contractId, method, args });

export function call(acct, contractId, methodName, args, { gasTera = 100n, deposit = 0n, waitUntil } = {}) {
  return acct.callFunction({ contractId, methodName, args, gas: gasTera * TGAS, deposit, ...(waitUntil ? { waitUntil } : {}) });
}

export function near(yocto) {
  const v = BigInt(yocto);
  const whole = v / YOCTO, frac = (v % YOCTO).toString().padStart(24, "0").slice(0, 4);
  return `${whole}.${frac}`;
}

export function toYocto(nearStr) {
  const [w, f = ""] = String(nearStr).split(".");
  return BigInt(w) * YOCTO + BigInt((f + "0".repeat(24)).slice(0, 24));
}
