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
    pub name: Option<String>,
    pub children: usize,
    pub logs: std::ops::Range<usize>,
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
    pub failure: Option<crate::diagnose::Failure>,
    pub slot: u64,
    pub nodes: Vec<Node>,
    pub accounts: Vec<Account>,
    pub balances: Vec<BalanceChange>,
    pub logs: Vec<String>,
}

fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache/solwork")
}

fn cache_path(cluster: &str, key: &str) -> PathBuf {
    cache_dir().join(cluster).join(format!("{key}.json.zst"))
}

pub fn cached(cluster: &str) -> Vec<String> {
    let mut entries: Vec<(std::time::SystemTime, String)> = fs::read_dir(cache_dir().join(cluster))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let key = name.strip_suffix(".json.zst")?.to_string();
            Some((e.metadata().ok()?.modified().ok()?, key))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    entries.into_iter().map(|(_, key)| key).collect()
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Tx,
    Account,
}

pub struct Entry {
    pub kind: Kind,
    pub key: String,
    pub status: Option<Status>,
    pub detail: String,
}

pub fn entries(cluster: &str) -> Vec<Entry> {
    cached(cluster)
        .into_iter()
        .filter(|key| !key.starts_with("idl-"))
        .map(|key| match key.strip_prefix("account-") {
            Some(pubkey) => {
                let account = load(cluster, &key).map(|value| parse_account(pubkey, &value));
                Entry {
                    kind: Kind::Account,
                    detail: account.map_or_else(
                        || "cached".to_string(),
                        |a| format!("{:>5} B  {}", a.data.len(), a.owner_name()),
                    ),
                    key: pubkey.to_string(),
                    status: None,
                }
            }
            None => {
                let tx = load(cluster, &key).map(|value| parse(&key, cluster, &value));
                Entry {
                    kind: Kind::Tx,
                    detail: tx.as_ref().map_or_else(
                        || "cached".to_string(),
                        |tx| {
                            format!(
                                "{:>2} ix  {:>9} CU",
                                tx.nodes.iter().filter(|n| n.depth == 0).count(),
                                tx.cu
                            )
                        },
                    ),
                    status: tx.map(|tx| tx.status),
                    key,
                }
            }
        })
        .collect()
}

pub fn load(cluster: &str, key: &str) -> Option<Value> {
    let bytes = fs::read(cache_path(cluster, key)).ok()?;
    serde_json::from_slice(&zstd::decode_all(&bytes[..]).ok()?).ok()
}

fn store(cluster: &str, key: &str, value: &Value) -> io::Result<()> {
    let path = cache_path(cluster, key);
    fs::create_dir_all(path.parent().unwrap_or(&cache_dir()))?;
    let raw = serde_json::to_vec(value)?;
    fs::write(path, zstd::encode_all(&raw[..], 19)?)
}

pub fn refetch(cluster: &str, signature: &str) -> Result<Value, Box<dyn Error>> {
    let _ = fs::remove_file(cache_path(cluster, signature));
    fetch(cluster, signature)
}

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

impl Tx {
    pub fn failure_path(&self) -> Vec<usize> {
        let Some(failed) = self.failed_node else { return Vec::new() };
        let mut path = vec![failed];
        let mut depth = self.nodes[failed].depth;
        for i in (0..failed).rev() {
            if self.nodes[i].depth < depth {
                path.push(i);
                depth = self.nodes[i].depth;
                if depth == 0 {
                    break;
                }
            }
        }
        path.reverse();
        path
    }
}

