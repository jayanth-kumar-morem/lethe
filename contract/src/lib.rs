//! LETHE: an information market where the AI forgets.
//!
//! A seller lists an information good sealed to an enclave key. A buyer escrows a budget and seals
//! its own private notes to the same enclave. `inspect` opens a NEAR yield: the transaction waits
//! while the enclave measures novelty (against the buyer's notes) and relevance (by a TEE-hosted
//! model whose response is ECDSA-signed). The enclave answers through `respond`; this contract
//! checks the enclave's ed25519 signature, re-verifies the AI receipt with `ecrecover`, recomputes
//! the price itself, and then either pays the seller and releases the sealed delivery, or refunds
//! the buyer and records the enclave's signed forget certificate. If the enclave never answers,
//! the yield times out and the buyer is refunded.
//!
//! Money rules live here, not in any model: the model can only lower a price below the buyer's
//! budget, never raise it.
use near_sdk::json_types::{Base64VecU8, U128};
use near_sdk::store::{IterableMap, LookupMap, LookupSet};
use near_sdk::{
    env, near, require, AccountId, CryptoHash, Gas, GasWeight, NearToken, PanicOnDefault, Promise,
    PromiseError,
};

pub const MAX_BLOB: usize = 12_000;
const CALLBACK_GAS: Gas = Gas::from_tgas(40);
const BPS: u128 = 10_000;

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Payout {
    /// e.g. "near", "solana"
    pub chain: String,
    /// e.g. "NEAR", "USDC"
    pub asset: String,
    /// destination address on that chain (shown to the settlement router)
    pub address: String,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Listing {
    pub id: u64,
    pub seller: AccountId,
    pub title: String,
    /// public teaser, written by the seller; never the content
    pub teaser: String,
    pub ask: U128,
    /// sha256(plaintext), hex. Lets a buyer check the delivery is what was inspected.
    pub content_hash: String,
    /// plaintext sealed (X25519 sealed box) to the enclave key, base64
    pub sealed_content: Base64VecU8,
    pub payout: Payout,
    pub sales: u32,
    pub inspections: u32,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, PartialEq, Debug)]
