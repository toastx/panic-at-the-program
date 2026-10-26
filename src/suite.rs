//! Run the project's test suite, scrape SVM program logs out of the output, and
//! rebuild each failed transaction as a `getTransaction` response so the whole
//! TUI (execution tree, logs, diagnosis) works on a local test failure.
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

/// Pseudo-cluster the reconstructed failures are cached under.
pub const CLUSTER: &str = "tests";

pub struct Case {
    pub name: String,
    pub logs: Vec<String>,
    pub err: Value,
    pub cu: u32,
}

/// Framework label plus the command that runs its suite. Anchor is pointed at a
/// validator we own instead of the throwaway one it starts and tears down, so the
/// signatures in its output are still fetchable once the suite is over.
pub fn detect(root: &Path) -> Option<(&'static str, Vec<&'static str>)> {
    if root.join("Anchor.toml").is_file() {
        return Some((
            "anchor",
            vec![
                "anchor",
                "test",
                "--skip-local-validator",
                "--provider.cluster",
                "localnet",
            ],
        ));
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let label = if manifest.contains("mollusk") {
        "mollusk"
    } else {
        "cargo"
    };
    Some((label, vec!["cargo", "test", "--", "--nocapture"]))
}

/// Combined stdout+stderr: frameworks split program logs across both.
pub fn run(root: &Path, command: &[&str]) -> std::io::Result<String> {
    let output = Command::new(command[0])
        .args(&command[1..])
        .current_dir(root)
        // litesvm / solana-program-test only emit program logs at this level.
        .env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "solana_runtime::message_processor=debug".to_string()),
        )
        .output()?;
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

/// A `solana-test-validator` that outlives the suite, so failed transactions can
/// still be fetched with their accounts and balances. Killed on drop.
pub struct Validator(Option<std::process::Child>);

