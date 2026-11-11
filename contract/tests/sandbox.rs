//! Sandbox tests (near-workspaces): yield/resume round trip, on-chain AI receipt verification
//! (ecrecover over a personal_sign'd NEAR AI style receipt), deal, no-deal and timeout paths.
//! Run: cargo near build non-reproducible-wasm && cargo test --test sandbox -- --nocapture
use ed25519_dalek::{Signer, SigningKey};
use k256::ecdsa::SigningKey as EcKey;
use near_workspaces::types::NearToken;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sha3::Keccak256;

const ONE_NEAR: u128 = 1_000_000_000_000_000_000_000_000;
const MODEL: &str = "Qwen/Qwen3.8-27B";

fn sha256_hex(b: &[u8]) -> String { hex::encode(Sha256::digest(b)) }

fn eth_address(k: &EcKey) -> String {
    let pk = k.verifying_key().to_encoded_point(false);
    let h = Keccak256::digest(&pk.as_bytes()[1..]);
    format!("0x{}", hex::encode(&h[12..]))
}

fn personal_sign(k: &EcKey, text: &str) -> String {
    let msg = format!("\x19Ethereum Signed Message:\n{}{}", text.len(), text);
    let h = Keccak256::digest(msg.as_bytes());
    let (sig, rec) = k.sign_prehash_recoverable(&h).unwrap();
    let mut out = sig.to_bytes().to_vec();
    out.push(27 + rec.to_byte());
    hex::encode(out)
}

fn receipt(k: &EcKey, relevance: f64, salt: &str) -> Value {
    let body = json!({"id": format!("chatcmpl-{salt}"), "object": "chat.completion", "model": MODEL,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": format!("{{\"relevance\": {relevance}}}")}}]}).to_string();
    let text = format!("{MODEL}:{}:{}", sha256_hex(format!("req-{salt}").as_bytes()), sha256_hex(body.as_bytes()));
    json!({"text": text, "signature": personal_sign(k, &text), "response_body": body})
}

