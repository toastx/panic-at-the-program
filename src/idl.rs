use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use curve25519_dalek::edwards::CompressedEdwardsY;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::solana::{self, bs58_decode, bs58_encode};

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<Value>>> = RefCell::new(HashMap::new());
}

pub fn idl(cluster: &str, program: &str) -> Option<Value> {
    let key = format!("{cluster}/{program}");
    if let Some(hit) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return hit;
    }
    let found = local(program)
        .or_else(|| cached(cluster, program))
        .or_else(|| fetch(cluster, program));
    CACHE.with(|cache| cache.borrow_mut().insert(key, found.clone()));
    found
}

fn local(program: &str) -> Option<Value> {
    let dir = crate::workspace::idl_dir();
    if let Some(path) = crate::workspace::idl_path(program) {
        return read_json(&path);
    }
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some(value) = read_json(&path) else { continue };
        if address(&value).as_deref() == Some(program) {
            return Some(value);
        }
    }
    None
}

fn read_json(path: &PathBuf) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn address(idl: &Value) -> Option<String> {
    idl["address"]
        .as_str()
        .or_else(|| idl.pointer("/metadata/address").and_then(Value::as_str))
        .map(str::to_string)
}

fn cache_key(program: &str) -> String {
    format!("idl-{program}")
}

fn cached(cluster: &str, program: &str) -> Option<Value> {
    match solana::load(cluster, &cache_key(program))? {
        Value::Null => Some(Value::Null),
        value => Some(value),
    }
    .filter(|value| !value.is_null())
}

fn fetch(cluster: &str, program: &str) -> Option<Value> {
    let addresses = candidates(program);
    let accounts = solana::accounts_info(cluster, &addresses).ok()?;
    let found = accounts
        .iter()
        .find(|account| account["owner"].as_str() == Some(program))
        .and_then(|account| account.pointer("/data/0").and_then(Value::as_str))
        .and_then(inflate_account);
    let _ = solana::store_value(cluster, &cache_key(program), found.as_ref().unwrap_or(&Value::Null));
    found
}

fn inflate_account(base64: &str) -> Option<Value> {
    let bytes = solana::base64_decode(base64);
    let length = u32::from_le_bytes(bytes.get(40..44)?.try_into().ok()?) as usize;
    let compressed = bytes.get(44..44 + length)?;
    let mut json = Vec::new();
    flate2::read::ZlibDecoder::new(compressed).read_to_end(&mut json).ok()?;
    serde_json::from_slice(&json).ok()
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn off_curve(bytes: &[u8; 32]) -> bool {
    CompressedEdwardsY(*bytes).decompress().is_none()
}

pub fn find_program_address(seeds: &[&[u8]], program: &[u8]) -> Option<([u8; 32], u8)> {
    for bump in (0..=255u8).rev() {
        let mut parts: Vec<&[u8]> = seeds.to_vec();
        let suffix = [bump];
        parts.push(&suffix);
        parts.push(program);
        parts.push(b"ProgramDerivedAddress");
        let candidate = sha256(&parts);
        if off_curve(&candidate) {
            return Some((candidate, bump));
        }
    }
    None
}

fn candidates(program: &str) -> Vec<String> {
    let bytes = bs58_decode(program);
    let Some((base, _)) = find_program_address(&[], &bytes) else { return Vec::new() };
    vec![bs58_encode(&sha256(&[&base, b"anchor:idl", &bytes]))]
}

pub fn error(idl: &Value, code: u32) -> Option<(String, String)> {
    idl["errors"].as_array()?.iter().find_map(|entry| {
        (entry["code"].as_u64()? as u32 == code).then(|| {
            (
                entry["name"].as_str().unwrap_or_default().to_string(),
                entry["msg"].as_str().unwrap_or_default().to_string(),
            )
        })
    })
}

pub struct Decoded {
    pub name: String,
    pub args: Vec<(String, String)>,
    pub accounts: Vec<String>,
}

pub fn instruction(idl: &Value, data: &[u8]) -> Option<Decoded> {
    let list = idl["instructions"].as_array()?;
    let entry = list.iter().find(|entry| {
        let name = entry["name"].as_str().unwrap_or_default();
        discriminator(entry, name)
            .map(|disc| data.starts_with(&disc))
            .unwrap_or(false)
    })?;
    let name = entry["name"].as_str().unwrap_or_default().to_string();
    let mut cursor = 8usize;
    let mut args = Vec::new();
    for arg in entry["args"].as_array().into_iter().flatten() {
        let label = arg["name"].as_str().unwrap_or_default().to_string();
        match read(&arg["type"], data, &mut cursor) {
            Some(value) => args.push((label, value)),
            None => {
                args.push((label, "…".to_string()));
                break;
            }
        }
    }
    Some(Decoded {
        name,
        args,
        accounts: entry["accounts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| a["name"].as_str().unwrap_or_default().to_string())
            .collect(),
    })
}

pub fn account(idl: &Value, data: &[u8]) -> Option<(String, Vec<(String, String)>)> {
    let entry = idl["accounts"].as_array()?.iter().find(|entry| {
        let name = entry["name"].as_str().unwrap_or_default();
        account_discriminator(entry, name)
            .map(|disc| data.starts_with(&disc))
            .unwrap_or(false)
    })?;
    let name = entry["name"].as_str().unwrap_or_default().to_string();
    let fields = type_fields(idl, &name)?;
    let mut cursor = 8usize;
    let mut decoded = Vec::new();
    for field in fields {
        let label = field["name"].as_str().unwrap_or_default().to_string();
        match read(&field["type"], data, &mut cursor) {
            Some(value) => decoded.push((label, value)),
            None => {
                decoded.push((label, "…".to_string()));
                break;
            }
        }
    }
    Some((name, decoded))
}

fn type_fields<'a>(idl: &'a Value, name: &str) -> Option<Vec<&'a Value>> {
    let types = idl["types"].as_array()?;
    let entry = types.iter().find(|t| t["name"].as_str() == Some(name))?;
    Some(entry.pointer("/type/fields")?.as_array()?.iter().collect())
}

