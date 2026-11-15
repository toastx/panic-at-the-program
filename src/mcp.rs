//! `panic mcp`: the diagnosis as tools an agent calls itself, over MCP's stdio
//! transport, which is one JSON-RPC message per line.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use crate::ai;
use crate::app::solana;

const DEFAULT_CLUSTER: &str = "devnet";

pub fn serve() -> io::Result<()> {
    let mut out = io::stdout();
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&message),
            Err(err) => Some(error(Value::Null, -32700, &format!("parse error: {err}"))),
        };
        if let Some(reply) = reply {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// The reply to one message, or None for a notification, which gets none.
pub fn handle(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let params = &message["params"];
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "panic", "version": env!("CARGO_PKG_VERSION")},
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => call(params["name"].as_str().unwrap_or_default(), &params["arguments"]),
        method => return Some(error(id, -32601, &format!("method not found: {method}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tools() -> Value {
    let cluster = json!({
        "type": "string",
        "description": "mainnet, devnet, testnet, localnet, a name saved with `panic rpc add`, or an RPC url. Default devnet.",
    });
    json!([
        {
            "name": "diagnose_transaction",
            "description": "Call this when a Solana transaction fails. Fetches it by signature and returns a structured diagnosis: the diagnostic id, confidence, failing instruction, accounts involved, the suggested fix, and `action`, what to do next (retry, raise_compute_and_retry, requote_and_retry, fund_account, stop_and_report, fix_transaction).",
            "inputSchema": {
                "type": "object",
                "properties": {"signature": {"type": "string", "description": "the transaction signature, base58"}, "cluster": cluster},
                "required": ["signature"],
            },
        },
        {
            "name": "simulate_transaction",
            "description": "Call this before sending a Solana transaction. Simulates the unsent, serialized transaction and returns the same diagnosis as diagnose_transaction, or status ok with action `send`.",
            "inputSchema": {
                "type": "object",
                "properties": {"transaction": {"type": "string", "description": "the serialized transaction, base64 or base58; it need not be signed"}, "cluster": cluster},
                "required": ["transaction"],
            },
        },
        {
            "name": "inspect_account",
            "description": "Reads a Solana account: lamports, owner, and its data decoded as an SPL mint or token account, or by the owning program's IDL.",
            "inputSchema": {
                "type": "object",
                "properties": {"address": {"type": "string", "description": "the account address, base58"}, "cluster": cluster},
                "required": ["address"],
            },
        },
    ])
}

/// A tool's result. A tool that fails is still a successful call, flagged with
/// `isError` so the agent reads the reason instead of the client hiding it.
fn call(name: &str, arguments: &Value) -> Value {
    let cluster = arguments["cluster"].as_str().unwrap_or(DEFAULT_CLUSTER);
    let argument = |key: &str| arguments[key].as_str().ok_or_else(|| format!("missing `{key}`"));
    let outcome: Result<Value, Box<dyn std::error::Error>> = match name {
        "diagnose_transaction" => argument("signature")
            .map_err(Into::into)
            .and_then(|signature| ai::load(cluster, signature))
            .map(|tx| ai::json(&tx)),
        "simulate_transaction" => argument("transaction")
            .map_err(Into::into)
            .and_then(|transaction| ai::simulate(cluster, transaction)),
        "inspect_account" => argument("address")
            .map_err(Into::into)
            .and_then(|address| inspect(cluster, address)),
        other => Err(format!("unknown tool: {other}").into()),
    };
    let (text, is_error) = match outcome {
        Ok(value) => (serde_json::to_string_pretty(&value).unwrap_or_default(), false),
        Err(err) => (json!({"status": "error", "error": err.to_string()}).to_string(), true),
    };
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

pub fn inspect(cluster: &str, address: &str) -> Result<Value, Box<dyn std::error::Error>> {
    // The account cache never expires, and an agent checking whether its funding
    // landed needs the balance now.
    solana::evict(cluster, &format!("account-{address}"));
    let account = solana::parse_account(address, &solana::fetch_account(cluster, address)?);
    let decoded = crate::ui::screens::decode_account_fields(cluster, &account);
    Ok(json!({
        "key": account.key,
        "lamports": account.lamports,
        "owner": account.owner,
        "owner_name": account.owner_name(),
        "executable": account.executable,
        "data_len": account.data.len(),
        "type": decoded.as_ref().map(|(layout, _)| layout),
        "fields": decoded.map(|(_, fields)| fields
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect::<serde_json::Map<_, _>>()),
    }))
}
