pub(crate) mod tap;

use std::path::Path;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// Pseudo-cluster the reconstructed failures are cached under.
pub const CLUSTER: &str = "tests";

/// Placeholder the suite command carries until the tap replaces it with its url.
pub const CLUSTER_ARG: &str = "localnet";

pub struct Case {
    pub name: String,
    /// `None` when nothing reported logs, as opposed to reporting none.
    pub logs: Option<Vec<String>>,
    pub err: Value,
    /// `None` when compute was never measured. Never defaulted to zero: the CU
    /// panel reading 0 is not a claim, but a 0 we invented would be.
    pub cu: Option<u64>,
    /// `transaction.message`, when a real serialized transaction was decoded.
    pub message: Option<Value>,
    pub fee: Option<u64>,
    /// False when the decoded account list could not be completed, so nothing
    /// derived from account indexes may be shown.
    pub accounts_complete: bool,
    /// Lamports the fee payer held, read live off the validator. A snapshot, not
    /// the transaction's pre-balance — a simulated transaction has none.
    pub payer_live: Option<u64>,
}

pub fn detect(root: &Path) -> Option<(&'static str, Vec<String>)> {
    let owned = |args: &[&str]| args.iter().map(|arg| arg.to_string()).collect();
    if root.join("Anchor.toml").is_file() {
        return Some((
            "anchor",
            owned(&[
                "anchor",
                "test",
                "--skip-local-validator",
                "--provider.cluster",
                CLUSTER_ARG,
            ]),
        ));
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let label = if manifest.contains("mollusk") {
        //TODO
        "mollusk"
    } else {
        "cargo"
    };
    Some((label, owned(&["cargo", "test", "--", "--nocapture"])))
}

pub fn available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Combined stdout+stderr: frameworks split program logs across both.
pub fn run(root: &Path, command: &[String]) -> std::io::Result<String> {
    let output = Command::new(&command[0])
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

pub struct Validator(Arc<Mutex<Option<Child>>>);

pub const TTL: u64 = 180;

/// 100 SOL, the top-up for a provider wallet on a validator we did not start.
const TOP_UP: u64 = 100_000_000_000;

fn kill(slot: &Mutex<Option<Child>>) {
    if let Some(mut child) = slot.lock().expect("validator lock").take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Validator {
    pub fn start(root: &Path) -> std::io::Result<Option<Self>> {
        let wallet = wallet(root);
        if crate::chain::solana::healthy("localnet") {
            eprintln!("reusing the validator already serving localnet");
            if let Some(wallet) = &wallet {
                crate::chain::solana::airdrop("localnet", wallet, TOP_UP);
            }
            return Ok(Some(Validator(Arc::default())));
        }
        let mut command = Command::new("solana-test-validator");
        command
            .args(["--reset", "--quiet", "--ledger"])
            .arg(root.join("test-ledger"));
        if let Some(wallet) = &wallet {
            command.args(["--mint", wallet]);
        }
        let child = command.current_dir(root).stdout(std::process::Stdio::null()).spawn();
        let child = match child {
            Ok(child) => child,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let validator = Validator(Arc::new(Mutex::new(Some(child))));
        for _ in 0..100 {
            if crate::chain::solana::healthy("localnet") {
                return Ok(Some(validator));
            }
            let exited = validator
                .0
                .lock()
                .expect("validator lock")
                .as_mut()
                .and_then(|child| child.try_wait().ok())
                .flatten();
            if let Some(status) = exited {
                return Err(std::io::Error::other(format!("solana-test-validator exited: {status}")));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        Err(std::io::Error::other("solana-test-validator never became healthy"))
    }

    /// Kills it `secs` from now, in the background.
    pub fn expire_after(&self, secs: u64) {
        let slot = Arc::clone(&self.0);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            kill(&slot);
        });
    }

    /// Kills it now. Nothing left to fetch, so nothing to keep it up for.
    pub fn stop(&self) {
        kill(&self.0);
    }
}

impl Drop for Validator {
    fn drop(&mut self) {
        kill(&self.0);
    }
}

fn wallet(root: &Path) -> Option<String> {
    let configured = std::fs::read_to_string(root.join("Anchor.toml"))
        .ok()
        .and_then(|text| crate::workspace::parse(&text).get("provider")?.get("wallet").cloned())
        .unwrap_or_else(|| "~/.config/solana/id.json".to_string()); // anchor's own default
    let path = match configured.strip_prefix("~") {
        Some(rest) => crate::workspace::home().join(rest.trim_start_matches(['/', std::path::MAIN_SEPARATOR])),
        None => root.join(configured),
    };
    let output = Command::new("solana-keygen").arg("pubkey").arg(path).output().ok()?;
    let address = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let plausible = (32..=44).contains(&address.len())
        && address.chars().all(|c| c.is_ascii_alphanumeric())
        && crate::chain::solana::bs58_decode(&address).len() == 32;
    plausible.then_some(address)
}

pub fn failed_signatures(output: &str, cluster: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for token in output.split(|c: char| !c.is_ascii_alphanumeric()) {
        if !crate::chain::solana::is_signature(token)
            || token.len() > 90
            || found.iter().any(|seen| seen == token)
            || crate::chain::solana::bs58_decode(token).len() != 64
        {
            continue;
        }
        if crate::chain::solana::fetch(cluster, token).is_ok_and(|value| !value["result"]["meta"]["err"].is_null()) {
            found.push(token.to_string());
        } else {
            crate::chain::solana::evict(cluster, token);
        }
    }
    found
}

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

fn error_value(index: usize, message: &str) -> Value {
    let code = message
        .split_once("0x")
        .and_then(|(_, hex)| u32::from_str_radix(hex.split_whitespace().next().unwrap_or(""), 16).ok());
    match code {
        Some(code) => json!({"InstructionError": [index, {"Custom": code}]}),
        None => json!({"InstructionError": [index, message.trim()]}),
    }
}

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
                    logs: Some(logs),
                    cu: Some(u64::from(cu)),
                    message: None,
                    fee: None,
                    accounts_complete: true,
                    payer_live: None,
                },
            ));
        } else if depth == 0 {
            open = None; // the transaction succeeded, nothing to diagnose
        }
    }

    let mut cases: Vec<Case> = Vec::new();
    for (index, mut case) in done {
        case.name = nearest_name(&names, index, cases.len());
        if !cases.iter().any(|seen| seen.logs == case.logs) {
            cases.push(case);
        }
    }
    cases
}

