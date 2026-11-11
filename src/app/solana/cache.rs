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
    /// Which `panic test` run stored it: 1, 2, 3, ... None for anything else.
    pub run: Option<u64>,
}

/// Every `panic test` run in one file: `{"1": {"<test>": <transaction>}, "2": ...}`.
const RUNS: &str = "runs";

fn runs(cluster: &str) -> serde_json::Map<String, Value> {
    match load_file(cluster, RUNS) {
        Some(Value::Object(runs)) => runs,
        _ => serde_json::Map::new(),
    }
}

/// Adds a run of failed transactions, by test name, under the next run number,
/// which it returns. Each is then loaded by the key `<run>/<test>`.
pub fn store_run(cluster: &str, cases: serde_json::Map<String, Value>) -> io::Result<u64> {
    let mut all = runs(cluster);
    let run = all.keys().filter_map(|run| run.parse::<u64>().ok()).max().unwrap_or(0) + 1;
    all.insert(run.to_string(), Value::Object(cases));
    store(cluster, RUNS, &Value::Object(all))?;
    Ok(run)
}

/// Test runs first, newest on top, then everything else cached, newest first.
pub fn entries(cluster: &str) -> Vec<Entry> {
    let tx_entry = |key: String, value: Option<Value>, run: Option<u64>| {
        let tx = value.map(|value| parse(&key, cluster, &value));
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
            run,
        }
    };

    let mut runs: Vec<(u64, Value)> = runs(cluster)
        .into_iter()
        .filter_map(|(run, cases)| Some((run.parse().ok()?, cases)))
        .collect();
    runs.sort_by_key(|(run, _)| std::cmp::Reverse(*run));
    let mut entries: Vec<Entry> = runs
        .into_iter()
        .flat_map(|(run, cases)| match cases {
            Value::Object(cases) => cases
                .into_iter()
                .map(|(name, value)| tx_entry(format!("{run}/{name}"), Some(value), Some(run)))
                .collect(),
            _ => Vec::new(),
        })
        .collect();

    entries.extend(
        cached(cluster)
            .into_iter()
            .filter(|key| !key.starts_with("idl-") && key != RUNS)
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
                        run: None,
                    }
                }
                None => {
                    let value = load(cluster, &key);
                    tx_entry(key, value, None)
                }
            }),
    );
    entries
}

/// A cached response. `<run>/<test>` keys are read out of the runs file.
pub fn load(cluster: &str, key: &str) -> Option<Value> {
    match key.split_once('/') {
        Some((run, test)) => runs(cluster).get(run)?.get(test).cloned(),
        None => load_file(cluster, key),
    }
}

fn load_file(cluster: &str, key: &str) -> Option<Value> {
    serde_json::from_reader(fs::File::open(cache_path(cluster, key)).ok()?).ok()
}

pub fn store(cluster: &str, key: &str, value: &Value) -> io::Result<()> {
    let path = cache_path(cluster, key);
    fs::create_dir_all(path.parent().unwrap_or(&cache_dir()))?;
    serde_json::to_writer(fs::File::create(path)?, value).map_err(io::Error::other)
}

/// Test runs are never evicted: there is no chain to fetch them from again.
pub fn evict(cluster: &str, key: &str) {
    if !key.contains('/') {
        let _ = fs::remove_file(cache_path(cluster, key));
    }
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