impl Validator {
    /// Reuses whatever is already serving localnet; otherwise starts one and waits
    /// for it to answer. `Ok(None)` means there was nothing to start and nothing
    /// listening either.
    pub fn start(root: &Path) -> std::io::Result<Option<Self>> {
        if crate::solana::healthy("localnet") {
            return Ok(Some(Validator(None)));
        }
        let child = Command::new("solana-test-validator")
            .args(["--reset", "--quiet", "--ledger"])
            .arg(root.join("test-ledger"))
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let mut validator = Validator(Some(child));
        for _ in 0..100 {
            if crate::solana::healthy("localnet") {
                return Ok(Some(validator));
            }
            if let Some(status) = validator.0.as_mut().expect("child").try_wait()? {
                return Err(std::io::Error::other(format!("solana-test-validator exited: {status}")));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        Err(std::io::Error::other("solana-test-validator never became healthy"))
    }
}

impl Drop for Validator {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Base58 signatures in the output that localnet confirms as failed, newest last.
pub fn failed_signatures(output: &str, cluster: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for token in output.split(|c: char| !c.is_ascii_alphanumeric()) {
        if !crate::solana::is_signature(token)
            || token.len() > 90
            || found.iter().any(|seen| seen == token)
            || crate::solana::bs58_decode(token).len() != 64
        {
            continue;
        }
        if crate::solana::fetch(cluster, token).is_ok_and(|value| !value["result"]["meta"]["err"].is_null()) {
            found.push(token.to_string());
        } else {
            crate::solana::evict(cluster, token);
        }
    }
    found
}

/// Strips whatever the framework wrapped a program log line in: a mocha quoted
/// array entry, a `solana_logger` timestamp prefix, or nothing at all.
fn log_line(line: &str) -> Option<&str> {
    let start = line.find("Program ")?;
    Some(line[start..].trim_end().trim_end_matches([',', '\'', '"', ' ']))
}

fn test_name(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("---- ") {
        return rest.strip_suffix(" stdout ----").map(str::to_string);
    }
    if let Some(rest) = line.strip_prefix("test ") {
        let name = rest.split(" ... ").next().unwrap_or(rest);
        return (!name.is_empty() && !name.starts_with("result:")).then(|| name.to_string());
    }
    // mocha: `✔ does the thing (41ms)` while running, `1) does the thing` when reporting.
    let rest = ["✔ ", "✓ ", "✗ ", "✘ ", "✖ ", "✕ "]
        .iter()
        .find_map(|mark| line.strip_prefix(mark))
        .or_else(|| {
            let (number, rest) = line.split_once(") ")?;
            number.chars().all(|c| c.is_ascii_digit()).then_some(rest)
        })?;
    let name = rest.trim().trim_end_matches(':').trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// `custom program error: 0x1771` -> `{"InstructionError": [0, {"Custom": 6001}]}`,
/// anything else keeps the runtime's own wording.
fn error_value(index: usize, message: &str) -> Value {
    let code = message
        .split_once("0x")
        .and_then(|(_, hex)| u32::from_str_radix(hex.split_whitespace().next().unwrap_or(""), 16).ok());
    match code {
        Some(code) => json!({"InstructionError": [index, {"Custom": code}]}),
        None => json!({"InstructionError": [index, message.trim()]}),
    }
}

/// One case per failed transaction found in the output.
pub fn cases(output: &str) -> Vec<Case> {
    let lines: Vec<&str> = output.lines().collect();
    let names: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| test_name(line).map(|name| (i, name)))
        .collect();

    let mut done: Vec<(usize, Case)> = Vec::new();
    let mut open: Option<(Vec<String>, u32)> = None;
    let mut depth = 0usize;

    for (index, raw) in lines.iter().enumerate() {
        let Some(line) = log_line(raw) else { continue };
        let Some(rest) = line.strip_prefix("Program ") else {
            continue;
        };

        if let Some((_, tail)) = rest.split_once(" invoke [") {
            depth = tail.trim_end_matches(']').parse().unwrap_or(depth + 1);
            if depth == 1 && open.is_none() {
                open = Some((Vec::new(), 0));
            }
        }
        let Some((logs, cu)) = open.as_mut() else { continue };
        logs.push(line.to_string());

        if depth == 1
            && let Some((_, tail)) = rest.split_once(" consumed ")
            && let Some(consumed) = tail.split(' ').next().and_then(|v| v.parse::<u32>().ok())
        {
            *cu += consumed;
        }
        let failed = rest.split_once(" failed: ");
        if rest.ends_with(" success") || failed.is_some() {
            depth = depth.saturating_sub(1);
        }
        if let Some((_, message)) = failed
            && depth == 0
        {
            let (logs, cu) = open.take().expect("open case");
            done.push((
                index,
                Case {
                    name: String::new(),
                    err: error_value(0, message),
                    logs,
                    cu,
                },
            ));
        } else if depth == 0 {
            open = None; // the transaction succeeded, nothing to diagnose
        }
    }

    let mut cases: Vec<Case> = Vec::new();
    for (index, mut case) in done {
        // Nearest test name either side: cargo prints it before the logs, mocha after.
        case.name = names
            .iter()
            .min_by_key(|(at, _)| at.abs_diff(index))
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| format!("failure {}", cases.len() + 1));
        if !cases.iter().any(|seen| seen.logs == case.logs) {
            cases.push(case);
        }
    }
    cases
}

/// A `getTransaction` response carrying only what the logs can tell us.
fn transaction_json(case: &Case) -> Value {
    json!({"result": {
        "slot": 0,
        "meta": {
            "err": case.err,
            "fee": 0,
            "computeUnitsConsumed": case.cu,
            "logMessages": case.logs,
            "preBalances": [],
            "postBalances": [],
            "innerInstructions": [],
        },
        "transaction": {"message": {
            "accountKeys": [],
            "instructions": [],
            "header": {"numRequiredSignatures": 0, "numReadonlySignedAccounts": 0, "numReadonlyUnsignedAccounts": 0},
        }},
    }})
}

fn key(name: &str) -> String {
    let key: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    key.trim_matches('-').chars().take(60).collect()
}

/// Caches each case so `App::open_transaction` can load it like any signature.
/// Returns the keys, most recently failed first.
pub fn stash(cases: &[Case]) -> Vec<String> {
    cases
        .iter()
        .enumerate()
        .filter_map(|(index, case)| {
            let key = match key(&case.name) {
                key if key.is_empty() => format!("failure-{}", index + 1),
                key => key,
            };
            crate::solana::store(CLUSTER, &key, &transaction_json(case)).ok()?;
            Some(key)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    const MOCHA: &str = r#"
  my program
Error: AnchorError caused by account: vault. Error Code: ConstraintSeeds. Error Number: 2006.
    logs: [
      'Program Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS invoke [1]',
      'Program log: Instruction: Deposit',
      'Program log: AnchorError caused by account: vault. Error Code: ConstraintSeeds. Error Number: 2006. Error Message: A seeds constraint was violated.',
      'Program Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS consumed 4521 of 200000 compute units',
      'Program Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS failed: custom program error: 0x7d6',
    ]
    1) deposits into the vault
"#;

    const CARGO: &str = r#"
---- tests::transfer_fails stdout ----
[2026-10-26T00:00:00Z DEBUG solana_runtime::message_processor::stable_log] Program 11111111111111111111111111111111 invoke [1]
[2026-10-26T00:00:00Z DEBUG solana_runtime::message_processor::stable_log] Program 11111111111111111111111111111111 consumed 150 of 200000 compute units
[2026-10-26T00:00:00Z DEBUG solana_runtime::message_processor::stable_log] Program 11111111111111111111111111111111 failed: insufficient funds for instruction
"#;

    #[test]
    fn scrapes_failures_from_both_report_styles() {
        let cases = super::cases(MOCHA);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "deposits into the vault");
        assert_eq!(cases[0].cu, 4521);
        assert_eq!(cases[0].err["InstructionError"][1]["Custom"], 2006);
        assert_eq!(cases[0].logs.len(), 5);
        assert_eq!(super::key(&cases[0].name), "deposits-into-the-vault");

        let cases = super::cases(CARGO);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "tests::transfer_fails");
        assert_eq!(
            cases[0].err["InstructionError"][1],
            "insufficient funds for instruction"
        );

        // the reconstructed response drives the normal parse + diagnose path
        let cases = super::cases(MOCHA);
        let tx = crate::solana::parse("deposit", super::CLUSTER, &super::transaction_json(&cases[0]));
        let failure = tx.failure.expect("diagnosed");
        assert_eq!(tx.nodes.len(), 1);
        assert_eq!(tx.cu, 4521);
        assert!(failure.headline.starts_with("ConstraintSeeds"), "{}", failure.headline);
        assert_eq!(failure.account.as_deref(), Some("vault"));

        // a suite that only prints successful transactions yields nothing to open
        assert!(
            super::cases(
                "Program 11111111111111111111111111111111 invoke [1]\nProgram 11111111111111111111111111111111 success"
            )
            .is_empty()
        );
    }
}