/// Cases from the recorded RPC conversation: the validator's own account of why
/// each call failed, which the suite output no longer carries once anchor's
/// client has turned it into a thrown string.
pub fn captured(captures: &[tap::Capture]) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for capture in captures {
        let logs = capture.logs.clone();
        let err = match (&capture.err, &capture.rpc_error) {
            (Some(err), _) => err.clone(),
            // No simulation result: the call failed before execution, so the
            // JSON-RPC message is the whole of what is known.
            (None, Some(rpc)) => json!(rpc["message"].as_str().unwrap_or("rpc error")),
            (None, None) => continue,
        };
        let name = logs
            .iter()
            .flatten()
            .find_map(|line| {
                line.split_once("Program log: Instruction: ")
                    .map(|(_, name)| name.trim().to_string())
            })
            .unwrap_or_else(|| format!("failure {}", cases.len() + 1));
        // Two tests can fail the same instruction, and keys come from the name.
        let taken = cases.iter().filter(|seen| seen.name.starts_with(&name)).count();
        let decoded = capture.transaction.as_ref().and_then(|(transaction, encoding)| {
            let bytes = match encoding.as_str() {
                "base64" => crate::chain::solana::base64_decode(transaction),
                _ => crate::chain::solana::bs58_decode(transaction),
            };
            crate::chain::solana::decode_message(&bytes)
        });
        let complete = decoded.as_ref().is_none_or(|message| message.accounts_complete);
        let message = decoded.map(|message| {
            let mut json = message.json;
            if !message.accounts_complete {
                // The indexes may point at addresses the message never carried.
                json["instructions"] = json!([]);
            }
            json
        });
        cases.push(Case {
            name: match taken {
                0 => name,
                n => format!("{name} {}", n + 1),
            },
            err,
            logs,
            cu: capture.cu,
            message,
            fee: capture.fee,
            accounts_complete: complete,
            payer_live: None,
        });
    }
    cases
}

/// Reads what each case's fee payer holds, while the validator is still up. The
/// payer is whoever the decoded message says signed — not a wallet we assumed.
/// The response is also cached under our own cluster, because the validator is
/// gone by the time anyone clicks that account in the TUI.
pub fn add_payer_balance(cases: &mut [Case], cluster: &str) {
    for case in cases {
        let Some(key) = case
            .message
            .as_ref()
            .and_then(|message| message["accountKeys"][0].as_str())
            .map(str::to_string)
        else {
            continue;
        };
        let cached = format!("account-{key}");
        // That cache never expires, and last run's balance would be a lie.
        crate::chain::solana::evict(cluster, &cached);
        let Ok(value) = crate::chain::solana::fetch_account(cluster, &key) else {
            continue;
        };
        let _ = crate::chain::solana::store(CLUSTER, &cached, &value);
        case.payer_live = Some(crate::chain::solana::parse_account(&key, &value).lamports);
    }
}

fn nearest_name(names: &[(usize, String)], index: usize, nth: usize) -> String {
    names
        .iter()
        .min_by_key(|(at, _)| at.abs_diff(index))
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| format!("failure {}", nth + 1))
}

