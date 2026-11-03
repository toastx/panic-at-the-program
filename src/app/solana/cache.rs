//! What has already been fetched: transactions and accounts on disk, and the
//! account state and rent minimums validation reads, in memory.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde_json::Value;

use super::rpc::{accounts_info_sparse, minimum_balance};
use super::{Status, base64_decode, parse, parse_account};

fn cache_dir() -> PathBuf {
    crate::workspace::home().join(".cache/panic")
}

fn cache_path(cluster: &str, key: &str) -> PathBuf {
    cache_dir().join(cluster).join(format!("{key}.json"))
}

pub fn cached(cluster: &str) -> Vec<String> {
    let mut entries: Vec<(std::time::SystemTime, String)> = fs::read_dir(cache_dir().join(cluster))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let key = name.strip_suffix(".json")?.to_string();
            Some((e.metadata().ok()?.modified().ok()?, key))
        })
        .collect();
    entries.sort_by_key(|a| std::cmp::Reverse(a.0));
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
    serde_json::from_reader(fs::File::open(cache_path(cluster, key)).ok()?).ok()
}

pub fn store(cluster: &str, key: &str, value: &Value) -> io::Result<()> {
    let path = cache_path(cluster, key);
    fs::create_dir_all(path.parent().unwrap_or(&cache_dir()))?;
    serde_json::to_writer(fs::File::create(path)?, value).map_err(io::Error::other)
}

pub fn evict(cluster: &str, key: &str) {
    let _ = fs::remove_file(cache_path(cluster, key));
}

/// The slice of an account that validation reads.
#[derive(Clone)]
pub struct AccountState {
    pub owner: String,
    pub lamports: u64,
    pub data: Vec<u8>,
}

thread_local! {
    static STATES: RefCell<HashMap<String, Option<AccountState>>> = RefCell::new(HashMap::new());
    static RENT: RefCell<HashMap<usize, u64>> = RefCell::new(HashMap::new());
}

/// The owner, balance and data of each key, fetched once per cluster and cached.
/// Keys with no account on chain are left out.
pub fn account_states(cluster: &str, keys: &[String]) -> HashMap<String, AccountState> {
    let missing: Vec<String> = keys
        .iter()
        .filter(|key| STATES.with(|c| !c.borrow().contains_key(&format!("{cluster}/{key}"))))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let fetched = accounts_info_sparse(cluster, &missing).unwrap_or_default();
        STATES.with(|cache| {
            let mut cache = cache.borrow_mut();
            for (key, account) in missing.iter().zip(fetched) {
                let state = account.as_ref().map(|account| AccountState {
                    owner: account["owner"].as_str().unwrap_or_default().to_string(),
                    lamports: account["lamports"].as_u64().unwrap_or(0),
                    data: account
                        .pointer("/data/0")
                        .and_then(Value::as_str)
                        .map(base64_decode)
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

/// Cached per size: the rent rate is the same on every cluster.
pub fn rent_exempt_minimum(cluster: &str, size: usize) -> Option<u64> {
    if let Some(hit) = RENT.with(|cache| cache.borrow().get(&size).copied()) {
        return Some(hit);
    }
    let value = minimum_balance(cluster, size).ok()?;
    RENT.with(|cache| cache.borrow_mut().insert(size, value));
    Some(value)
}
