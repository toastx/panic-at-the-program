//! Handing a diagnosis to an AI coding agent: the markdown context, the
//! clipboard escape, and the agent command itself.

use std::io::{self, BufRead, Write};
use std::process::Command;

use crate::app::diagnose::{Confidence, Failure, Tier};
use crate::app::solana::{Node, Status, Tx, base64_encode};
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
pub fn program(agent: &str) -> &str {
    agent.split_whitespace().next().unwrap_or(agent)
}

/// The one-time opt-in, asked on stdin before the TUI takes the screen.
pub fn setup() -> io::Result<()> {
    // No one at the keyboard (piped stdin): leave it unanswered and ask again
    // rather than opting the user out on their behalf.
    let Some(answer) = ask("enable AI agent export? [y/n] ")? else {
        return Ok(());
    };
    if !answer.to_lowercase().starts_with('y') {
        return workspace::set_global("ai", "enabled", "false");
    }
    let agent = ask(&format!(
        "agent command [{DEFAULT_AGENT}] (e.g. codex, gemini, opencode): "
    ))?
    .filter(|line| !line.is_empty())
    .unwrap_or_else(|| DEFAULT_AGENT.to_string());
    workspace::set_global("ai", "enabled", "true")?;
    workspace::set_global("ai", "agent", &agent)?;
    println!("saved agent \"{agent}\" in {}", workspace::global_path().display());
    Ok(())
}

/// A trimmed line from stdin, or None at EOF.
fn ask(prompt: &str) -> io::Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line)? == 0 {
        println!();
        return Ok(None);
    }
    Ok(Some(line.trim().to_string()))
}

/// Copies through OSC 52, which the terminal forwards to the system clipboard —
/// no clipboard dependency, and it survives SSH.
pub fn copy(text: &str) -> io::Result<()> {
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))?;
    out.flush()
}

/// Runs the agent with the context as its prompt, in this terminal.
pub fn send(agent: &str, prompt: &str) -> io::Result<()> {
    let mut parts = agent.split_whitespace();
    let binary = parts.next().unwrap_or(DEFAULT_AGENT);
    Command::new(binary).args(parts).arg(prompt).status().map(drop)
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
    out.extend(tx.nodes.iter().map(|node| {
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
    }));
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

    if !tx.accounts.is_empty() {
        out.push(String::new());
        out.push("## Accounts".to_string());
        out.extend(
            tx.accounts
                .iter()
                .enumerate()
                .map(|(index, account)| format!("- #{index} `{}` — {}", account.key, account.role())),
        );
    }

    out.push(String::new());
    out.join("\n")
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
