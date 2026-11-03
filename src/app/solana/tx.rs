//! A fetched transaction parsed into its instruction tree, accounts, and
//! balance changes.

use super::*;
use serde_json::Value;

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
    pub failure: Option<crate::app::diagnose::Failure>,
    pub findings: Vec<crate::app::validate::Finding>,
    pub slot: u64,
    pub nodes: Vec<Node>,
    pub accounts: Vec<Account>,
    pub balances: Vec<BalanceChange>,
    pub logs: Vec<String>,
    /// Fee payer and what it held: `accountKeys[0]` with `preBalances[0]`, which is
    /// what the chain recorded. None when the response carried neither.
    pub payer: Option<(String, u64)>,
    /// Lamports the payer held when we looked, for transactions that never
    /// landed and so have no recorded balance. A snapshot, labelled as one.
    pub payer_live: Option<u64>,
    /// False when the account list is known to be short of what the transaction
    /// actually loaded, so account-derived findings must be withheld.
    pub accounts_complete: bool,
}

impl Tx {
    pub fn log_frames(&self) -> Vec<Option<usize>> {
        let mut frames = vec![None; self.logs.len()];
        for (index, node) in self.nodes.iter().enumerate() {
            for line in node.logs.clone() {
                let deeper = frames[line]
                    .map(|current: usize| self.nodes[current].depth < node.depth)
                    .unwrap_or(true);
                if deeper {
                    frames[line] = Some(index);
                }
            }
        }
        frames
    }

    pub fn failure_path(&self) -> Vec<usize> {
        self.failed_node
            .map(|failed| self.ancestors(failed))
            .unwrap_or_default()
    }

    /// Call chain from the top-level instruction down to `index`, inclusive.
    pub fn ancestors(&self, index: usize) -> Vec<usize> {
        let mut path = vec![index];
        for i in (0..index).rev() {
            if self.nodes[i].depth < self.nodes[path[path.len() - 1]].depth {
                path.push(i);
            }
        }
        path.reverse();
        path
    }
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
    accounts.extend(loaded_writable.into_iter().map(|key| Account {
        key,
        signer: false,
        writable: true,
    }));
    accounts.extend(loaded_readonly.into_iter().map(|key| Account {
        key,
        signer: false,
        writable: false,
    }));
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
                    None => Account {
                        key: format!("#{i}"),
                        signer: false,
                        writable: false,
                    },
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
                            t.pointer("/uiTokenAmount/uiAmount")
                                .and_then(Value::as_f64)
                                .unwrap_or(0.0),
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
        let Some(rest) = line.strip_prefix("Program ") else {
            continue;
        };
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

            counters.resize(depth, 0);
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
        } else if let Some((_, message)) = rest.split_once(" failed: ")
            && let Some(idx) = stack.pop()
        {
            nodes[idx].status = Status::Failed;
            nodes[idx].error = Some(message.to_string());
            nodes[idx].logs.end = line_index + 1;
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
    let error = if result["meta"]["err"].is_null() {
        None
    } else {
        {
            nodes
                .iter()
                .find(|n| n.error.is_some())
                .and_then(|n| n.error.clone())
                .or_else(|| Some(result["meta"]["err"].to_string()))
        }
    };

    let cu = result["meta"]["computeUnitsConsumed"].as_u64().unwrap_or(0) as u32;
    let cu_limit = if cu_limit == 0 { 200_000 } else { cu_limit };
    let failure = crate::app::diagnose::failure(&result["meta"]["err"], &nodes, &logs, cu, cu_limit);
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
        // Filled by idl::enrich, which can reach the network for account state.
        findings: Vec::new(),
        balances: balance_changes(result, &keys),
        payer: match (
            keys.first(),
            result.pointer("/meta/preBalances/0").and_then(Value::as_u64),
        ) {
            (Some(account), Some(lamports)) => Some((account.key.clone(), lamports)),
            _ => None,
        },
        payer_live: value.pointer("/patp/payerBalance").and_then(Value::as_u64),
        accounts_complete: value
            .pointer("/patp/accountsComplete")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        accounts: keys,
        logs,
        nodes,
    }
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
                            crate::ui::widgets::short(&account(1)),
                        )
                    }
                    7 => ("Token::MintTo", account(0), account(1), raw, "tokens".to_string()),
                    8 => ("Token::Burn", account(0), account(1), raw, "tokens".to_string()),
                    _ => continue,
                };
                out.push(Transfer {
                    node,
                    kind: kind.to_string(),
                    from,
                    to,
                    amount,
                    unit,
                });
            }
            _ => {}
        }
    }
    out
}
