//! Handing a diagnosis to an AI coding agent: the markdown context, the
//! clipboard escape, and a new terminal window running the agent.

use std::io::{self, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::app::diagnose::{Confidence, DiagnosticId, Failure, LogId, Tier};
use crate::app::errors::Class;
use crate::app::idl;
use crate::app::solana::{self, ATA, Node, SYSTEM, Status, TOKEN, TOKEN_2022, Tx, base64_encode, bs58_decode};
use crate::ui::widgets::node_label;
use crate::workspace;

const DEFAULT_AGENT: &str = "claude";

/// Lines of program log carried along with the diagnosis. The failing frame is
/// usually short; this only bites when the whole transaction is the slice.
const LOG_LIMIT: usize = 60;

fn setting(key: &str) -> Option<String> {
    workspace::global("ai")
        .into_iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

/// Whether `[ai]` has an answer already, either way: the opt-in is asked once.
pub fn configured() -> bool {
    setting("enabled").is_some()
}

/// The configured agent command, or None when export is off.
pub fn agent() -> Option<String> {
    (setting("enabled")? == "true").then(|| setting("agent").unwrap_or_else(|| DEFAULT_AGENT.to_string()))
}

/// The binary to look for on PATH: `codex exec` is still `codex`.
fn program(agent: &str) -> &str {
    agent.split_whitespace().next().unwrap_or(agent)
}

/// Records the one-time opt-in answer from the TUI's first-run dialog.
pub fn set_enabled(enabled: bool) -> io::Result<()> {
    workspace::set_global("ai", "enabled", if enabled { "true" } else { "false" })
}

/// Copies through OSC 52, which the terminal forwards to the system clipboard —
/// no clipboard dependency, and it survives SSH.
pub fn copy(text: &str) -> io::Result<()> {
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))?;
    out.flush()
}

/// Opens the agent in a new terminal window, with the context as its opening
/// prompt, and leaves the TUI where it is. Returns where it opened.
pub fn open(agent: &str, prompt: &str) -> io::Result<&'static str> {
    if !on_path(program(agent)) {
        return Err(io::Error::other(format!("`{}` not on PATH", program(agent))));
    }
    let dir = std::env::temp_dir();
    let context = dir.join("panic-context.md");
    let launcher = dir.join("panic-agent.command");
    std::fs::write(&context, prompt)?;
    let cwd = std::env::current_dir()?;
    std::fs::write(
        &launcher,
        script(agent, &cwd.to_string_lossy(), &context.to_string_lossy()),
    )?;
    let launcher = launcher.to_string_lossy().into_owned();
    let (name, mut command) = terminal(&launcher).ok_or_else(|| io::Error::other("no supported terminal found"))?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| name)
}

/// The shell script the new window runs: back to this directory, then the agent.
pub fn script(agent: &str, cwd: &str, context: &str) -> String {
    format!(
        "#!/bin/sh\ncd {}\nexec {agent} \"$(cat {})\"\n",
        quote(cwd),
        quote(context)
    )
}

