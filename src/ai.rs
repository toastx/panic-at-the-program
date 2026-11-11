//! Handing a diagnosis to an AI coding agent: the markdown context, the
//! clipboard escape, and a new terminal window running the agent.

use std::io::{self, Write};
use std::process::{Command, Stdio};

use crate::app::diagnose::{Confidence, Failure, Tier};
use crate::app::idl;
use crate::app::solana::{ATA, Node, SYSTEM, Status, TOKEN, TOKEN_2022, Tx, base64_encode, bs58_decode};
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

    // The failing frame's own logs, which is what the diagnosis was read from.
    let frame = tx
        .failure
        .as_ref()
        .and_then(|failure| failure.node)
        .and_then(|node| tx.logs.get(tx.nodes[node].logs.clone()));
    let logs = frame.unwrap_or(&tx.logs);
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
    let decoded = idl::idl(&tx.cluster, &node.program).and_then(|idl| idl::instruction(&idl, &bs58_decode(&node.data)));
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
        let tier = match note.tier {
            Tier::Span => "span",
            Tier::Note => "note",
            Tier::Help => "help",
            Tier::Fix => "fix",
        };
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
