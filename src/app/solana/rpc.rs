//! Talking to a cluster: endpoints, JSON-RPC calls, and the fetches that go
//! through the disk cache first.

use std::error::Error;

use serde_json::{Value, json};

use super::cache::{load, store};

fn rpc(cluster: &str, method: &str, params: Value) -> Result<Value, Box<dyn Error>> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let mut resp = ureq::post(cluster_url(cluster)).send_json(body)?;
    let value: Value = resp.body_mut().read_json()?;
    if let Some(err) = value.get("error") {
        let message = err["message"].as_str().unwrap_or("unknown").to_string();
        return Err(message.into());
    }
    Ok(value)
}

/// Whether a validator is answering on `cluster` right now.
pub fn healthy(cluster: &str) -> bool {
    rpc(cluster, "getHealth", json!([])).is_ok()
}

/// Best effort top-up: a validator that refuses airdrops is not our problem, the
/// suite will say so itself.
pub fn airdrop(cluster: &str, pubkey: &str, lamports: u64) -> bool {
    rpc(cluster, "requestAirdrop", json!([pubkey, lamports])).is_ok()
}

pub fn fetch(cluster: &str, signature: &str) -> Result<Value, Box<dyn Error>> {
    if let Some(value) = load(cluster, signature) {
        return Ok(value);
    }
    let value = rpc(
        cluster,
        "getTransaction",
        json!([signature, {"encoding": "json", "maxSupportedTransactionVersion": 1}]),
    )?;
    if value.get("result").is_none_or(Value::is_null) {
        return Err(format!("transaction not found on {cluster}").into());
    }
    store(cluster, signature, &value)?;
    Ok(value)
}

pub fn cluster_url(name: &str) -> String {
    if let Some((_, url)) = crate::workspace::global("rpc").into_iter().find(|(key, _)| key == name) {
        return url;
    }
    match name {
        "mainnet" => "https://api.mainnet-beta.solana.com".to_string(),
        "devnet" => "https://api.devnet.solana.com".to_string(),
        "testnet" => "https://api.testnet.solana.com".to_string(),
        "localnet" => "http://127.0.0.1:8899".to_string(),
        url => url.to_string(),
    }
}

pub fn explorer_url(cluster: &str, kind: &str, key: &str) -> String {
    let suffix = match cluster {
        "mainnet" => String::new(),
        "devnet" | "testnet" => format!("?cluster={cluster}"),
        "localnet" => "?cluster=custom&customUrl=http%3A%2F%2F127.0.0.1%3A8899".to_string(),
        name => format!("?cluster=custom&customUrl={}", urlencode(&cluster_url(name))),
    };
    format!("https://explorer.solana.com/{kind}/{key}{suffix}")
}

fn urlencode(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

pub fn fetch_account(cluster: &str, pubkey: &str) -> Result<Value, Box<dyn Error>> {
    let key = format!("account-{pubkey}");
    if let Some(value) = load(cluster, &key) {
        return Ok(value);
    }
    let value = rpc(cluster, "getAccountInfo", json!([pubkey, {"encoding": "base64"}]))?;
    if value.pointer("/result/value").is_none_or(Value::is_null) {
        return Err(format!("account not found on {cluster}").into());
    }
    store(cluster, &key, &value)?;
    Ok(value)
}

pub fn accounts_info_sparse(cluster: &str, addresses: &[String]) -> Result<Vec<Option<Value>>, Box<dyn Error>> {
    let value = rpc(
        cluster,
        "getMultipleAccounts",
        json!([addresses, {"encoding": "base64"}]),
    )?;
    Ok(value
        .pointer("/result/value")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|v| if v.is_null() { None } else { Some(v.clone()) })
                .collect()
        })
        .unwrap_or_default())
}

pub fn minimum_balance(cluster: &str, size: usize) -> Result<u64, Box<dyn Error>> {
    let value = rpc(cluster, "getMinimumBalanceForRentExemption", json!([size]))?;
    value["result"].as_u64().ok_or_else(|| "no result".into())
}