#[tokio::test]
async fn lethe_end_to_end() -> anyhow::Result<()> {
    let wasm = std::fs::read("target/near/lethe.wasm").expect("build first: cargo near build non-reproducible-wasm");
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;
    let mk = |name: &'static str, n: u128| { let root = root.clone(); async move {
        root.create_subaccount(name).initial_balance(NearToken::from_near(n)).transact().await.unwrap().into_result().unwrap() } };
    let owner = mk("owner", 10).await;
    let seller = mk("seller", 5).await;
    let buyer_a = mk("buyera", 30).await;
    let buyer_b = mk("buyerb", 30).await;
    let relayer = mk("enclave", 5).await;
    let lethe = mk("lethe", 20).await.deploy(&wasm).await?.into_result()?;

    let enclave = SigningKey::from_bytes(&[7u8; 32]);
    let ai = EcKey::from_slice(&[9u8; 32]).unwrap();
    let rogue_ai = EcKey::from_slice(&[11u8; 32]).unwrap();
    let box_pk = hex::encode([1u8; 32]);

    lethe.call("new").args_json(json!({"owner": owner.id(), "enclave_pk": hex::encode(enclave.verifying_key().to_bytes()),
        "enclave_box_pk": box_pk, "enclave_codehash": "sha256:test"})).transact().await?.into_result()?;
    owner.call(lethe.id(), "add_ai_signer").args_json(json!({"address": eth_address(&ai), "label": "NEAR AI Cloud (test key)", "simulated": true})).transact().await?.into_result()?;
    owner.call(lethe.id(), "allow_model").args_json(json!({"model": MODEL})).transact().await?.into_result()?;
    // non-owner cannot register signers
    assert!(seller.call(lethe.id(), "add_ai_signer").args_json(json!({"address": eth_address(&rogue_ai), "label": "x", "simulated": false})).transact().await?.is_failure());

    let id: u64 = seller.call(lethe.id(), "list").args_json(json!({"title": "memo", "teaser": "t", "ask": (4*ONE_NEAR).to_string(),
        "content_hash": sha256_hex(b"secret memo"), "sealed_content": "c2VhbGVk", "payout": {"chain":"solana","asset":"USDC","address":"9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"}}))
        .transact().await?.json()?;
    assert_eq!(id, 0);

    // budget below ask is rejected before any yield
    assert!(buyer_a.call(lethe.id(), "inspect").args_json(json!({"listing_id": 0, "need": "n", "sealed_corpus": "", "buyer_box_pk": box_pk}))
        .deposit(NearToken::from_near(1)).transact().await?.is_failure());

    // helper: open an inspection asynchronously, wait until it is pending, return the status handle
    async fn open(lethe: &near_workspaces::Contract, buyer: &near_workspaces::Account, budget: u128, box_pk: &str)
        -> anyhow::Result<(near_workspaces::operations::TransactionStatus, u64)> {
        let (_, before) = lethe.view("next_ids").args_json(json!({})).await?.json::<(u64, u64)>()?;
        let st = buyer.call(lethe.id(), "inspect").args_json(json!({"listing_id": 0, "need": "perp funding arbitrage", "sealed_corpus": "Y29ycHVz", "buyer_box_pk": box_pk}))
            .deposit(NearToken::from_yoctonear(budget)).gas(near_workspaces::types::Gas::from_tgas(100)).transact_async().await?;
        for _ in 0..60 {
            let p: Vec<Value> = lethe.view("get_pending").args_json(json!({"from": before, "limit": 10})).await?.json()?;
            if !p.is_empty() { return Ok((st, p[0]["id"].as_u64().unwrap())); }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        anyhow::bail!("request never became pending")
    }
    let sign = |v: &Value| { let s = v.to_string(); (s.clone(), hex::encode(enclave.sign(s.as_bytes()).to_bytes())) };

    // ---- Buyer A: novel + relevant -> DEAL ----
    let a_before = buyer_a.view_account().await?.balance.as_yoctonear();
    let s_before = seller.view_account().await?.balance.as_yoctonear();
    let (st, rid) = open(&lethe, &buyer_a, 10 * ONE_NEAR, &box_pk).await?;
    // a tampered response body (relevance raised after signing) is rejected on-chain
    let mut bad = receipt(&ai, 0.5, "a0");
    bad["response_body"] = Value::String(bad["response_body"].as_str().unwrap().replace("0.5", "1.0"));
    let (vj, sig) = sign(&json!({"request_id": rid, "listing_id": 0, "buyer": buyer_a.id(), "novelty_bps": 9000, "ai_receipt": bad, "delivery": "ZGVsaXZlcnk=", "forget_cert": null}));
    let r = relayer.call(lethe.id(), "respond").args_json(json!({"verdict_json": vj, "signature": sig})).transact().await?;
    assert!(r.is_failure(), "tampered body must fail");
    // a receipt from an unregistered AI key is rejected
    let (vj, sig) = sign(&json!({"request_id": rid, "listing_id": 0, "buyer": buyer_a.id(), "novelty_bps": 9000, "ai_receipt": receipt(&rogue_ai, 1.0, "a1"), "delivery": "ZGVsaXZlcnk=", "forget_cert": null}));
    assert!(relayer.call(lethe.id(), "respond").args_json(json!({"verdict_json": vj, "signature": sig})).transact().await?.is_failure());
    // a verdict not signed by the enclave is rejected
    let forged = SigningKey::from_bytes(&[8u8; 32]);
    let vjs = json!({"request_id": rid, "listing_id": 0, "buyer": buyer_a.id(), "novelty_bps": 10000, "ai_receipt": receipt(&ai, 1.0, "a2"), "delivery": "ZA==", "forget_cert": null}).to_string();
    assert!(relayer.call(lethe.id(), "respond").args_json(json!({"verdict_json": vjs, "signature": hex::encode(forged.sign(vjs.as_bytes()).to_bytes())})).transact().await?.is_failure());
    // the honest verdict
    let (vj, sig) = sign(&json!({"request_id": rid, "listing_id": 0, "buyer": buyer_a.id(), "novelty_bps": 9000, "ai_receipt": receipt(&ai, 0.8, "a3"), "delivery": "ZGVsaXZlcnk=", "forget_cert": null}));
    relayer.call(lethe.id(), "respond").args_json(json!({"verdict_json": vj, "signature": sig})).transact().await?.into_result()?;
    let out: Value = st.await?.json()?;
    println!("buyer A settlement: {out}");
    assert_eq!(out["status"], "Deal");
    // valuation = 10 * 0.9 * 0.8 = 7.2; pay = 4 + (7.2-4)/2 = 5.6 NEAR; refund 4.4
    assert_eq!(out["paid"], (56 * ONE_NEAR / 10).to_string());
    assert_eq!(out["refunded"], (44 * ONE_NEAR / 10).to_string());
    let s_after = seller.view_account().await?.balance.as_yoctonear();
    assert_eq!(s_after - s_before, 56 * ONE_NEAR / 10);
    let a_after = buyer_a.view_account().await?.balance.as_yoctonear();
    assert!(a_before - a_after > 56 * ONE_NEAR / 10 && a_before - a_after < 57 * ONE_NEAR / 10, "buyer pays 5.6 + gas");
    let req: Value = lethe.view("get_request").args_json(json!({"id": rid})).await?.json()?;
    assert!(req["outcome"]["delivery"].is_string());

    // replaying the same AI receipt is rejected
    let (st2, rid2) = open(&lethe, &buyer_b, 10 * ONE_NEAR, &box_pk).await?;
    let r3 = receipt(&ai, 0.8, "a3");
    let (vj, sig) = sign(&json!({"request_id": rid2, "listing_id": 0, "buyer": buyer_b.id(), "novelty_bps": 9000, "ai_receipt": r3, "delivery": "ZA==", "forget_cert": null}));
    assert!(relayer.call(lethe.id(), "respond").args_json(json!({"verdict_json": vj, "signature": sig})).transact().await?.is_failure());

    // ---- Buyer B: already knows most of it -> NO DEAL, full refund, forget certificate ----
    let b_before = buyer_b.view_account().await?.balance.as_yoctonear();
    let cert = json!({"type": "lethe/forget-certificate/v1", "request_id": rid2, "zeroized": ["seller_plaintext", "buyer_corpus", "session_key"]}).to_string();
    let (vj, sig) = sign(&json!({"request_id": rid2, "listing_id": 0, "buyer": buyer_b.id(), "novelty_bps": 3000, "ai_receipt": receipt(&ai, 0.8, "b1"), "delivery": null, "forget_cert": cert}));
    relayer.call(lethe.id(), "respond").args_json(json!({"verdict_json": vj, "signature": sig})).transact().await?.into_result()?;
    let out: Value = st2.await?.json()?;
    println!("buyer B settlement: {out}");
    assert_eq!(out["status"], "NoDeal");
    assert_eq!(out["paid"], "0");
    assert_eq!(out["refunded"], (10 * ONE_NEAR).to_string());
    assert_eq!(out["forget_cert_hash"], sha256_hex(cert.as_bytes()));
    let b_after = buyer_b.view_account().await?.balance.as_yoctonear();
    assert!(b_after + ONE_NEAR / 100 > b_before, "refund returned (minus small gas)");

    // ---- Timeout: enclave never answers -> refund ----
    let (st3, rid3) = open(&lethe, &buyer_b, 5 * ONE_NEAR, &box_pk).await?;
    worker.fast_forward(250).await?;
    let out: Value = st3.await?.json()?;
    println!("timeout settlement: {out}");
    assert_eq!(out["status"], "TimedOut");
    let req: Value = lethe.view("get_request").args_json(json!({"id": rid3})).await?.json()?;
    assert_eq!(req["status"], "TimedOut");
    Ok(())
}
