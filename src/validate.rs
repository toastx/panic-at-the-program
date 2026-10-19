use std::cell::RefCell;
use std::collections::HashMap;

use serde_json::Value;

use crate::idl;
use crate::solana::{self, Node, Tx, bs58_decode, bs58_encode};

pub const TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const ATA: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
pub const SYSTEM: &str = "11111111111111111111111111111111";

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Level {
    Error,
    Warn,
}

pub struct Finding {
    pub index: usize,
    pub role: String,
    pub key: String,
    pub level: Level,
    pub message: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

#[derive(Clone)]
pub struct State {
    pub owner: String,
    pub lamports: u64,
    pub data: Vec<u8>,
}

thread_local! {
    static STATES: RefCell<HashMap<String, Option<State>>> = RefCell::new(HashMap::new());
    static RENT: RefCell<HashMap<usize, u64>> = RefCell::new(HashMap::new());
}

fn states(cluster: &str, keys: &[String]) -> HashMap<String, State> {
    let missing: Vec<String> = keys
        .iter()
        .filter(|key| STATES.with(|c| !c.borrow().contains_key(&format!("{cluster}/{key}"))))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let fetched = solana::accounts_info_sparse(cluster, &missing).unwrap_or_default();
        STATES.with(|cache| {
            let mut cache = cache.borrow_mut();
            for (key, account) in missing.iter().zip(fetched) {
                let state = account.as_ref().map(|account| State {
                    owner: account["owner"].as_str().unwrap_or_default().to_string(),
                    lamports: account["lamports"].as_u64().unwrap_or(0),
                    data: account
                        .pointer("/data/0")
                        .and_then(Value::as_str)
                        .map(solana::base64_decode)
                        .unwrap_or_default(),
                });
                cache.insert(format!("{cluster}/{key}"), state);
            }
        });
    }
    keys.iter()
        .filter_map(|key| {
            let state = STATES.with(|c| c.borrow().get(&format!("{cluster}/{key}")).cloned())??;
            Some((key.clone(), state))
        })
        .collect()
}

fn rent_exempt_minimum(cluster: &str, size: usize) -> Option<u64> {
    if let Some(hit) = RENT.with(|cache| cache.borrow().get(&size).copied()) {
        return Some(hit);
    }
    let value = solana::minimum_balance(cluster, size).ok()?;
    RENT.with(|cache| cache.borrow_mut().insert(size, value));
    Some(value)
}

pub fn check(cluster: &str, tx: &Tx, index: usize) -> Vec<Finding> {
    let Some(node) = tx.nodes.get(index) else { return Vec::new() };
    let decoded = idl::idl(cluster, &node.program)
        .and_then(|idl| idl::instruction(&idl, &bs58_decode(&node.data)).map(|d| (idl, d)));

    let mut findings = Vec::new();
    let mut roles = Vec::new();
    if let Some((idl, decoded)) = &decoded {
        let specs = idl["instructions"]
            .as_array()
            .and_then(|list| list.iter().find(|entry| entry["name"].as_str() == Some(decoded.name.as_str())))
            .and_then(|entry| entry["accounts"].as_array().cloned())
            .unwrap_or_default();
        roles = decoded.accounts.clone();
        findings.extend(declared_findings(&specs, node));
    }

    let keys: Vec<String> = node.accounts.iter().map(|a| a.key.clone()).collect();
    let state = states(cluster, &keys);
    let rent = |size: usize| rent_exempt_minimum(cluster, size);
    findings.extend(state_findings(node, &roles, &state, rent));
    findings
}

