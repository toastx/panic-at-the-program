use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Ok,
    Failed,
}

pub struct Node {
    pub depth: usize,
    pub label: String,
    pub program: String,
    pub cu: u32,
    pub status: Status,
    pub accounts: Vec<(String, Status)>,
}

pub struct Tx {
    pub signature: String,
    pub cluster: String,
    pub cu: u32,
    pub status: Status,
    pub nodes: Vec<Node>,
}

pub fn cluster_url(name: &str) -> String {
    match name {
        "devnet" | "mainnet" | "testnet" => format!("https://api.{name}-beta.solana.com")
            .replace("api.devnet-beta", "api.devnet")
            .replace("api.testnet-beta", "api.testnet"),
        url => url.to_string(),
    }
}

fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache/solwork")
}

fn cache_path(signature: &str) -> PathBuf {
    cache_dir().join(format!("{signature}.json.zst"))
}

pub fn cached() -> Vec<String> {
    let mut entries: Vec<(std::time::SystemTime, String)> = fs::read_dir(cache_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let sig = name.strip_suffix(".json.zst")?.to_string();
            Some((e.metadata().ok()?.modified().ok()?, sig))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    entries.into_iter().map(|(_, sig)| sig).collect()
}

pub struct Summary {
    pub signature: String,
    pub status: Status,
    pub cu: u32,
    pub instructions: usize,
}

pub fn summaries() -> Vec<Summary> {
    cached()
        .into_iter()
        .map(|signature| match load(&signature) {
            Some(value) => {
                let tx = parse(&signature, "", &value);
                Summary {
                    signature,
                    status: tx.status,
                    cu: tx.cu,
                    instructions: tx.nodes.iter().filter(|n| n.depth == 0).count(),
                }
            }
            None => Summary { signature, status: Status::Ok, cu: 0, instructions: 0 },
        })
        .collect()
}

pub fn load(signature: &str) -> Option<Value> {
    let bytes = fs::read(cache_path(signature)).ok()?;
    serde_json::from_slice(&zstd::decode_all(&bytes[..]).ok()?).ok()
}

fn store(signature: &str, value: &Value) -> io::Result<()> {
    fs::create_dir_all(cache_dir())?;
    let raw = serde_json::to_vec(value)?;
    fs::write(cache_path(signature), zstd::encode_all(&raw[..], 19)?)
}

pub fn fetch(cluster: &str, signature: &str) -> Result<Value, Box<dyn Error>> {
    if let Some(value) = load(signature) {
        return Ok(value);
    }
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [signature, {"encoding": "json", "maxSupportedTransactionVersion": 1}],
    });
    let mut resp = ureq::post(cluster_url(cluster)).send_json(body)?;
    let value: Value = resp.body_mut().read_json()?;
    if let Some(err) = value.get("error") {
        return Err(format!("rpc error: {err}").into());
    }
    if value.get("result").is_none_or(Value::is_null) {
        return Err(format!("transaction not found on {cluster}").into());
    }
    store(signature, &value)?;
    Ok(value)
}

fn program_name(id: &str) -> String {
    match id {
        "11111111111111111111111111111111" => "System Program",
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" => "Token Program",
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => "Token-2022",
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL" => "Associated Token",
        "ComputeBudget111111111111111111111111111111" => "Compute Budget",
        "Vote111111111111111111111111111111111111111" => "Vote Program",
        other => return format!("{}…{}", &other[..4.min(other.len())], &other[other.len().saturating_sub(4)..]),
    }
    .to_string()
}

fn account_keys(result: &Value) -> Vec<String> {
    let str_list = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|k| k.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let mut keys = str_list(result.pointer("/transaction/message/accountKeys"));
    keys.extend(str_list(result.pointer("/meta/loadedAddresses/writable")));
    keys.extend(str_list(result.pointer("/meta/loadedAddresses/readonly")));
    keys
}

fn instruction_accounts(instruction: &Value, keys: &[String], status: Status) -> Vec<(String, Status)> {
    instruction["accounts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i.as_u64())
                .map(|i| {
                    let key = keys.get(i as usize).cloned().unwrap_or_else(|| format!("#{i}"));
                    (key, status)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse(signature: &str, cluster: &str, value: &Value) -> Tx {
    let result = &value["result"];
    let keys = account_keys(result);
    let empty = Vec::new();
    let top: &Vec<Value> = result
        .pointer("/transaction/message/instructions")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let inner: Vec<&Value> = result
        .pointer("/meta/innerInstructions")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();

    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut top_cursor = 0usize;
    let mut inner_cursor = 0usize;

    let logs = result.pointer("/meta/logMessages").and_then(Value::as_array).unwrap_or(&empty);
    for line in logs.iter().filter_map(Value::as_str) {
        if let Some(rest) = line.strip_prefix("Program ") {
            if rest.starts_with("log:") || rest.starts_with("data:") || rest.starts_with("return:") {
                continue;
            }
            if let Some((id, tail)) = rest.split_once(" invoke [") {
                let depth: usize = tail.trim_end_matches(']').parse().unwrap_or(1);
                let instruction = if depth == 1 {
                    let i = top_cursor;
                    top_cursor += 1;
                    inner_cursor = 0;
                    top.get(i).cloned()
                } else {
                    let i = inner_cursor;
                    inner_cursor += 1;
                    inner
                        .iter()
                        .find(|g| g["index"].as_u64() == Some(top_cursor as u64 - 1))
                        .and_then(|g| g["instructions"].as_array())
                        .and_then(|list| list.get(i).cloned())
                };
                stack.push(nodes.len());
                nodes.push(Node {
                    depth: depth - 1,
                    label: program_name(id),
                    program: id.to_string(),
                    cu: 0,
                    status: Status::Ok,
                    accounts: instruction
                        .map(|ix| instruction_accounts(&ix, &keys, Status::Ok))
                        .unwrap_or_default(),
                });
                continue;
            }
            if let Some(consumed) = rest.split_once(" consumed ").and_then(|(_, t)| t.split(' ').next()) {
                if let (Some(&idx), Ok(cu)) = (stack.last(), consumed.parse::<u32>()) {
                    nodes[idx].cu = cu;
                }
                continue;
            }
            if rest.ends_with(" success") {
                stack.pop();
            } else if rest.contains(" failed:") {
                if let Some(idx) = stack.pop() {
                    nodes[idx].status = Status::Failed;
                    for account in &mut nodes[idx].accounts {
                        account.1 = Status::Failed;
                    }
                }
            }
        }
    }

    let failed = !result["meta"]["err"].is_null();
    Tx {
        signature: signature.to_string(),
        cluster: cluster.to_string(),
        cu: result["meta"]["computeUnitsConsumed"].as_u64().unwrap_or(0) as u32,
        status: if failed { Status::Failed } else { Status::Ok },
        nodes,
    }
}
