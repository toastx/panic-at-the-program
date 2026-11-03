//! Byte-level decoding: base58, base64, wire messages, and the native
//! programs' instructions and accounts.

use super::*;
use serde_json::{Value, json};

pub fn base64_decode(input: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes().filter(|b| *b != b'=') {
        let Some(value) = TABLE.iter().position(|t| *t == byte) else {
            continue;
        };
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

/// `transaction.message` recovered from a serialized transaction, in the shape
/// `parse` reads.
pub struct Message {
    pub json: Value,
    /// False when the account list cannot be completed: a versioned transaction
    /// carries address-table *descriptors*, not the keys those tables resolve to,
    /// while instruction indexes may point at them. Nothing that needs the full
    /// mapping may be derived from such a message.
    pub accounts_complete: bool,
}

/// Decodes a wire-format transaction far enough to recover its message. None when
/// the bytes are not a transaction we can read — never a partial guess.
pub fn decode_message(bytes: &[u8]) -> Option<Message> {
    let mut at = 0usize;
    let signatures = shortvec(bytes, &mut at)?;
    at = at.checked_add(signatures.checked_mul(64)?)?;

    // A leading high bit marks a versioned message; legacy messages start with
    // the header itself.
    let versioned = bytes.get(at)? & 0x80 != 0;
    if versioned {
        at += 1;
    }
    let header = bytes.get(at..at.checked_add(3)?)?.to_vec();
    at += 3;

    let count = shortvec(bytes, &mut at)?;
    let mut keys: Vec<String> = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        keys.push(bs58_encode(bytes.get(at..at.checked_add(32)?)?));
        at += 32;
    }
    at = at.checked_add(32)?; // recent blockhash

    let count = shortvec(bytes, &mut at)?;
    let mut instructions: Vec<Value> = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        let program = *bytes.get(at)?;
        at += 1;
        let len = shortvec(bytes, &mut at)?;
        let accounts: Vec<u64> = bytes
            .get(at..at.checked_add(len)?)?
            .iter()
            .map(|index| u64::from(*index))
            .collect();
        at += len;
        let len = shortvec(bytes, &mut at)?;
        let data = bs58_encode(bytes.get(at..at.checked_add(len)?)?);
        at += len;
        instructions.push(json!({"programIdIndex": program, "accounts": accounts, "data": data}));
    }

    // The lookups name their tables but not their contents, so any that actually
    // load addresses leave us with an account list we cannot complete.
    let mut accounts_complete = true;
    if versioned {
        let tables = shortvec(bytes, &mut at)?;
        for _ in 0..tables {
            at = at.checked_add(32)?;
            let writable = shortvec(bytes, &mut at)?;
            at = at.checked_add(writable)?;
            let readonly = shortvec(bytes, &mut at)?;
            at = at.checked_add(readonly)?;
            accounts_complete = accounts_complete && writable == 0 && readonly == 0;
        }
    }

    Some(Message {
        json: json!({
            "accountKeys": keys,
            "instructions": instructions,
            "header": {
                "numRequiredSignatures": header[0],
                "numReadonlySignedAccounts": header[1],
                "numReadonlyUnsignedAccounts": header[2],
            },
        }),
        accounts_complete,
    })
}

/// Compact-u16: seven bits per byte, high bit continues.
fn shortvec(bytes: &[u8], at: &mut usize) -> Option<usize> {
    let mut value = 0usize;
    for shift in 0..3 {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= usize::from(byte & 0x7f) << (shift * 7);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
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
    let mut out = "1".repeat(leading);
    out.extend(digits.iter().rev().map(|d| BS58[*d as usize] as char));
    if out.is_empty() { "1".to_string() } else { out }
}

pub fn bs58_decode(input: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();
    for c in input.chars() {
        let Some(value) = BS58.iter().position(|b| *b as char == c) else {
            continue;
        };
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
    bytes.extend(std::iter::repeat_n(
        0u8,
        input.chars().take_while(|c| *c == '1').count(),
    ));
    bytes.reverse();
    bytes
}

pub(super) fn u64_at(data: &[u8], offset: usize) -> u64 {
    data.get(offset..offset + 8)
        .map(|slice| u64::from_le_bytes(slice.try_into().unwrap_or([0; 8])))
        .unwrap_or(0)
}

fn key_at(data: &[u8], offset: usize) -> String {
    data.get(offset..offset + 32).map(bs58_encode).unwrap_or_default()
}

fn option_key(data: &[u8], offset: usize) -> String {
    match data
        .get(offset..offset + 4)
        .map(|t| u32::from_le_bytes(t.try_into().unwrap_or([0; 4])))
    {
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
