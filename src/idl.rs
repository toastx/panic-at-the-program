use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::io::Read;
use std::path::PathBuf;

use curve25519_dalek::edwards::CompressedEdwardsY;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::solana::{self, bs58_decode, bs58_encode};

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<Rc<Program>>>> = RefCell::new(HashMap::new());
}

pub fn idl(cluster: &str, program: &str) -> Option<Rc<Program>> {
    let key = format!("{cluster}/{program}");
    if let Some(hit) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return hit;
    }
    let found = local(program)
        .or_else(|| cached(cluster, program))
        .or_else(|| fetch(cluster, program))
        .map(|value| Rc::new(parse(&value)))
        .filter(|parsed| parsed.address.as_deref().is_none_or(|address| address == program));
    if let Some(found) = &found {
        crate::workspace::record_flavor(program, found.flavor.label());
    }
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
    solana::load(cluster, &cache_key(program)).filter(|value| !value.is_null())
}

fn fetch(cluster: &str, program: &str) -> Option<Value> {
    let addresses = candidates(program);
    let accounts = solana::accounts_info_sparse(cluster, &addresses).ok()?;
    let found = accounts
        .iter()
        .flatten()
        .find(|account| account["owner"].as_str() == Some(program))
        .and_then(|account| account.pointer("/data/0").and_then(Value::as_str))
        .and_then(inflate_account);
    let _ = solana::store(cluster, &cache_key(program), found.as_ref().unwrap_or(&Value::Null));
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

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Flavor {
    Anchor,
    AnchorLegacy,
    Shank,
    Codama,
}

impl Flavor {
    pub fn label(&self) -> &'static str {
        match self {
            Flavor::Anchor => "anchor",
            Flavor::AnchorLegacy => "anchor-legacy",
            Flavor::Shank => "shank",
            Flavor::Codama => "codama",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    U8,
    U16,
    U32,
    U64,
    U128,
    I8,
    I16,
    I32,
    I64,
    I128,
    Bool,
    Pubkey,
    Str,
    Bytes,
    List(Box<Type>),
    Opt(Box<Type>),
    Array(Box<Type>, usize),
    Defined(String),
    Unknown,
}

#[derive(Clone, Debug)]
pub enum Seed {
    Const(Vec<u8>),
    Account(String),
    Arg(String),
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct AccountSpec {
    pub name: String,
    pub signer: bool,
    pub writable: bool,
    pub address: Option<String>,
    pub seeds: Option<Vec<Seed>>,
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub name: String,
    pub discriminator: Vec<u8>,
    pub accounts: Vec<AccountSpec>,
    pub args: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct AccountType {
    pub name: String,
    pub discriminator: Vec<u8>,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct ErrorDef {
    pub code: u32,
    pub name: String,
    pub message: String,
}

pub struct Program {
    pub flavor: Flavor,
    pub address: Option<String>,
    pub instructions: Vec<Instruction>,
    pub accounts: Vec<AccountType>,
    pub errors: Vec<ErrorDef>,
}

pub fn detect(value: &Value) -> Flavor {
    if value["kind"].as_str() == Some("rootNode") || value["standard"].as_str() == Some("codama") {
        return Flavor::Codama;
    }
    if value.pointer("/metadata/origin").and_then(Value::as_str) == Some("shank")
        || value["instructions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|ix| ix.get("discriminant").is_some())
    {
        return Flavor::Shank;
    }
    if value["address"].is_string() || value.pointer("/metadata/spec").is_some() {
        return Flavor::Anchor;
    }
    Flavor::AnchorLegacy
}

pub fn parse(value: &Value) -> Program {
    match detect(value) {
        Flavor::Codama => parse_codama(value),
        flavor => parse_anchor_like(value, flavor),
    }
}

fn bytes_field(entry: &Value, key: &str) -> Option<Vec<u8>> {
    Some(
        entry[key]
            .as_array()?
            .iter()
            .filter_map(Value::as_u64)
            .map(|b| b as u8)
            .collect(),
    )
}

fn parse_anchor_like(value: &Value, flavor: Flavor) -> Program {
    let instructions = value["instructions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let name = entry["name"].as_str().unwrap_or_default().to_string();
            let discriminator = bytes_field(entry, "discriminator")
                .or_else(|| {
                    let value = entry.pointer("/discriminant/value")?.as_u64()?;
                    Some(vec![value as u8])
                })
                .unwrap_or_else(|| sighash("global", &name));
            Instruction {
                accounts: entry["accounts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(parse_anchor_account_spec)
                    .collect(),
                args: entry["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|arg| Field {
                        name: arg["name"].as_str().unwrap_or_default().to_string(),
                        ty: anchor_type(&arg["type"]),
                    })
                    .collect(),
                name,
                discriminator,
            }
        })
        .collect();

    let types = value["types"].as_array().cloned().unwrap_or_default();
    let fields_for = |entry: &Value, name: &str| -> Vec<Field> {
        let inline = entry.pointer("/type/fields").and_then(Value::as_array).cloned();
        let defined = types
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .and_then(|t| t.pointer("/type/fields").and_then(Value::as_array).cloned());
        inline
            .or(defined)
            .unwrap_or_default()
            .iter()
            .map(|field| Field {
                name: field["name"].as_str().unwrap_or_default().to_string(),
                ty: anchor_type(&field["type"]),
            })
            .collect()
    };

    let accounts = value["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let name = entry["name"].as_str().unwrap_or_default().to_string();
            AccountType {
                discriminator: bytes_field(entry, "discriminator")
                    .unwrap_or_else(|| sighash("account", &name)),
                fields: fields_for(entry, &name),
                name,
            }
        })
        .collect();

    Program {
        flavor,
        address: address(value),
        instructions,
        accounts,
        errors: parse_errors(value["errors"].as_array(), "msg"),
    }
}

fn parse_anchor_account_spec(entry: &Value) -> AccountSpec {
    let flag = |new: &str, old: &str| {
        entry[new]
            .as_bool()
            .or_else(|| entry[old].as_bool())
            .unwrap_or(false)
    };
    AccountSpec {
        name: entry["name"].as_str().unwrap_or_default().to_string(),
        signer: flag("signer", "isSigner"),
        writable: flag("writable", "isMut"),
        address: entry["address"].as_str().map(str::to_string),
        seeds: entry.pointer("/pda/seeds").and_then(Value::as_array).map(|seeds| {
            seeds
                .iter()
                .filter_map(|seed| match seed["kind"].as_str()? {
                    "const" => bytes_field(seed, "value").map(Seed::Const),
                    "account" => Some(Seed::Account(seed["path"].as_str()?.to_string())),
                    "arg" => Some(Seed::Arg(seed["path"].as_str()?.to_string())),
                    _ => None,
                })
                .collect()
        }),
    }
}

fn parse_errors(list: Option<&Vec<Value>>, message_key: &str) -> Vec<ErrorDef> {
    list.into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(ErrorDef {
                code: entry["code"].as_u64()? as u32,
                name: entry["name"].as_str().unwrap_or_default().to_string(),
                message: entry[message_key].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

fn number_type(name: &str) -> Option<Type> {
    Some(match name {
        "u8" => Type::U8,
        "u16" => Type::U16,
        "u32" => Type::U32,
        "u64" => Type::U64,
        "u128" => Type::U128,
        "i8" => Type::I8,
        "i16" => Type::I16,
        "i32" => Type::I32,
        "i64" => Type::I64,
        "i128" => Type::I128,
        _ => return None,
    })
}

fn anchor_type(value: &Value) -> Type {
    if let Some(name) = value.as_str() {
        if let Some(number) = number_type(name) {
            return number;
        }
        return match name {
            "bool" => Type::Bool,
            "pubkey" | "publicKey" => Type::Pubkey,
            "string" => Type::Str,
            "bytes" => Type::Bytes,
            other => Type::Defined(other.to_string()),
        };
    }
    if let Some(inner) = value.get("vec") {
        return Type::List(Box::new(anchor_type(inner)));
    }
    if let Some(inner) = value.get("option") {
        return Type::Opt(Box::new(anchor_type(inner)));
    }
    if let Some(array) = value.get("array").and_then(Value::as_array) {
        let len = array.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
        return Type::Array(Box::new(anchor_type(&array[0])), len);
    }
    if let Some(defined) = value.get("defined") {
        let name = defined
            .as_str()
            .or_else(|| defined["name"].as_str())
            .unwrap_or_default();
        return Type::Defined(name.to_string());
    }
    Type::Unknown
}

fn parse_codama(value: &Value) -> Program {
    let program = &value["program"];
    let instructions = program["instructions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let args: Vec<Value> = entry["arguments"].as_array().cloned().unwrap_or_default();
            let discriminator = codama_discriminator(entry, &args);
            Instruction {
                name: entry["name"].as_str().unwrap_or_default().to_string(),
                discriminator,
                accounts: entry["accounts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|account| AccountSpec {
                        name: account["name"].as_str().unwrap_or_default().to_string(),
                        signer: !matches!(account["isSigner"].as_bool(), Some(false) | None),
                        writable: account["isWritable"].as_bool().unwrap_or(false),
                        address: account.pointer("/defaultValue/publicKey").and_then(Value::as_str).map(str::to_string),
                        seeds: codama_seeds(account),
                    })
                    .collect(),
                args: args
                    .iter()
                    .filter(|arg| arg["defaultValueStrategy"].as_str() != Some("omitted"))
                    .map(|arg| Field {
                        name: arg["name"].as_str().unwrap_or_default().to_string(),
                        ty: codama_type(&arg["type"]),
                    })
                    .collect(),
            }
        })
        .collect();

    let accounts = program["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let fields: Vec<Value> = entry
                .pointer("/data/fields")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            AccountType {
                name: entry["name"].as_str().unwrap_or_default().to_string(),
                discriminator: codama_discriminator(entry, &fields),
                fields: fields
                    .iter()
                    .filter(|field| field["defaultValueStrategy"].as_str() != Some("omitted"))
                    .map(|field| Field {
                        name: field["name"].as_str().unwrap_or_default().to_string(),
                        ty: codama_type(&field["type"]),
                    })
                    .collect(),
            }
        })
        .collect();

    Program {
        flavor: Flavor::Codama,
        address: program["publicKey"].as_str().map(str::to_string),
        instructions,
        accounts,
        errors: parse_errors(program["errors"].as_array(), "message"),
    }
}

fn codama_value_bytes(value: &Value, ty: &Type) -> Option<Vec<u8>> {
    match value["kind"].as_str()? {
        "numberValueNode" => {
            let number = value["number"].as_u64()?;
            Some(match ty {
                Type::U16 => (number as u16).to_le_bytes().to_vec(),
                Type::U32 => (number as u32).to_le_bytes().to_vec(),
                Type::U64 => number.to_le_bytes().to_vec(),
                _ => vec![number as u8],
            })
        }
        "bytesValueNode" => {
            let data = value["data"].as_str()?;
            match value["encoding"].as_str() {
                Some("base16") => (0..data.len() / 2)
                    .map(|i| u8::from_str_radix(&data[i * 2..i * 2 + 2], 16).ok())
                    .collect(),
                Some("base64") => Some(crate::solana::base64_decode(data)),
                _ => Some(crate::solana::bs58_decode(data)),
            }
        }
        "constantValueNode" => codama_value_bytes(&value["value"], ty),
        _ => None,
    }
}

fn codama_discriminator(entry: &Value, args: &[Value]) -> Vec<u8> {
    for discriminator in entry["discriminators"].as_array().into_iter().flatten() {
        match discriminator["kind"].as_str() {
            Some("fieldDiscriminatorNode") => {
                let name = discriminator["name"].as_str().unwrap_or_default();
                if let Some(arg) = args.iter().find(|arg| arg["name"].as_str() == Some(name)) {
                    if let Some(bytes) = codama_value_bytes(&arg["defaultValue"], &codama_type(&arg["type"])) {
                        return bytes;
                    }
                }
            }
            Some("constantDiscriminatorNode") => {
                if let Some(bytes) = codama_value_bytes(&discriminator["constant"], &Type::Bytes) {
                    return bytes;
                }
            }
            _ => {}
        }
    }
    Vec::new()
}

fn codama_seeds(account: &Value) -> Option<Vec<Seed>> {
    let seeds = account.pointer("/defaultValue/seeds")?.as_array()?;
    Some(
        seeds
            .iter()
            .filter_map(|seed| match seed["kind"].as_str()? {
                "constantPdaSeedNode" => Some(Seed::Const(codama_value_bytes(&seed["value"], &Type::Bytes)?)),
                "variablePdaSeedNode" => Some(Seed::Account(seed["name"].as_str()?.to_string())),
                _ => None,
            })
            .collect(),
    )
}

fn codama_type(value: &Value) -> Type {
    match value["kind"].as_str().unwrap_or_default() {
        "numberTypeNode" => number_type(value["format"].as_str().unwrap_or_default()).unwrap_or(Type::Unknown),
        "booleanTypeNode" => Type::Bool,
        "publicKeyTypeNode" => Type::Pubkey,
        "stringTypeNode" => Type::Str,
        "bytesTypeNode" => Type::Bytes,
        "optionTypeNode" | "zeroableOptionTypeNode" => Type::Opt(Box::new(codama_type(&value["item"]))),
        "arrayTypeNode" => {
            let item = Box::new(codama_type(&value["item"]));
            match value.pointer("/count/value").and_then(Value::as_u64) {
                Some(len) => Type::Array(item, len as usize),
                None => Type::List(item),
            }
        }
        "fixedSizeTypeNode" | "sizePrefixTypeNode" => codama_type(&value["type"]),
        "definedTypeLinkNode" => Type::Defined(value["name"].as_str().unwrap_or_default().to_string()),
        _ => Type::Unknown,
    }
}


#[cfg(test)]
pub fn account_specs(value: &Value) -> Vec<AccountSpec> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(parse_anchor_account_spec)
        .collect()
}

pub fn error(program: &Program, code: u32) -> Option<(String, String)> {
    program
        .errors
        .iter()
        .find(|entry| entry.code == code)
        .map(|entry| (entry.name.clone(), entry.message.clone()))
}

pub struct Decoded {
    pub name: String,
    pub args: Vec<(String, String)>,
    pub specs: Vec<AccountSpec>,
}

pub fn instruction(program: &Program, data: &[u8]) -> Option<Decoded> {
    let entry = program
        .instructions
        .iter()
        .filter(|entry| !entry.discriminator.is_empty())
        .find(|entry| data.starts_with(&entry.discriminator))?;
    let mut cursor = entry.discriminator.len();
    Some(Decoded {
        name: entry.name.clone(),
        args: decode_fields(&entry.args, data, &mut cursor),
        specs: entry.accounts.clone(),
    })
}

pub fn account(program: &Program, data: &[u8]) -> Option<(String, Vec<(String, String)>)> {
    let entry = program
        .accounts
        .iter()
        .filter(|entry| !entry.discriminator.is_empty())
        .find(|entry| data.starts_with(&entry.discriminator))?;
    let mut cursor = entry.discriminator.len();
    Some((entry.name.clone(), decode_fields(&entry.fields, data, &mut cursor)))
}

fn decode_fields(fields: &[Field], data: &[u8], cursor: &mut usize) -> Vec<(String, String)> {
    let mut decoded = Vec::new();
    for field in fields {
        match read(&field.ty, data, cursor) {
            Some(value) => decoded.push((field.name.clone(), value)),
            None => {
                decoded.push((field.name.clone(), "…".to_string()));
                break;
            }
        }
    }
    decoded
}

fn sighash(namespace: &str, name: &str) -> Vec<u8> {
    let subject = if namespace == "global" { snake_case(name) } else { name.to_string() };
    sha256(&[format!("{namespace}:{subject}").as_bytes()])[..8].to_vec()
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

fn read(kind: &Type, data: &[u8], cursor: &mut usize) -> Option<String> {
    Some(match kind {
        Type::Bool => (take(data, cursor, 1)?[0] == 1).to_string(),
        Type::U8 => take(data, cursor, 1)?[0].to_string(),
        Type::I8 => (take(data, cursor, 1)?[0] as i8).to_string(),
        Type::U16 => u16::from_le_bytes(take(data, cursor, 2)?.try_into().ok()?).to_string(),
        Type::I16 => i16::from_le_bytes(take(data, cursor, 2)?.try_into().ok()?).to_string(),
        Type::U32 => u32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?).to_string(),
        Type::I32 => i32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?).to_string(),
        Type::U64 => u64::from_le_bytes(take(data, cursor, 8)?.try_into().ok()?).to_string(),
        Type::I64 => i64::from_le_bytes(take(data, cursor, 8)?.try_into().ok()?).to_string(),
        Type::U128 => u128::from_le_bytes(take(data, cursor, 16)?.try_into().ok()?).to_string(),
        Type::I128 => i128::from_le_bytes(take(data, cursor, 16)?.try_into().ok()?).to_string(),
        Type::Pubkey => bs58_encode(take(data, cursor, 32)?),
        Type::Str => {
            let len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?) as usize;
            String::from_utf8_lossy(take(data, cursor, len)?).to_string()
        }
        Type::Bytes => {
            let len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?) as usize;
            hex(take(data, cursor, len)?)
        }
        Type::Opt(inner) => match take(data, cursor, 1)?[0] {
            0 => "none".to_string(),
            _ => read(inner, data, cursor)?,
        },
        Type::List(inner) => {
            let len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().ok()?) as usize;
            let items: Vec<String> = (0..len).filter_map(|_| read(inner, data, cursor)).collect();
            format!("[{}]", items.join(", "))
        }
        Type::Array(inner, len) => {
            let items: Vec<String> = (0..*len).filter_map(|_| read(inner, data, cursor)).collect();
            format!("[{}]", items.join(", "))
        }
        Type::Defined(_) | Type::Unknown => return None,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn enrich(tx: &mut solana::Tx) {
    let cluster = tx.cluster.clone();
    let (cu, limit) = (tx.cu, tx.cu_limit);
    let Some(node) = tx.failure.as_ref().and_then(|failure| failure.node) else { return };
    let program = tx.nodes[node].program.clone();
    let logs = tx.logs[tx.nodes[node].logs.clone()].to_vec();
    let parsed = idl(&cluster, &program);
    let Some(failure) = tx.failure.as_mut() else { return };
    let crate::diagnose::Code::Custom(code) = failure.code else { return };

    let named = crate::workspace::error_label(&program, code)
        .map(|(name, message)| (name, message, "patp.toml".to_string()))
        .or_else(|| {
            let parsed = parsed.as_ref()?;
            let (name, message) = error(parsed, code)?;
            Some((name, message, parsed.flavor.label().to_string()))
        });

    match named {
        Some((name, message, source)) => {
            failure.headline = format!("{name} — error {code} (0x{code:x})");
            if !message.is_empty() {
                failure.meaning = message;
            }
            failure.checks.retain(|check| !check.contains("error enum"));
            if source == "patp.toml" {
                failure.checks.push("named by your patp.toml error table".to_string());
            }
        }
        None => {
            let flavor = parsed.as_ref().map(|parsed| parsed.flavor);
            let (headline, meaning, hint) = crate::diagnose::infer_custom(code, flavor);
            failure.headline = headline;
            failure.meaning = meaning;
            failure.checks.retain(|check| !check.contains("error enum"));
            failure.checks.push(hint);
            failure.checks.push(format!(
                "name it in patp.toml: [errors.{program}] {code} = \"Name: what it means\""
            ));
            crate::workspace::record_error_stub(&program, code);
        }
    }

    for check in crate::diagnose::rules(&failure.code, &failure.headline, &logs, cu, limit) {
        if !failure.checks.contains(&check) {
            failure.checks.push(check);
        }
    }
}
