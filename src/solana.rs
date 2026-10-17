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

pub struct Account {
    pub key: String,
    pub signer: bool,
    pub writable: bool,
}

impl Account {
    pub fn role(&self) -> &'static str {
        match (self.signer, self.writable) {
            (true, true) => "signer, writable",
            (true, false) => "signer",
            (false, true) => "writable",
            (false, false) => "readonly",
        }
    }
}

pub struct Node {
    pub depth: usize,
    pub path: String,
    pub label: String,
    pub program: String,
    pub cu: u32,
    pub self_cu: u32,
    pub status: Status,
    pub error: Option<String>,
    pub accounts: Vec<Account>,
    pub data: String,
    pub children: usize,
}

pub struct Tx {
    pub signature: String,
    pub cluster: String,
    pub cu: u32,
    pub cu_limit: u32,
    pub fee: u64,
    pub status: Status,
    pub error: Option<String>,
    pub failed_node: Option<usize>,
    pub slot: u64,
    pub nodes: Vec<Node>,
    pub accounts: Vec<Account>,
    pub balances: Vec<BalanceChange>,
    pub logs: Vec<String>,
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

pub fn refetch(cluster: &str, signature: &str) -> Result<Value, Box<dyn Error>> {
    let _ = fs::remove_file(cache_path(signature));
    fetch(cluster, signature)
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

fn account_roles(result: &Value) -> Vec<Account> {
    let str_list = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|k| k.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let statics = str_list(result.pointer("/transaction/message/accountKeys"));
    let loaded_writable = str_list(result.pointer("/meta/loadedAddresses/writable"));
    let loaded_readonly = str_list(result.pointer("/meta/loadedAddresses/readonly"));

    let header = |name: &str| {
        result
            .pointer(&format!("/transaction/message/header/{name}"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize
    };
    let signers = header("numRequiredSignatures");
    let readonly_signed = header("numReadonlySignedAccounts");
    let readonly_unsigned = header("numReadonlyUnsignedAccounts");
    let static_len = statics.len();

    let mut accounts: Vec<Account> = statics
        .into_iter()
        .enumerate()
        .map(|(i, key)| Account {
            key,
            signer: i < signers,
            writable: if i < signers {
                i < signers - readonly_signed.min(signers)
            } else {
                i < static_len - readonly_unsigned.min(static_len)
            },
        })
        .collect();
    accounts.extend(loaded_writable.into_iter().map(|key| Account { key, signer: false, writable: true }));
    accounts.extend(loaded_readonly.into_iter().map(|key| Account { key, signer: false, writable: false }));
    accounts
}

fn instruction_accounts(instruction: &Value, keys: &[Account]) -> Vec<Account> {
    instruction["accounts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i.as_u64())
                .map(|i| match keys.get(i as usize) {
                    Some(account) => Account {
                        key: account.key.clone(),
                        signer: account.signer,
                        writable: account.writable,
                    },
                    None => Account { key: format!("#{i}"), signer: false, writable: false },
                })
                .collect()
        })
        .unwrap_or_default()
}

pub struct BalanceChange {
    pub key: String,
    pub label: String,
    pub before: f64,
    pub after: f64,
}

fn balance_changes(result: &Value, accounts: &[Account]) -> Vec<BalanceChange> {
    let lamports = |name: &str| -> Vec<u64> {
        result
            .pointer(&format!("/meta/{name}"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default()
    };
    let (pre, post) = (lamports("preBalances"), lamports("postBalances"));
    let mut changes: Vec<BalanceChange> = pre
        .iter()
        .zip(post.iter())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (a, b))| BalanceChange {
            key: accounts.get(i).map(|x| x.key.clone()).unwrap_or_default(),
            label: "SOL".to_string(),
            before: *a as f64 / 1e9,
            after: *b as f64 / 1e9,
        })
        .collect();

    let token = |name: &str| -> Vec<(u64, String, f64)> {
        result
            .pointer(&format!("/meta/{name}"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|t| {
                        (
                            t["accountIndex"].as_u64().unwrap_or(0),
                            t["mint"].as_str().unwrap_or("").to_string(),
                            t.pointer("/uiTokenAmount/uiAmount").and_then(Value::as_f64).unwrap_or(0.0),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let post_tokens = token("postTokenBalances");
    for (index, mint, before) in token("preTokenBalances") {
        let after = post_tokens
            .iter()
            .find(|(i, m, _)| *i == index && *m == mint)
            .map(|(_, _, v)| *v)
            .unwrap_or(0.0);
        if before != after {
            changes.push(BalanceChange {
                key: accounts.get(index as usize).map(|x| x.key.clone()).unwrap_or_default(),
                label: program_name(&mint),
                before,
                after,
            });
        }
    }
    changes
}

pub fn parse(signature: &str, cluster: &str, value: &Value) -> Tx {
    let result = &value["result"];
    let keys = account_roles(result);
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
    let mut counters: Vec<usize> = Vec::new();
    let mut top_cursor = 0usize;
    let mut inner_cursor = 0usize;
    let mut cu_limit = 0u32;

    let logs: Vec<String> = result
        .pointer("/meta/logMessages")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();

    for line in &logs {
        let Some(rest) = line.strip_prefix("Program ") else { continue };
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

            counters.truncate(depth);
            while counters.len() < depth {
                counters.push(0);
            }
            counters[depth - 1] += 1;
            let path = counters.iter().map(usize::to_string).collect::<Vec<_>>().join(".");

            stack.push(nodes.len());
            nodes.push(Node {
                depth: depth - 1,
                path,
                label: program_name(id),
                program: id.to_string(),
                cu: 0,
                self_cu: 0,
                status: Status::Ok,
                error: None,
                accounts: instruction
                    .as_ref()
                    .map(|ix| instruction_accounts(ix, &keys))
                    .unwrap_or_default(),
                data: instruction
                    .as_ref()
                    .and_then(|ix| ix["data"].as_str().map(str::to_string))
                    .unwrap_or_default(),
                children: 0,
            });
            continue;
        }
        if let Some((_, tail)) = rest.split_once(" consumed ") {
            let mut parts = tail.split(' ');
            let consumed = parts.next().and_then(|v| v.parse::<u32>().ok());
            let budget = parts.nth(1).and_then(|v| v.parse::<u32>().ok());
            if let (Some(&idx), Some(cu)) = (stack.last(), consumed) {
                nodes[idx].cu = cu;
                if nodes[idx].depth == 0 {
                    cu_limit = cu_limit.max(budget.unwrap_or(0));
                }
            }
            continue;
        }
        if rest.ends_with(" success") {
            stack.pop();
        } else if let Some((_, message)) = rest.split_once(" failed: ") {
            if let Some(idx) = stack.pop() {
                nodes[idx].status = Status::Failed;
                nodes[idx].error = Some(message.to_string());
            }
        }
    }

    for i in 0..nodes.len() {
        let mut children_cu = 0;
        let mut children = 0;
        for j in i + 1..nodes.len() {
            if nodes[j].depth <= nodes[i].depth {
                break;
            }
            if nodes[j].depth == nodes[i].depth + 1 {
                children += 1;
                children_cu += nodes[j].cu;
            }
        }
        nodes[i].children = children;
        nodes[i].self_cu = nodes[i].cu.saturating_sub(children_cu);
    }

    let failed_node = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.status == Status::Failed)
        .max_by_key(|(_, n)| n.depth)
        .map(|(i, _)| i);
    let error = result["meta"]["err"]
        .is_null()
        .then_some(None)
        .unwrap_or_else(|| {
            nodes
                .iter()
                .find(|n| n.error.is_some())
                .and_then(|n| n.error.clone())
                .or_else(|| Some(result["meta"]["err"].to_string()))
        });

    Tx {
        signature: signature.to_string(),
        cluster: cluster.to_string(),
        cu: result["meta"]["computeUnitsConsumed"].as_u64().unwrap_or(0) as u32,
        cu_limit: if cu_limit == 0 { 200_000 } else { cu_limit },
        fee: result["meta"]["fee"].as_u64().unwrap_or(0),
        slot: result["slot"].as_u64().unwrap_or(0),
        status: if error.is_some() { Status::Failed } else { Status::Ok },
        error,
        failed_node,
        balances: balance_changes(result, &keys),
        accounts: keys,
        logs,
        nodes,
    }
}