pub fn declared_findings(specs: &[Value], node: &Node) -> Vec<Finding> {
    if specs.len() != node.accounts.len() {
        return Vec::new();
    }
    let mut findings = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let role = spec["name"].as_str().unwrap_or_default().to_string();
        let Some(actual) = node.accounts.get(i) else {
            findings.push(Finding {
                index: i,
                role,
                key: String::new(),
                level: Level::Error,
                message: "account missing from the instruction".to_string(),
                expected: None,
                actual: None,
            });
            continue;
        };
        let finding = |level, message: String, expected, actual_value| Finding {
            index: i,
            role: role.clone(),
            key: actual.key.clone(),
            level,
            message,
            expected,
            actual: actual_value,
        };

        if spec["signer"].as_bool() == Some(true) && !actual.signer {
            findings.push(finding(Level::Error, "declared as signer but did not sign".into(), None, None));
        }
        if spec["writable"].as_bool() == Some(true) && !actual.writable {
            findings.push(finding(Level::Error, "declared writable but passed read-only".into(), None, None));
        }
        if let Some(fixed) = spec["address"].as_str() {
            if fixed != actual.key {
                findings.push(finding(
                    Level::Error,
                    "address does not match the one the program requires".into(),
                    Some(fixed.to_string()),
                    Some(actual.key.clone()),
                ));
            }
        }
        if let Some(expected) = derive_pda(spec, &node.program, specs, node) {
            if expected != actual.key {
                findings.push(finding(
                    Level::Error,
                    "PDA does not match the seeds declared in the IDL".into(),
                    Some(expected),
                    Some(actual.key.clone()),
                ));
            }
        }
    }
    findings
}

pub fn state_findings(
    node: &Node,
    roles: &[String],
    state: &HashMap<String, State>,
    rent: impl Fn(usize) -> Option<u64>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (i, account) in node.accounts.iter().enumerate() {
        let Some(info) = state.get(&account.key) else { continue };
        let role = roles.get(i).cloned().unwrap_or_default();
        let finding = |level, message: String, expected, actual| Finding {
            index: i,
            role: role.clone(),
            key: account.key.clone(),
            level,
            message,
            expected,
            actual,
        };

        if info.owner == TOKEN && info.data.len() == 165 {
            let mint = bs58_encode(&info.data[0..32]);
            let owner = bs58_encode(&info.data[32..64]);

            if role.contains("associated") {
                let ata = find_ata(&owner, &mint);
                if ata.as_deref() != Some(account.key.as_str()) {
                    findings.push(finding(
                        Level::Error,
                        "not the associated token account for this owner and mint".to_string(),
                        ata,
                        Some(account.key.clone()),
                    ));
                }
            }
        }

        if account.writable && info.owner != SYSTEM {
            if let Some(minimum) = rent(info.data.len()) {
                if info.lamports < minimum {
                    findings.push(finding(
                        Level::Warn,
                        "below the rent-exempt minimum".to_string(),
                        Some(format!("{:.9} SOL", minimum as f64 / 1e9)),
                        Some(format!("{:.9} SOL", info.lamports as f64 / 1e9)),
                    ));
                }
            }
        }
    }
    findings
}

fn find_ata(owner: &str, mint: &str) -> Option<String> {
    let (address, _) = idl::find_program_address(
        &[&bs58_decode(owner), &bs58_decode(TOKEN), &bs58_decode(mint)],
        &bs58_decode(ATA),
    )?;
    Some(bs58_encode(&address))
}

fn derive_pda(spec: &Value, program: &str, specs: &[Value], node: &Node) -> Option<String> {
    let seeds = spec.pointer("/pda/seeds")?.as_array()?;
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for seed in seeds {
        match seed["kind"].as_str()? {
            "const" => parts.push(
                seed["value"]
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|b| b as u8)
                    .collect(),
            ),
            "account" => {
                let path = seed["path"].as_str()?;
                let position = specs.iter().position(|s| s["name"].as_str() == Some(path))?;
                parts.push(bs58_decode(&node.accounts.get(position)?.key));
            }
            _ => return None,
        }
    }
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    let (address, _) = idl::find_program_address(&refs, &bs58_decode(program))?;
    Some(bs58_encode(&address))
}