pub enum Status {
    Pending,
    Deal,
    NoDeal,
    TimedOut,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Outcome {
    pub novelty_bps: u32,
    pub relevance_bps: u32,
    /// floor(budget * novelty * relevance): the buyer's valuation under its own policy
    pub valuation: U128,
    /// what moved to the seller (0 on no deal)
    pub paid: U128,
    pub refunded: U128,
    pub ai_signer: String,
    pub ai_model: String,
    /// true when the AI receipt came from the local stand-in signer (no NEAR AI key)
    pub ai_simulated: bool,
    pub ai_receipt_text: String,
    /// 65-byte personal_sign signature over `ai_receipt_text` (hex), for off-chain re-verification
    pub ai_receipt_sig: String,
    /// sealed to the buyer's X25519 key; only present on a deal
    pub delivery: Option<Base64VecU8>,
    /// sha256 of the enclave-signed forget certificate; only present on no deal
    pub forget_cert_hash: Option<String>,
    pub forget_cert: Option<String>,
    pub enclave_codehash: String,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct Request {
    pub id: u64,
    pub listing_id: u64,
    pub buyer: AccountId,
    pub budget: U128,
    /// public one-line statement of what the buyer is looking for
    pub need: String,
    /// buyer's private notes sealed to the enclave key, base64
    pub sealed_corpus: Base64VecU8,
    /// buyer X25519 public key (hex) used for the sealed delivery
    pub buyer_box_pk: String,
    pub yield_id: Base64VecU8,
    pub status: Status,
    pub opened_block: u64,
    pub outcome: Option<Outcome>,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct AiSigner {
    pub label: String,
    pub simulated: bool,
}

/// What the enclave signs (as the exact JSON string passed to `respond`).
#[near(serializers = [json])]
pub struct Verdict {
    pub request_id: u64,
    pub listing_id: u64,
    pub buyer: AccountId,
    pub novelty_bps: u32,
    pub ai_receipt: AiReceipt,
    pub delivery: Option<Base64VecU8>,
    pub forget_cert: Option<String>,
}

#[near(serializers = [json])]
pub struct AiReceipt {
    /// "{model}:{sha256(request)}:{sha256(response)}" as signed by NEAR AI Cloud
    pub text: String,
    /// 65-byte r||s||v hex (Ethereum personal_sign)
    pub signature: String,
    /// the exact response body that was hashed
    pub response_body: String,
}

/// Settlement instruction carried through yield/resume into the callback.
#[near(serializers = [borsh, json])]
pub struct Settlement {
    pub deal: bool,
    pub novelty_bps: u32,
    pub relevance_bps: u32,
    pub valuation: U128,
    pub pay: U128,
    pub ai_signer: String,
    pub ai_model: String,
    pub ai_simulated: bool,
    pub ai_receipt_text: String,
    pub ai_receipt_sig: String,
    pub delivery: Option<Base64VecU8>,
    pub forget_cert: Option<String>,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Lethe {
    owner: AccountId,
    /// ed25519 public key of the enclave's verdict-signing key
    enclave_pk: [u8; 32],
    /// X25519 public key buyers and sellers seal to (hex), published for clients
    enclave_box_pk: String,
    /// code measurement the owner approved for this key (sha256 of the enclave build)
    enclave_codehash: String,
    /// lowercase 0x address -> signer info
    ai_signers: IterableMap<String, AiSigner>,
    allowed_models: IterableMap<String, bool>,
    used_receipts: LookupSet<String>,
    listings: IterableMap<u64, Listing>,
    requests: IterableMap<u64, Request>,
    pending: LookupMap<u64, bool>,
    /// verdicts checked by `respond`, consumed by `on_verdict` (yield payloads are capped at 1 KiB)
    settlements: LookupMap<u64, Settlement>,
    next_listing: u64,
    next_request: u64,
}

fn hex32(s: &str) -> [u8; 32] {
    let v = hex::decode(s.trim_start_matches("0x")).unwrap_or_else(|_| env::panic_str("bad hex"));
    v.try_into().unwrap_or_else(|_| env::panic_str("expected 32 bytes"))
}

/// Recover the Ethereum address that personal_signed `text`.
pub fn recover_eth_address(text: &str, sig_hex: &str) -> Option<String> {
    let sig = hex::decode(sig_hex.trim_start_matches("0x")).ok()?;
    if sig.len() != 65 {
        return None;
    }
    let mut v = sig[64];
    if v >= 27 {
        v -= 27;
    }
    let prefixed = format!("\x19Ethereum Signed Message:\n{}{}", text.len(), text);
    let hash = env::keccak256_array(prefixed.as_bytes());
    let pk = env::ecrecover(&hash, &sig[..64], v, true)?;
    let addr = env::keccak256_array(&pk);
    Some(format!("0x{}", hex::encode(&addr[12..])))
}

/// Pull `relevance` in [0,1] out of an OpenAI-style chat completion body.
pub fn parse_relevance(response_body: &str) -> Option<u32> {
    let v: near_sdk::serde_json::Value = near_sdk::serde_json::from_str(response_body).ok()?;
    let content = v["choices"][0]["message"]["content"].as_str()?;
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    let inner: near_sdk::serde_json::Value =
        near_sdk::serde_json::from_str(&content[start..=end]).ok()?;
    let r = inner["relevance"].as_f64()?;
    if !r.is_finite() {
        return None;
    }
    let r = r.clamp(0.0, 1.0);
    Some((r * BPS as f64).round() as u32)
}

pub fn valuation(budget: u128, novelty_bps: u32, relevance_bps: u32) -> u128 {
    // budget * n * r / 1e8 without overflow for any realistic budget
    budget / BPS * novelty_bps as u128 / BPS * relevance_bps as u128
        + (budget % BPS) * novelty_bps as u128 / BPS * relevance_bps as u128 / BPS
}

#[near]
impl Lethe {
    #[init]
    pub fn new(
        owner: AccountId,
        enclave_pk: String,
        enclave_box_pk: String,
        enclave_codehash: String,
    ) -> Self {
        Self {
            owner,
            enclave_pk: hex32(&enclave_pk),
            enclave_box_pk,
            enclave_codehash,
            ai_signers: IterableMap::new(b"s"),
            allowed_models: IterableMap::new(b"m"),
            used_receipts: LookupSet::new(b"u"),
            listings: IterableMap::new(b"l"),
            requests: IterableMap::new(b"r"),
            pending: LookupMap::new(b"p"),
            settlements: LookupMap::new(b"v"),
            next_listing: 0,
            next_request: 0,
        }
    }

    fn only_owner(&self) {
        require!(env::predecessor_account_id() == self.owner, "owner only");
    }

    // ---------- owner: trust registry ----------

    /// Register the enclave key after checking its attestation off-chain. Rotation = call again.
    pub fn set_enclave(&mut self, enclave_pk: String, enclave_box_pk: String, enclave_codehash: String) {
        self.only_owner();
        self.enclave_pk = hex32(&enclave_pk);
        self.enclave_box_pk = enclave_box_pk;
        self.enclave_codehash = enclave_codehash;
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"enclave_set\",\"codehash\":\"{}\"}}", self.enclave_codehash));
    }

    /// Register an AI response signer (e.g. the NEAR AI Cloud signing address taken from its
    /// public attestation report). `simulated` marks a local stand-in signer.
    pub fn add_ai_signer(&mut self, address: String, label: String, simulated: bool) {
        self.only_owner();
        self.ai_signers.insert(address.to_lowercase(), AiSigner { label, simulated });
    }

    pub fn remove_ai_signer(&mut self, address: String) {
        self.only_owner();
        self.ai_signers.remove(&address.to_lowercase());
    }

    pub fn allow_model(&mut self, model: String) {
        self.only_owner();
        self.allowed_models.insert(model, true);
    }

    // ---------- seller ----------

    pub fn list(
        &mut self,
        title: String,
        teaser: String,
        ask: U128,
        content_hash: String,
        sealed_content: Base64VecU8,
        payout: Payout,
    ) -> u64 {
        require!(sealed_content.0.len() <= MAX_BLOB, "sealed content too large");
        require!(title.len() <= 140 && teaser.len() <= 600, "title/teaser too long");
        require!(ask.0 > 0, "ask must be > 0");
        hex32(&content_hash);
        let id = self.next_listing;
        self.next_listing += 1;
        self.listings.insert(
            id,
            Listing {
                id,
                seller: env::predecessor_account_id(),
                title,
                teaser,
                ask,
                content_hash,
                sealed_content,
                payout,
                sales: 0,
                inspections: 0,
            },
        );
        env::log_str(&format!("EVENT_JSON:{{\"event\":\"listed\",\"listing_id\":{id}}}"));
        id
    }

    // ---------- buyer ----------

    /// Escrow `budget` (attached deposit) and ask the enclave to inspect listing `listing_id`
    /// against the buyer's sealed notes. The call yields; its result is the settlement.
    #[payable]
    pub fn inspect(
        &mut self,
        listing_id: u64,
        need: String,
        sealed_corpus: Base64VecU8,
        buyer_box_pk: String,
    ) {
        let budget = env::attached_deposit().as_yoctonear();
        let listing = self.listings.get(&listing_id).cloned().unwrap_or_else(|| env::panic_str("no such listing"));
        require!(budget >= listing.ask.0, "budget below ask: no deal is possible");
        require!(sealed_corpus.0.len() <= MAX_BLOB, "sealed corpus too large");
        require!(need.len() <= 400, "need too long");
        hex32(&buyer_box_pk);
        let id = self.next_request;
        self.next_request += 1;
        let args = near_sdk::serde_json::json!({ "request_id": id }).to_string();
        let (idx, yield_id) =
            env::promise_yield_create_id("on_verdict", args.as_bytes(), CALLBACK_GAS, GasWeight(0));
        let yid: CryptoHash = yield_id_bytes(&yield_id);
        self.requests.insert(
            id,
            Request {
                id,
                listing_id,
                buyer: env::predecessor_account_id(),
                budget: U128(budget),
                need,
                sealed_corpus,
                buyer_box_pk,
                yield_id: Base64VecU8(yid.to_vec()),
                status: Status::Pending,
                opened_block: env::block_height(),
                outcome: None,
            },
        );
        self.pending.insert(id, true);
        if let Some(l) = self.listings.get_mut(&listing_id) {
            l.inspections += 1;
        }
        env::log_str(&format!(
            "EVENT_JSON:{{\"event\":\"inspect_opened\",\"request_id\":{id},\"listing_id\":{listing_id}}}"
        ));
        // The buyer's transaction resolves to whatever `on_verdict` returns.
        env::promise_return(idx);
    }

    // ---------- enclave ----------

    /// Called by the enclave (any relayer can carry it: authority is the ed25519 signature).
    pub fn respond(&mut self, verdict_json: String, signature: String) -> bool {
        let sig: [u8; 64] = hex::decode(&signature)
            .ok()
            .and_then(|v| v.try_into().ok())
            .unwrap_or_else(|| env::panic_str("bad enclave signature encoding"));
        require!(
            env::ed25519_verify(&sig, verdict_json.as_bytes(), &self.enclave_pk),
            "enclave signature invalid"
        );
        let v: Verdict = near_sdk::serde_json::from_str(&verdict_json)
            .unwrap_or_else(|_| env::panic_str("bad verdict json"));
        let req = self.requests.get(&v.request_id).cloned().unwrap_or_else(|| env::panic_str("no such request"));
        require!(req.status == Status::Pending, "request not pending");
        require!(req.listing_id == v.listing_id && req.buyer == v.buyer, "verdict does not match request");
        require!(v.novelty_bps as u128 <= BPS, "novelty out of range");
        let listing = self.listings.get(&req.listing_id).cloned().unwrap();

        // --- AI receipt: verify on-chain ---
        let r = &v.ai_receipt;
        let parts: Vec<&str> = r.text.split(':').collect();
        require!(parts.len() == 3 || parts.len() == 2, "bad receipt text");
        let (model, resp_hash) = if parts.len() == 3 { (parts[0].to_string(), parts[2]) } else { (String::new(), parts[1]) };
        // optional allowlist: enforced only once the owner has added at least one model
        if !model.is_empty() && !self.allowed_models.is_empty() {
            require!(self.allowed_models.contains_key(&model), "model not allowed");
        }
        let body_hash = hex::encode(env::sha256_array(r.response_body.as_bytes()));
        require!(body_hash == resp_hash, "response body does not match signed hash");
        require!(!self.used_receipts.contains(&r.text), "receipt already used");
        let signer = recover_eth_address(&r.text, &r.signature).unwrap_or_else(|| env::panic_str("ecrecover failed"));
        let info = self.ai_signers.get(&signer).cloned().unwrap_or_else(|| env::panic_str("AI signer not registered"));
        let relevance_bps = parse_relevance(&r.response_body).unwrap_or_else(|| env::panic_str("no relevance in signed response"));
        self.used_receipts.insert(r.text.clone());

        // --- price rule (the contract's, not the model's) ---
        let budget = req.budget.0;
        let val = valuation(budget, v.novelty_bps, relevance_bps);
        let ask = listing.ask.0;
        let deal = val >= ask;
        // split the surplus between seller and buyer (symmetric Nash bargaining over [ask, valuation])
        let pay = if deal { ask + (val - ask) / 2 } else { 0 };
        if deal {
            require!(v.delivery.as_ref().map(|d| !d.0.is_empty()).unwrap_or(false), "deal needs a sealed delivery");
        } else {
            require!(v.forget_cert.is_some(), "no deal needs a forget certificate");
        }
        let s = Settlement {
            deal,
            novelty_bps: v.novelty_bps,
            relevance_bps,
            valuation: U128(val),
            pay: U128(pay),
            ai_signer: signer,
            ai_model: model,
            ai_simulated: info.simulated,
            ai_receipt_text: r.text.clone(),
            ai_receipt_sig: r.signature.clone(),
            delivery: if deal { v.delivery } else { None },
            forget_cert: if deal { None } else { v.forget_cert },
        };
        require!(!self.settlements.contains_key(&v.request_id), "already answered");
        self.settlements.insert(v.request_id, s);
        let yid: CryptoHash = req.yield_id.0.clone().try_into().unwrap();
        // the resume payload is just a signal; the checked settlement is read from state
        let ok = env::promise_yield_resume(&yid, b"true");
        require!(ok, "yield not found (timed out?)");
        ok
    }

    #[private]
    pub fn on_verdict(
        &mut self,
        request_id: u64,
        #[callback_result] signal: Result<bool, PromiseError>,
    ) -> near_sdk::serde_json::Value {
        let stored = self.settlements.remove(&request_id);
        let settlement = match (signal, stored) {
            (Ok(true), Some(s)) => Ok(s),
            _ => Err(()),
        };
        let mut req = self.requests.get(&request_id).cloned().unwrap_or_else(|| env::panic_str("no such request"));
        self.pending.remove(&request_id);
        let budget = req.budget.0;
        match settlement {
            Err(_) => {
                req.status = Status::TimedOut;
                Promise::new(req.buyer.clone()).transfer(NearToken::from_yoctonear(budget)).detach();
                self.requests.insert(request_id, req);
                env::log_str(&format!("EVENT_JSON:{{\"event\":\"timed_out\",\"request_id\":{request_id}}}"));
                near_sdk::serde_json::json!({"request_id": request_id, "status": "TimedOut", "refunded": budget.to_string()})
            }
            Ok(s) => {
                let listing = self.listings.get(&req.listing_id).cloned().unwrap();
                let pay = s.pay.0.min(budget);
                let refund = budget - pay;
                if s.deal {
                    Promise::new(listing.seller.clone()).transfer(NearToken::from_yoctonear(pay)).detach();
                    if let Some(l) = self.listings.get_mut(&req.listing_id) {
                        l.sales += 1;
                    }
                }
                if refund > 0 {
                    Promise::new(req.buyer.clone()).transfer(NearToken::from_yoctonear(refund)).detach();
                }
                let forget_cert_hash = s.forget_cert.as_ref().map(|c| hex::encode(env::sha256_array(c.as_bytes())));
                req.status = if s.deal { Status::Deal } else { Status::NoDeal };
                req.outcome = Some(Outcome {
                    novelty_bps: s.novelty_bps,
                    relevance_bps: s.relevance_bps,
                    valuation: s.valuation,
                    paid: U128(if s.deal { pay } else { 0 }),
                    refunded: U128(refund),
                    ai_signer: s.ai_signer.clone(),
                    ai_model: s.ai_model.clone(),
                    ai_simulated: s.ai_simulated,
                    ai_receipt_text: s.ai_receipt_text.clone(),
                    ai_receipt_sig: s.ai_receipt_sig.clone(),
                    delivery: s.delivery.clone(),
                    forget_cert_hash: forget_cert_hash.clone(),
                    forget_cert: s.forget_cert.clone(),
                    enclave_codehash: self.enclave_codehash.clone(),
                });
                let status = req.status.clone();
                self.requests.insert(request_id, req);
                env::log_str(&format!(
                    "EVENT_JSON:{{\"event\":\"settled\",\"request_id\":{request_id},\"deal\":{},\"paid\":\"{}\",\"refunded\":\"{}\"}}",
                    s.deal, if s.deal { pay } else { 0 }, refund
                ));
                near_sdk::serde_json::json!({
                    "request_id": request_id, "status": status,
                    "novelty_bps": s.novelty_bps, "relevance_bps": s.relevance_bps,
                    "valuation": s.valuation, "paid": (if s.deal { pay } else { 0 }).to_string(),
                    "refunded": refund.to_string(), "ai_simulated": s.ai_simulated,
                    "forget_cert_hash": forget_cert_hash,
                })
            }
        }
    }

    // ---------- views ----------

    pub fn get_config(&self) -> near_sdk::serde_json::Value {
        near_sdk::serde_json::json!({
            "owner": self.owner,
            "enclave_pk": hex::encode(self.enclave_pk),
            "enclave_box_pk": self.enclave_box_pk,
            "enclave_codehash": self.enclave_codehash,
            "ai_signers": self.ai_signers.iter().map(|(k, v)| near_sdk::serde_json::json!({"address": k, "label": v.label, "simulated": v.simulated})).collect::<Vec<_>>(),
            "allowed_models": self.allowed_models.keys().collect::<Vec<_>>(),
        })
    }

    pub fn get_listing(&self, id: u64) -> Option<Listing> {
        self.listings.get(&id).cloned()
    }

    pub fn get_listings(&self, from: u64, limit: u64) -> Vec<Listing> {
        (from..self.next_listing.min(from + limit)).filter_map(|i| self.listings.get(&i).cloned()).collect()
    }

    pub fn get_request(&self, id: u64) -> Option<Request> {
        self.requests.get(&id).cloned()
    }

    /// Requests the enclave still has to answer (scan window keeps this bounded).
    pub fn get_pending(&self, from: u64, limit: u64) -> Vec<Request> {
        (from..self.next_request.min(from + limit))
            .filter(|i| self.pending.contains_key(i))
            .filter_map(|i| self.requests.get(&i).cloned())
            .collect()
    }

    pub fn next_ids(&self) -> (u64, u64) {
        (self.next_listing, self.next_request)
    }
}

fn yield_id_bytes(y: &near_sdk::YieldId) -> CryptoHash {
    let v = near_sdk::serde_json::to_value(y).unwrap();
    let s = v.as_str().unwrap_or_else(|| env::panic_str("yield id"));
    let b = near_sdk::base64::Engine::decode(&near_sdk::base64::engine::general_purpose::STANDARD, s)
        .unwrap_or_else(|_| env::panic_str("yield id b64"));
    b.try_into().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valuation_math() {
        let one = 10u128.pow(24);
        assert_eq!(valuation(10 * one, 10_000, 10_000), 10 * one);
        assert_eq!(valuation(10 * one, 5_000, 5_000), 25 * one / 10);
        assert_eq!(valuation(10 * one, 0, 10_000), 0);
    }

    #[test]
    fn relevance_parsing() {
        let body = r#"{"id":"x","choices":[{"message":{"role":"assistant","content":"```json\n{\"relevance\": 0.82}\n```"}}]}"#;
        assert_eq!(parse_relevance(body), Some(8200));
        let body = r#"{"choices":[{"message":{"content":"{\"relevance\": 7}"}}]}"#;
        assert_eq!(parse_relevance(body), Some(10_000));
        assert_eq!(parse_relevance(r#"{"choices":[]}"#), None);
    }
}