/// Single-quoted for sh: a `'` inside closes, escapes, and reopens.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The terminal we are running in, asked for a new window running `launcher`.
// ponytail: no Windows launcher yet (wt + a .ps1); `open` errors and the caller copies instead.
fn terminal(launcher: &str) -> Option<(&'static str, Command)> {
    let env = |key: &str| std::env::var(key).unwrap_or_default();
    let with = |program: &str, args: &[&str]| {
        let mut command = Command::new(program);
        command.args(args);
        command
    };
    if !env("TMUX").is_empty() {
        return Some(("tmux", with("tmux", &["new-window", "sh", launcher])));
    }
    let mac = cfg!(target_os = "macos");
    Some(match env("TERM_PROGRAM").as_str() {
        "WezTerm" => ("WezTerm", with("wezterm", &["start", "--", "sh", launcher])),
        "ghostty" if mac => (
            "Ghostty",
            with("open", &["-na", "Ghostty", "--args", "-e", "sh", launcher]),
        ),
        "ghostty" => ("Ghostty", with("ghostty", &["-e", "sh", launcher])),
        "iTerm.app" => (
            "iTerm",
            with(
                "osascript",
                &[
                    "-e",
                    &format!(
                        "tell application \"iTerm\" to create window with default profile command \"sh {launcher}\""
                    ),
                ],
            ),
        ),
        _ if !env("KITTY_WINDOW_ID").is_empty() => ("kitty", with("kitty", &["sh", launcher])),
        _ if mac => {
            // Terminal.app runs an executable `.command` file it is asked to open.
            executable(launcher).ok()?;
            ("Terminal", with("open", &["-a", "Terminal", launcher]))
        }
        _ if cfg!(unix) && on_path("x-terminal-emulator") => {
            ("terminal", with("x-terminal-emulator", &["-e", "sh", launcher]))
        }
        _ if cfg!(unix) => ("gnome-terminal", with("gnome-terminal", &["--", "sh", launcher])),
        _ => return None,
    })
}

