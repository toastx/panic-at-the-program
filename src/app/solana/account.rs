//! A single fetched account.

use super::*;
use serde_json::Value;

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