/// A `getTransaction` response carrying only what we were actually told. Absent
/// evidence is left out rather than zeroed: a 0 we invented would read as a
/// measurement, and `parse` tolerates every one of these keys being missing.
fn transaction_json(case: &Case) -> Value {
    let mut meta = json!({"err": case.err});
    if let Some(logs) = &case.logs {
        meta["logMessages"] = json!(logs);
    }
    if let Some(cu) = case.cu {
        meta["computeUnitsConsumed"] = json!(cu);
    }
    if let Some(fee) = case.fee {
        meta["fee"] = json!(fee);
    }
    let message = case.message.clone().unwrap_or_else(|| {
        json!({
            "accountKeys": [],
            "instructions": [],
            "header": {"numRequiredSignatures": 0, "numReadonlySignedAccounts": 0, "numReadonlyUnsignedAccounts": 0},
        })
    });
    // Ours, not the RPC's: a live balance snapshot and a completeness flag have no
    // place in `meta`, which is supposed to be what the chain recorded.
    let mut value = json!({"result": {"slot": 0, "meta": meta, "transaction": {"message": message}}});
    value["patp"] = json!({"payerBalance": case.payer_live, "accountsComplete": case.accounts_complete});
    value
}

fn key(name: &str) -> String {
    let key: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    key.trim_matches('-').chars().take(60).collect()
}

pub fn stash(cases: &[Case]) -> Vec<String> {
    cases
        .iter()
        .enumerate()
        .filter_map(|(index, case)| {
            let key = match key(&case.name) {
                key if key.is_empty() => format!("failure-{}", index + 1),
                key => key,
            };
            crate::chain::solana::store(CLUSTER, &key, &transaction_json(case)).ok()?;
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

    /// The deterministic part: each error class is a real recorded RPC envelope
    /// replayed through the same pipeline, with no validator and no network. A
    /// new class is a new envelope, not new code. Each row asserts only the
    /// evidence that envelope genuinely carried.
    #[test]
    fn every_error_class_diagnoses_from_its_recorded_envelope() {
        use super::tap as rpc_tap;

        let rebuild = |request: &str, response: &str| {
            let capture = rpc_tap::capture(request.as_bytes(), response.as_bytes()).expect("captured");
            let cases = super::captured(std::slice::from_ref(&capture));
            let case = cases.into_iter().next().expect("one case");
            let tx = crate::chain::solana::parse("t", super::CLUSTER, &super::transaction_json(&case));
            (case, tx)
        };

        for (response, headline, cu, instruction) in [
            (rpc_tap::SEND_ZERO_AMOUNT, "ZeroAmount", 4340, "Deposit"),
            (rpc_tap::SEND_INSUFFICIENT_FUNDS, "InsufficientFunds", 4367, "Withdraw"),
            (rpc_tap::SEND_NOT_INITIALIZED, "AccountNotInitialized", 2471, "Deposit"),
        ] {
            let (case, tx) = rebuild(rpc_tap::SEND_REQUEST, response);
            let failure = tx.failure.as_ref().expect("diagnosed");
            assert!(failure.headline.starts_with(headline), "{}", failure.headline);
            // the evidence the suite's own output had lost
            assert_eq!(tx.cu, cu, "compute units, as the validator measured them");
            assert_eq!(tx.fee, 5000);
            assert!(!tx.logs.is_empty(), "real program logs");
            assert_eq!(tx.nodes.len(), 1, "an execution tree, from those logs");
            // and the accounts, decoded from the request's own transaction
            assert_eq!(tx.accounts.len(), 3);
            assert_eq!(tx.accounts[0].key, "4AvpokDPwAV5MBwY4VahxddcDF9AHJHCDbpbUdfUpjWa");
            assert_eq!(tx.accounts[0].role(), "signer, writable");
            assert!(tx.accounts_complete);
            assert_eq!(case.name, instruction, "named after the instruction that failed");
        }

        // A failure before execution: nothing ran, so nothing is claimed to have.
        let (_, tx) = rebuild(rpc_tap::BARE_REQUEST, rpc_tap::SIG_VERIFY);
        let failure = tx.failure.as_ref().expect("diagnosed");
        assert!(
            failure.headline.contains("signature verification"),
            "{}",
            failure.headline
        );
        assert!(tx.logs.is_empty(), "no logs were reported, and none are invented");
        assert_eq!(tx.cu, 0);
        assert!(tx.accounts.is_empty(), "no transaction was decodable");
        assert!(tx.payer.is_none());
    }

    #[test]
    fn available_distinguishes_missing_from_failing() {
        assert!(super::available("cargo"));
        assert!(!super::available("panic-no-such-program"));
    }

    #[test]
    fn scrapes_failures_from_both_report_styles() {
        let cases = super::cases(MOCHA);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "deposits into the vault");
        assert_eq!(cases[0].cu, Some(4521));
        assert_eq!(cases[0].err["InstructionError"][1]["Custom"], 2006);
        assert_eq!(cases[0].logs.as_deref().map(<[String]>::len), Some(5));
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
        let tx = crate::chain::solana::parse("deposit", super::CLUSTER, &super::transaction_json(&cases[0]));
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