pub fn cluster_url(name: &str) -> String {
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
        url => format!("?cluster=custom&customUrl={}", urlencode(url)),
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

    for (line_index, line) in logs.iter().enumerate() {
        let Some(rest) = line.strip_prefix("Program ") else { continue };
        if let Some(name) = rest.strip_prefix("log: Instruction: ") {
            if let Some(&index) = stack.last() {
                nodes[index].name.get_or_insert_with(|| name.trim().to_string());
            }
            continue;
        }
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
                name: None,
                data: instruction
                    .as_ref()
                    .and_then(|ix| ix["data"].as_str().map(str::to_string))
                    .unwrap_or_default(),
                children: 0,
                logs: line_index..line_index + 1,
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
            if let Some(idx) = stack.pop() {
                nodes[idx].logs.end = line_index + 1;
            }
        } else if let Some((_, message)) = rest.split_once(" failed: ") {
            if let Some(idx) = stack.pop() {
                nodes[idx].status = Status::Failed;
                nodes[idx].error = Some(message.to_string());
                nodes[idx].logs.end = line_index + 1;
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

    let cu = result["meta"]["computeUnitsConsumed"].as_u64().unwrap_or(0) as u32;
    let cu_limit = if cu_limit == 0 { 200_000 } else { cu_limit };
    let failure = crate::diagnose::failure(&result["meta"]["err"], &nodes, &logs, cu, cu_limit);
    let failed_node = failed_node.or_else(|| failure.as_ref().and_then(|f| f.node));

    Tx {
        signature: signature.to_string(),
        cluster: cluster.to_string(),
        cu,
        cu_limit,
        fee: result["meta"]["fee"].as_u64().unwrap_or(0),
        slot: result["slot"].as_u64().unwrap_or(0),
        status: if error.is_some() { Status::Failed } else { Status::Ok },
        error,
        failed_node,
        failure,
        balances: balance_changes(result, &keys),
        accounts: keys,
        logs,
        nodes,
    }
}

pub struct AccountInfo {
    pub key: String,
    pub lamports: u64,
    pub owner: String,
    pub executable: bool,
    pub rent_epoch: u64,
    pub data: Vec<u8>,
}

impl AccountInfo {
    pub fn owner_name(&self) -> String {
        program_name(&self.owner)
    }
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

pub fn accounts_info(cluster: &str, addresses: &[String]) -> Result<Vec<Value>, Box<dyn Error>> {
    let value = rpc(cluster, "getMultipleAccounts", json!([addresses, {"encoding": "base64"}]))?;
    Ok(value
        .pointer("/result/value")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter(|v| !v.is_null()).cloned().collect())
        .unwrap_or_default())
}

pub fn store_value(cluster: &str, key: &str, value: &Value) -> io::Result<()> {
    store(cluster, key, value)
}

pub fn refetch_account(cluster: &str, pubkey: &str) -> Result<Value, Box<dyn Error>> {
    let _ = fs::remove_file(cache_path(cluster, &format!("account-{pubkey}")));
    fetch_account(cluster, pubkey)
}

pub fn parse_account(pubkey: &str, value: &Value) -> AccountInfo {
    let account = &value["result"]["value"];
    let data = account["data"][0].as_str().map(base64_decode).unwrap_or_default();
    AccountInfo {
        key: pubkey.to_string(),
        lamports: account["lamports"].as_u64().unwrap_or(0),
        owner: account["owner"].as_str().unwrap_or_default().to_string(),
        executable: account["executable"].as_bool().unwrap_or(false),
        rent_epoch: account["rentEpoch"].as_u64().unwrap_or(0),
        data,
    }
}

pub fn base64_decode(input: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes().filter(|b| *b != b'=') {
        let Some(value) = TABLE.iter().position(|t| *t == byte) else { continue };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

pub fn is_signature(input: &str) -> bool {
    input.len() >= 80
}

const BS58: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

pub fn bs58_encode(bytes: &[u8]) -> String {
    let mut digits: Vec<u8> = Vec::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let mut carry = *byte as usize;
        for digit in digits.iter_mut() {
            carry += (*digit as usize) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let leading = bytes.iter().take_while(|b| **b == 0).count();
    let mut out = String::from("1".repeat(leading));
    out.extend(digits.iter().rev().map(|d| BS58[*d as usize] as char));
    if out.is_empty() { "1".to_string() } else { out }
}

pub fn bs58_decode(input: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();
    for c in input.chars() {
        let Some(value) = BS58.iter().position(|b| *b as char == c) else { continue };
        let mut carry = value;
        for byte in bytes.iter_mut() {
            carry += (*byte as usize) * 58;
            *byte = (carry & 0xff) as u8;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    bytes.extend(std::iter::repeat_n(0u8, input.chars().take_while(|c| *c == '1').count()));
    bytes.reverse();
    bytes
}

fn u64_at(data: &[u8], offset: usize) -> u64 {
    data.get(offset..offset + 8)
        .map(|slice| u64::from_le_bytes(slice.try_into().unwrap_or([0; 8])))
        .unwrap_or(0)
}

fn key_at(data: &[u8], offset: usize) -> String {
    data.get(offset..offset + 32).map(bs58_encode).unwrap_or_default()
}

fn option_key(data: &[u8], offset: usize) -> String {
    match data.get(offset..offset + 4).map(|t| u32::from_le_bytes(t.try_into().unwrap_or([0; 4]))) {
        Some(1) => key_at(data, offset + 4),
        _ => "none".to_string(),
    }
}

pub fn decode_account(account: &AccountInfo) -> Option<(&'static str, Vec<(String, String)>)> {
    let data = &account.data;
    let token = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
    let token22 = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
    if account.owner != token && account.owner != token22 {
        return None;
    }
    match data.len() {
        82 => Some((
            "SPL Mint",
            vec![
                ("mint_authority".into(), option_key(data, 0)),
                ("supply".into(), u64_at(data, 36).to_string()),
                ("decimals".into(), data[44].to_string()),
                ("is_initialized".into(), (data[45] == 1).to_string()),
                ("freeze_authority".into(), option_key(data, 46)),
            ],
        )),
        165 => Some((
            "SPL Token Account",
            vec![
                ("mint".into(), key_at(data, 0)),
                ("owner".into(), key_at(data, 32)),
                ("amount".into(), u64_at(data, 64).to_string()),
                ("delegate".into(), option_key(data, 72)),
                (
                    "state".into(),
                    match data[108] {
                        0 => "uninitialized",
                        1 => "initialized",
                        2 => "frozen",
                        _ => "unknown",
                    }
                    .to_string(),
                ),
                ("delegated_amount".into(), u64_at(data, 121).to_string()),
                ("close_authority".into(), option_key(data, 129)),
            ],
        )),
        _ => None,
    }
}

pub fn decode_instruction(program: &str, data: &str) -> Option<(String, Vec<(String, String)>)> {
    let bytes = bs58_decode(data);
    if bytes.is_empty() {
        return None;
    }
    match program {
        "11111111111111111111111111111111" => {
            let index = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
            let name = match index {
                0 => "CreateAccount",
                1 => "Assign",
                2 => "Transfer",
                3 => "CreateAccountWithSeed",
                8 => "Allocate",
                _ => "Unknown",
            };
            let mut fields = vec![("instruction".into(), format!("{name} ({index})"))];
            if index == 2 {
                fields.push(("lamports".into(), u64_at(&bytes, 4).to_string()));
            }
            Some((name.to_string(), fields))
        }
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" | "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => {
            let tag = bytes[0];
            let name = match tag {
                0 => "InitializeMint",
                1 => "InitializeAccount",
                3 => "Transfer",
                7 => "MintTo",
                8 => "Burn",
                9 => "CloseAccount",
                12 => "TransferChecked",
                _ => "Unknown",
            };
            let mut fields = vec![("instruction".into(), format!("{name} ({tag})"))];
            if matches!(tag, 3 | 7 | 8 | 12) {
                fields.push(("amount".into(), u64_at(&bytes, 1).to_string()));
            }
            if tag == 12 {
                fields.push(("decimals".into(), bytes.get(9).map_or(0, |b| *b).to_string()));
            }
            Some((name.to_string(), fields))
        }
        "ComputeBudget111111111111111111111111111111" => {
            let name = match bytes[0] {
                2 => "SetComputeUnitLimit",
                3 => "SetComputeUnitPrice",
                _ => "Unknown",
            };
            let value = bytes
                .get(1..5)
                .map(|v| u32::from_le_bytes(v.try_into().unwrap_or([0; 4])))
                .unwrap_or(0);
            Some((name.to_string(), vec![("value".into(), value.to_string())]))
        }
        _ => None,
    }
}

pub fn hex_dump(data: &[u8], offset: usize, rows: usize) -> Vec<(String, String, String)> {
    data.chunks(16)
        .skip(offset)
        .take(rows)
        .enumerate()
        .map(|(row, chunk)| {
            let hex = chunk
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .chunks(8)
                .map(|half| half.join(" "))
                .collect::<Vec<_>>()
                .join("  ");
            let ascii = chunk
                .iter()
                .map(|b| if b.is_ascii_graphic() { *b as char } else { '·' })
                .collect();
            (format!("{:08x}", (offset + row) * 16), hex, ascii)
        })
        .collect()
}

pub struct Transfer {
    pub node: usize,
    pub kind: String,
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub unit: String,
}

pub fn transfers(tx: &Tx) -> Vec<Transfer> {
    let mut out = Vec::new();
    for (node, instruction) in tx.nodes.iter().enumerate() {
        let bytes = bs58_decode(&instruction.data);
        let account = |i: usize| instruction.accounts.get(i).map(|a| a.key.clone()).unwrap_or_default();
        match instruction.program.as_str() {
            "11111111111111111111111111111111" if bytes.len() >= 12 => {
                if u32::from_le_bytes(bytes[0..4].try_into().unwrap_or([0; 4])) != 2 {
                    continue;
                }
                out.push(Transfer {
                    node,
                    kind: "System::Transfer".to_string(),
                    from: account(0),
                    to: account(1),
                    amount: u64_at(&bytes, 4) as f64 / 1e9,
                    unit: "SOL".to_string(),
                });
            }
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" | "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => {
                let Some(tag) = bytes.first().copied() else { continue };
                let raw = u64_at(&bytes, 1) as f64;
                let (kind, from, to, amount, unit) = match tag {
                    3 => ("Token::Transfer", account(0), account(1), raw, "tokens".to_string()),
                    12 => {
                        let decimals = bytes.get(9).copied().unwrap_or(0) as i32;
                        (
                            "Token::TransferChecked",
                            account(0),
                            account(2),
                            raw / 10f64.powi(decimals),
                            crate::ui::short(&account(1)),
                        )
                    }
                    7 => ("Token::MintTo", account(0), account(1), raw, "tokens".to_string()),
                    8 => ("Token::Burn", account(0), account(1), raw, "tokens".to_string()),
                    _ => continue,
                };
                out.push(Transfer { node, kind: kind.to_string(), from, to, amount, unit });
            }
            _ => {}
        }
    }
    out
}