#[cfg(unix)]
fn executable(path: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn executable(_: &str) -> io::Result<()> {
    Ok(())
}

/// Found on PATH, without running it: `--version` on a terminal can open a window.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// What the agent gets: the transaction, our diagnosis, the call tree, the logs
/// around the failure, and the accounts it loaded.
pub fn markdown(tx: &Tx) -> String {
    let mut out = vec![
        "Help me debug why this Solana transaction failed.".to_string(),
        String::new(),
        "## Transaction".to_string(),
        format!("- signature: `{}`", tx.signature),
        format!("- cluster: {}", tx.cluster),
        format!("- slot: {}", tx.slot),
        format!(
            "- status: {}",
            match tx.status {
                Status::Failed => "failed",
                Status::Ok => "succeeded",
            }
        ),
        format!("- compute: {} of {} CU", tx.cu, tx.cu_limit),
        format!("- fee: {} lamports", tx.fee),
    ];
    if let Some(error) = &tx.error {
        out.push(format!("- error: {error}"));
    }
    if let Some(failure) = &tx.failure {
        out.extend(diagnosis(tx, failure));
    }

    out.push(String::new());
    out.push("## Call tree".to_string());
    out.push("```".to_string());
    out.extend(tree(tx));
    out.push("```".to_string());

    let logs = frame_logs(tx);
    if !logs.is_empty() {
        out.push(String::new());
        out.push("## Logs".to_string());
        out.push("```".to_string());
        out.extend(logs.iter().take(LOG_LIMIT).cloned());
        if logs.len() > LOG_LIMIT {
            out.push(format!("... {} more lines", logs.len() - LOG_LIMIT));
        }
        out.push("```".to_string());
    }

    let accounts = accounts(tx);
    if !accounts.is_empty() {
        out.push(String::new());
        out.push("## Accounts".to_string());
        out.extend(accounts);
    }

    out.push(String::new());
    out.join("\n")
}

/// The failing frame's own logs, which is what the diagnosis was read from;
/// the whole transaction's when nothing failed.
fn frame_logs(tx: &Tx) -> &[String] {
    failing(tx)
        .and_then(|node| tx.logs.get(tx.nodes[node].logs.clone()))
        .unwrap_or(&tx.logs)
}

const COMPUTE_BUDGET: &str = "ComputeBudget111111111111111111111111111111";

/// The node the failure points at, when there is one.
fn failing(tx: &Tx) -> Option<usize> {
    tx.failure.as_ref()?.node
}

/// Programs and sysvars an instruction is handed only so it can call them:
/// readonly, they tell an agent nothing about why it failed.
fn plumbing(key: &str) -> bool {
    matches!(key, SYSTEM | TOKEN | TOKEN_2022 | ATA | COMPUTE_BUDGET) || key.starts_with("Sysvar")
}

/// The failing top-level instruction in full; the others one line each, in
/// order, so "the ATA was created first" survives without their CPIs.
/// Compute Budget is dropped: its limit is already in the header.
fn tree(tx: &Tx) -> Vec<String> {
    let line = |node: &Node| {
        format!(
            "{:indent$}{} {}  {} CU (self {}){}",
            "",
            node.path,
            frame(node),
            node.cu,
            node.self_cu,
            match node.status {
                Status::Failed => "  <-- failed here",
                Status::Ok => "",
            },
            indent = node.depth * 2,
        )
    };
    let Some(failed) = failing(tx) else {
        return tx.nodes.iter().map(line).collect();
    };
    let top = tx.nodes[..=failed]
        .iter()
        .rposition(|node| node.depth == 0)
        .unwrap_or(0);
    let mut out = Vec::new();
    let mut inside = false;
    for (index, node) in tx.nodes.iter().enumerate() {
        if node.depth == 0 {
            inside = index == top;
        }
        if inside {
            out.push(line(node));
        } else if node.depth == 0 && node.program != COMPUTE_BUDGET {
            out.push(format!("{} {} — ok", node.path, frame(node)));
        }
    }
    out
}

/// The failing instruction's accounts under their IDL names, minus readonly
/// plumbing, unless the error or a finding points at it. Without a failing
/// instruction, every account the transaction loaded, minus the same plumbing.
fn accounts(tx: &Tx) -> Vec<String> {
    let Some(node) = failing(tx).map(|index| &tx.nodes[index]) else {
        return tx
            .accounts
            .iter()
            .filter(|account| account.signer || account.writable || !plumbing(&account.key))
            .map(|account| format!("- `{}` — {}", account.key, account.role()))
            .collect();
    };
    let decoded = decoded(tx, node);
    let blamed = tx.failure.as_ref().and_then(|failure| failure.account.as_deref());

    let mut out = Vec::new();
    if let Some(decoded) = decoded.as_ref().filter(|decoded| !decoded.args.is_empty()) {
        out.push(format!(
            "args: {}",
            decoded
                .args
                .iter()
                .map(|(name, value)| format!("{name} = {value}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for (index, account) in node.accounts.iter().enumerate() {
        let name = decoded
            .as_ref()
            .and_then(|decoded| decoded.specs.get(index))
            .map(|spec| spec.name.as_str());
        let findings: Vec<&str> = tx
            .findings
            .iter()
            .filter(|finding| finding.key == account.key)
            .map(|finding| finding.message.as_str())
            .collect();
        let pointed_at = blamed.is_some() && name == blamed;
        if !account.signer && !account.writable && plumbing(&account.key) && !pointed_at && findings.is_empty() {
            continue;
        }
        let mut entry = match name {
            Some(name) => format!("- #{index} {name} `{}` — {}", account.key, account.role()),
            None => format!("- #{index} `{}` — {}", account.key, account.role()),
        };
        for finding in findings {
            entry.push_str(&format!("\n  - {finding}"));
        }
        out.push(entry);
    }
    out
}

/// The failing instruction as its program's IDL reads it: name, args, and the
/// declared account specs by position.
fn decoded(tx: &Tx, node: &Node) -> Option<idl::Decoded> {
    idl::idl(&tx.cluster, &node.program).and_then(|idl| idl::instruction(&idl, &bs58_decode(&node.data)))
}

fn tier(tier: Tier) -> &'static str {
    match tier {
        Tier::Span => "span",
        Tier::Note => "note",
        Tier::Help => "help",
        Tier::Fix => "fix",
    }
}

/// A tree frame as an agent wants it: the full program id rather than the short
/// label the screen has room for, plus the instruction name when one is known.
fn frame(node: &Node) -> String {
    match &node.name {
        Some(name) => format!("{} · {name}", node.program),
        None => node.program.clone(),
    }
}

/// The diagnosis as the Overview screen states it: the error, what it means,
/// where it was thrown, and every note we have with its tier.
fn diagnosis(tx: &Tx, failure: &Failure) -> Vec<String> {
    let mut out = vec![String::new(), format!("## {}", failure.label())];
    if !failure.meaning.is_empty() {
        out.push(failure.meaning.clone());
    }
    out.push(String::new());
    if let Some(class) = failure.class {
        out.push(format!("- class: {class:?}"));
    }
    if let Some(node) = failure.node.map(|index| &tx.nodes[index]) {
        out.push(format!("- failing instruction: {} {}", node.path, node_label(node)));
        out.push(format!("- program: `{}`", node.program));
    }
    if let Some(source) = &failure.source {
        out.push(format!("- thrown in: {source}"));
    }
    if let Some(account) = &failure.account {
        out.push(format!("- caused by account: {account}"));
    }
    out.extend(failure.notes.iter().map(|note| {
        let tier = tier(note.tier);
        let guess = match note.confidence {
            Confidence::Likely => " (inferred)",
            Confidence::Certain => "",
        };
        let values = match (&note.expected, &note.actual) {
            (Some(expected), Some(actual)) => format!(" — expected `{expected}`, got `{actual}`"),
            (Some(expected), None) => format!(" — expected `{expected}`"),
            (None, Some(actual)) => format!(" — got `{actual}`"),
            (None, None) => String::new(),
        };
        format!("- {tier}: {}{values}{guess}", note.text)
    }));
    out
}

/// Fetches a transaction and diagnoses it the way the TUI does, without one.
pub fn load(cluster: &str, signature: &str) -> Result<Tx, Box<dyn std::error::Error>> {
    Ok(analyze(signature, cluster, &solana::fetch(cluster, signature)?))
}

/// Diagnoses a `getTransaction`-shaped value.
pub fn analyze(signature: &str, cluster: &str, value: &Value) -> Tx {
    let mut tx = solana::parse(signature, cluster, value);
    idl::enrich(&mut tx);
    tx
}

/// What an agent should do next, and whether resending can succeed. Agents
/// need a decision, not an explanation. `class` only applies to the failure's
/// own id, because notes have no class.
fn action(id: &DiagnosticId, class: Option<Class>) -> Option<(&'static str, bool)> {
    Some(match id {
        // Write-lock contention with another transaction: it clears on its own.
        DiagnosticId::Runtime(name) if name == "AccountInUse" => ("retry", true),
        DiagnosticId::Log(LogId::CuExceeded) => ("raise_compute_and_retry", true),
        DiagnosticId::Log(LogId::Slippage) => ("requote_and_retry", true),
        DiagnosticId::Log(LogId::LamportShortfall) => ("fund_account", false),
        DiagnosticId::Log(LogId::SignatureMissing) => ("stop_and_report", false),
        // "already in use" means the address exists. It is not a lock, so a
        // resend fails the same way.
        DiagnosticId::Log(LogId::AccountInUse) => ("fix_transaction", false),
        _ => match class? {
            Class::Expired => ("retry", true),
            Class::Slippage => ("requote_and_retry", true),
            Class::Insufficient | Class::Rent => ("fund_account", false),
            Class::NotSigner => ("stop_and_report", false),
            _ => return None,
        },
    })
}

fn confidence(confidence: Option<Confidence>) -> &'static str {
    match confidence {
        Some(Confidence::Certain) => "high",
        Some(Confidence::Likely) => "medium",
        None => "low",
    }
}

/// The diagnosis as JSON for an agent: the TUI's content plus `action`.
pub fn json(tx: &Tx) -> Value {
    let compute = json!({"used": tx.cu, "limit": tx.cu_limit});
    let Some(failure) = &tx.failure else {
        return json!({
            "status": "ok",
            "signature": tx.signature,
            "cluster": tx.cluster,
            "action": "none",
            "retryable": false,
            "compute": compute,
        });
    };

    // The failure's own id first, then the notes in the order they render.
    let (diagnostic, (action, retryable)) = std::iter::once((&failure.id, failure.class))
        .chain(failure.notes.iter().map(|note| (&note.id, None)))
        .find_map(|(id, class)| Some((id, action(id, class)?)))
        .unwrap_or((&failure.id, ("fix_transaction", false)));
    let certainty = failure
        .notes
        .iter()
        .find(|note| &note.id == diagnostic)
        .or_else(|| {
            failure
                .notes
                .iter()
                .find(|note| note.id == DiagnosticId::Log(LogId::ClassInferred))
        })
        .map(|note| note.confidence);
    let fix = [Tier::Fix, Tier::Help]
        .iter()
        .find_map(|&wanted| failure.notes.iter().find(|note| note.tier == wanted))
        .map(|note| note.text.clone());

    let node = failure.node.map(|index| &tx.nodes[index]);
    let specs = node
        .and_then(|node| decoded(tx, node))
        .map(|decoded| decoded.specs)
        .unwrap_or_default();
    let accounts: Vec<Value> = node
        .map_or(&tx.accounts, |node| &node.accounts)
        .iter()
        .enumerate()
        .map(|(index, account)| {
            json!({
                "index": index,
                "name": specs.get(index).map(|spec| &spec.name),
                "key": account.key,
                "signer": account.signer,
                "writable": account.writable,
                "findings": tx
                    .findings
                    .iter()
                    .filter(|finding| finding.key == account.key)
                    .map(|finding| &finding.message)
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let notes: Vec<Value> = failure
        .notes
        .iter()
        .map(|note| {
            json!({
                "id": note.id.to_string(),
                "tier": tier(note.tier),
                "confidence": confidence(Some(note.confidence)),
                "text": note.text,
                "expected": note.expected,
                "actual": note.actual,
            })
        })
        .collect();

    json!({
        "status": "failed",
        "signature": tx.signature,
        "cluster": tx.cluster,
        "diagnostic": diagnostic.to_string(),
        "confidence": confidence(certainty),
        "action": action,
        "retryable": retryable,
        "error": {
            "id": failure.id.to_string(),
            "name": failure.headline,
            "meaning": failure.meaning,
            "class": failure.class.map(|class| format!("{class:?}")),
            "account": failure.account,
            "source": failure.source,
        },
        "instruction": node.map(|node| node.name.clone().unwrap_or_else(|| node.label.clone())),
        "instruction_path": node.map(|node| &node.path),
        "program": node.map(|node| &node.program),
        "fix": fix,
        "accounts": accounts,
        "notes": notes,
        "compute": compute,
        "logs": frame_logs(tx).iter().take(LOG_LIMIT).collect::<Vec<_>>(),
    })
}

/// Diagnoses an unsent transaction, given as base64 or base58, by simulating it.
/// A failure takes the same path as one the rpc tap recorded.
pub fn simulate(cluster: &str, transaction: &str) -> Result<Value, Box<dyn std::error::Error>> {
    // base58 is a subset of base64's alphabet, so only a message that decodes
    // shows which encoding it was. Anything else goes to the RPC as base64: it
    // reads formats our decoder does not (v1), and it says why when it cannot.
    let bytes = bs58_decode(transaction);
    let transaction = match solana::decode_message(&solana::base64_decode(transaction)) {
        None if solana::decode_message(&bytes).is_some() => base64_encode(&bytes),
        _ => transaction.to_string(),
    };
    let (request, response) = solana::simulate(cluster, &transaction)?;
    let Some(capture) = crate::suite::tap::capture(&serde_json::to_vec(&request)?, &serde_json::to_vec(&response)?)
    else {
        let value = &response["result"]["value"];
        return Ok(json!({
            "status": "ok",
            "cluster": cluster,
            "action": "send",
            "retryable": false,
            "compute": {"used": value["unitsConsumed"]},
            "logs": value["logs"],
        }));
    };
    let case = crate::suite::captured(&[capture])
        .pop()
        .ok_or("simulation reported no result")?;
    let tx = analyze("simulation", cluster, &crate::suite::transaction_json(&case));
    Ok(json(&tx))
}