fn discriminator(entry: &Value, name: &str) -> Option<Vec<u8>> {
    if let Some(list) = entry["discriminator"].as_array() {
        return Some(list.iter().filter_map(Value::as_u64).map(|b| b as u8).collect());
    }
    Some(sha256(&[format!("global:{}", snake_case(name)).as_bytes()])[..8].to_vec())
}

fn account_discriminator(entry: &Value, name: &str) -> Option<Vec<u8>> {
    if let Some(list) = entry["discriminator"].as_array() {
        return Some(list.iter().filter_map(Value::as_u64).map(|b| b as u8).collect());
    }
    Some(sha256(&[format!("account:{name}").as_bytes()])[..8].to_vec())
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn take<'a>(data: &'a [u8], cursor: &mut usize, len: usize) -> Option<&'a [u8]> {
    let slice = data.get(*cursor..*cursor + len)?;
    *cursor += len;
    Some(slice)
}

fn read(kind: &Value, data: &[u8], cursor: &mut usize) -> Option<String> {
    if let Some(name) = kind.as_str() {
        return Some(match name {
            "bool" => (take(data, cursor, 1)?[0] == 1).to_string(),
            "u8" => take(data, cursor, 1)?[0].to_string(),
            "i8" => (take(data, cursor, 1)?[0] as i8).to_string(),
            "u16" => u16::from_le_bytes(take(data, cursor, 2)?.try_into().ok()?).to_string(),
            "i16" => i16::from_le_bytes(take(data, cursor, 2)?.try_into().ok()?).to_string(),
            "u32" => u32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?).to_string(),
            "i32" => i32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?).to_string(),
            "u64" => u64::from_le_bytes(take(data, cursor, 8)?.try_into().ok()?).to_string(),
            "i64" => i64::from_le_bytes(take(data, cursor, 8)?.try_into().ok()?).to_string(),
            "u128" => u128::from_le_bytes(take(data, cursor, 16)?.try_into().ok()?).to_string(),
            "i128" => i128::from_le_bytes(take(data, cursor, 16)?.try_into().ok()?).to_string(),
            "pubkey" | "publicKey" => bs58_encode(take(data, cursor, 32)?),
            "string" => {
                let len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?) as usize;
                String::from_utf8_lossy(take(data, cursor, len)?).to_string()
            }
            "bytes" => {
                let len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?) as usize;
                hex(take(data, cursor, len)?)
            }
            _ => return None,
        });
    }
    if let Some(inner) = kind.get("option") {
        return match take(data, cursor, 1)?[0] {
            0 => Some("none".to_string()),
            _ => read(inner, data, cursor),
        };
    }
    if let Some(inner) = kind.get("vec") {
        let len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?) as usize;
        let items: Vec<String> = (0..len).filter_map(|_| read(inner, data, cursor)).collect();
        return Some(format!("[{}]", items.join(", ")));
    }
    if let Some(array) = kind.get("array").and_then(Value::as_array) {
        let len = array.get(1)?.as_u64()? as usize;
        let items: Vec<String> = (0..len).filter_map(|_| read(array.first()?, data, cursor)).collect();
        return Some(format!("[{}]", items.join(", ")));
    }
    None
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn enrich(tx: &mut solana::Tx) {
    let cluster = tx.cluster.clone();
    let Some(failure) = tx.failure.as_mut() else { return };
    let crate::diagnose::Code::Custom(code) = failure.code else { return };
    let Some(node) = failure.node else { return };
    let Some(idl) = idl(&cluster, &tx.nodes[node].program) else { return };
    if let Some((name, message)) = error(&idl, code) {
        failure.headline = format!("{name} — error {code} (0x{code:x})");
        if !message.is_empty() {
            failure.meaning = message;
        }
        failure.checks.retain(|check| !check.contains("error enum"));
    }
    let _ = json!({});
}
