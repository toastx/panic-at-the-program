use serde_json::Value;

use crate::solana::{Node, Status};

#[derive(Clone, PartialEq, Debug)]
pub enum Code {
    Custom(u32),
    Named(String),
    Other(String),
}

pub struct Failure {
    pub instruction: Option<usize>,
    pub node: Option<usize>,
    pub code: Code,
    pub headline: String,
    pub meaning: String,
    pub account: Option<String>,
    pub source: Option<String>,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub checks: Vec<String>,
}

#[derive(Default)]
pub struct AnchorLog {
    pub name: Option<String>,
    pub number: Option<u32>,
    pub message: Option<String>,
    pub account: Option<String>,
    pub source: Option<String>,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

fn field(line: &str, key: &str) -> Option<String> {
    let value = &line[line.find(key)? + key.len()..];
    let end = value.find(". ").unwrap_or(value.len());
    Some(value[..end].trim().trim_end_matches('.').to_string())
}

pub fn anchor_log(logs: &[String]) -> AnchorLog {
    let mut parsed = AnchorLog::default();
    let mut lines = logs.iter().filter_map(|line| line.strip_prefix("Program log: ")).peekable();

    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("AnchorError ") {
            let head = rest.split("Error Code: ").next().unwrap_or_default();
            if let Some(account) = head.strip_prefix("caused by account: ") {
                parsed.account = Some(account.trim().trim_end_matches('.').to_string());
            }
            if let Some(source) = head.strip_prefix("thrown in ") {
                parsed.source = Some(source.trim().trim_end_matches('.').to_string());
            }
            parsed.name = field(rest, "Error Code: ").or(parsed.name);
            parsed.number = field(rest, "Error Number: ").and_then(|v| v.parse().ok()).or(parsed.number);
            parsed.message = field(rest, "Error Message: ").or(parsed.message);
            continue;
        }
        if line.starts_with("Left:") {
            parsed.actual = lines.peek().map(|next| next.trim().to_string());
            continue;
        }
        if line.starts_with("Right:") {
            parsed.expected = lines.peek().map(|next| next.trim().to_string());
            continue;
        }
        if let Some(message) = line.strip_prefix("Error: ") {
            parsed.message.get_or_insert_with(|| message.trim().to_string());
        }
    }
    parsed
}

pub fn failure(err: &Value, nodes: &[Node], logs: &[String], cu: u32, cu_limit: u32) -> Option<Failure> {
    if err.is_null() {
        return None;
    }
    let (instruction, code) = match classify(err) {
        Some(parsed) => parsed,
        None => (None, Code::Other(err.to_string())),
    };
    let node = nodes
        .iter()
        .rposition(|n| n.status == Status::Failed)
        .or_else(|| instruction.and_then(|index| top_level(nodes).get(index).copied()));
    let program = node.map(|i| nodes[i].program.as_str()).unwrap_or_default();
    let (mut headline, mut meaning) = describe(&code, program);

    let logs_slice = node.and_then(|i| logs.get(nodes[i].logs.clone())).unwrap_or(logs);
    let anchor = anchor_log(logs_slice);
    if let (Some(name), Some(number)) = (&anchor.name, anchor.number) {
        headline = format!("{name} — error {number} (0x{number:x})");
    }
    if let Some(message) = &anchor.message {
        if !message.is_empty() {
            meaning = message.clone();
        }
    }

    let mut checks = Vec::new();
    if let (Code::Named(name), Some(node)) = (&code, node) {
        if name == "MissingRequiredSignature" {
            let missing: Vec<String> = nodes[node]
                .accounts
                .iter()
                .filter(|a| !a.signer)
                .map(|a| crate::ui::short(&a.key))
                .take(4)
                .collect();
            if !missing.is_empty() {
                checks.push(format!("no signature from: {}", missing.join(", ")));
            }
        }
        if name == "ComputeBudgetExceeded" || name == "ProgramFailedToComplete" {
            checks.push(format!(
                "consumed {cu} of {cu_limit} compute units — raise the limit with ComputeBudget"
            ));
        }
    }
    if let Code::Custom(n) = code {
        if n >= 6000 {
            checks.push(format!(
                "program-defined error #{} — check the program's error enum",
                n - 6000
            ));
        } else if let Some(hint) = anchor_hint(n) {
            checks.push(hint.to_string());
        }
    }

    checks.extend(rules(&code, &headline, logs_slice, cu, cu_limit));

    if let (Some(expected), Some(actual)) = (&anchor.expected, &anchor.actual) {
        if expected != actual {
            checks.push("the value the program expected differs from the one passed".to_string());
        }
    }
    if let Some(account) = &anchor.account {
        checks.insert(0, format!("account `{account}` failed the constraint"));
    }

    Some(Failure {
        instruction,
        node,
        code,
        headline,
        meaning,
        account: anchor.account,
        source: anchor.source,
        expected: anchor.expected,
        actual: anchor.actual,
        checks,
    })
}

pub fn rules(code: &Code, headline: &str, logs: &[String], cu: u32, cu_limit: u32) -> Vec<String> {
    let mut out = Vec::new();
    let text = logs.join("\n");

    if let Code::Named(name) = code {
        match name.as_str() {
            "BlockhashNotFound" => out.push(
                "the blockhash expired before the transaction landed — fetch a fresh one and re-sign".to_string(),
            ),
            "AlreadyProcessed" => out.push(
                "this transaction already landed — reuse the earlier signature instead of resending".to_string(),
            ),
            "InsufficientFundsForFee" => {
                out.push("the fee payer cannot cover the fee — fund it before retrying".to_string())
            }
            _ => {}
        }
    }

    if text.contains("exceeded CUs")
        || text.contains("exceeded maximum number of instructions")
        || matches!(code, Code::Named(name) if name == "ComputeBudgetExceeded")
    {
        out.push(format!(
            "ran out of compute: {cu} of {cu_limit} units used — raise it with ComputeBudget::SetComputeUnitLimit"
        ));
    }

    if let Some(line) = logs.iter().find(|line| line.contains("insufficient lamports")) {
        let numbers: Vec<u64> = line
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|part| part.parse().ok())
            .collect();
        match numbers.as_slice() {
            [have, need, ..] if need > have => out.push(format!(
                "fund the account with {:.9} more SOL to reach the rent-exempt minimum",
                (need - have) as f64 / 1e9
            )),
            _ => out.push("an account is below the rent-exempt minimum — fund it first".to_string()),
        }
    }

    if text.contains("already in use") {
        out.push("the account already exists — change the seed, or use init_if_needed".to_string());
    }

    if headline.contains("Slippage") || text.contains("slippage") {
        out.push("the quote moved before the transaction landed — widen the tolerance or requote".to_string());
    }

    if matches!(code, Code::Custom(100 | 101 | 102)) {
        out.push(
            "the deployed program does not recognise this instruction — redeploy, or regenerate the client from the deployed IDL"
                .to_string(),
        );
    }

    if text.contains("access violation") || text.contains("panicked at") {
        out.push("the program panicked — check unwraps and arithmetic in that instruction".to_string());
    }

    out
}

fn top_level(nodes: &[Node]) -> Vec<usize> {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.depth == 0)
        .map(|(i, _)| i)
        .collect()
}

fn classify(err: &Value) -> Option<(Option<usize>, Code)> {
    if let Some(name) = err.as_str() {
        return Some((None, Code::Named(name.to_string())));
    }
    let (key, value) = err.as_object()?.iter().next()?;
    if key != "InstructionError" {
        return Some((None, Code::Named(key.clone())));
    }
    let parts = value.as_array()?;
    let index = parts.first().and_then(Value::as_u64).map(|i| i as usize);
    let code = match parts.get(1) {
        Some(Value::String(name)) => Code::Named(name.clone()),
        Some(Value::Object(map)) => match map.iter().next() {
            Some((name, code)) if name == "Custom" => Code::Custom(code.as_u64().unwrap_or(0) as u32),
            Some((name, detail)) => Code::Other(format!("{name}: {detail}")),
            None => Code::Other("unknown".to_string()),
        },
        other => Code::Other(other.map(ToString::to_string).unwrap_or_default()),
    };
    Some((index, code))
}

fn describe(code: &Code, program: &str) -> (String, String) {
    match code {
        Code::Custom(n) => {
            if let Some((name, meaning)) = program_error(program, *n) {
                return (
                    format!("{name} — {program_label} error {n} (0x{n:x})", program_label = short_program(program)),
                    meaning.to_string(),
                );
            }
            if *n >= 6000 {
                (
                    format!("Anchor user error #{} — code {n} (0x{n:x})", n - 6000),
                    "defined by the program itself; the IDL carries its name and message".to_string(),
                )
            } else if let Some((name, meaning)) = anchor_error(*n) {
                (format!("{name} — Anchor error {n} (0x{n:x})"), meaning.to_string())
            } else {
                (
                    format!("custom program error {n} (0x{n:x})"),
                    "not a known Anchor or SPL code".to_string(),
                )
            }
        }
        Code::Named(name) => (name.clone(), runtime_error(name).to_string()),
        Code::Other(detail) => (detail.clone(), String::new()),
    }
}

fn short_program(program: &str) -> &'static str {
    match program {
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" | "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => "Token program",
        "11111111111111111111111111111111" => "System program",
        _ => "program",
    }
}

fn program_error(program: &str, code: u32) -> Option<(&'static str, &'static str)> {
    match program {
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" | "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => {
            Some(match code {
                0 => ("NotRentExempt", "the account holds less than the rent-exempt minimum"),
                1 => ("InsufficientFunds", "the source token account does not hold enough tokens"),
                2 => ("InvalidMint", "the mint account is not a valid mint"),
                3 => ("MintMismatch", "the token account belongs to a different mint"),
                4 => ("OwnerMismatch", "the signer is not the owner or delegate of the token account"),
                5 => ("FixedSupply", "the mint has a fixed supply and cannot mint more"),
                6 => ("AlreadyInUse", "the account is already initialized"),
                9 => ("UninitializedState", "the account has not been initialized"),
                10 => ("NativeNotSupported", "this instruction does not accept native SOL accounts"),
                11 => ("NonNativeHasBalance", "the account still holds tokens and cannot be closed"),
                13 => ("InvalidState", "the account is in an invalid state for this instruction"),
                14 => ("Overflow", "the operation overflowed"),
                17 => ("AccountFrozen", "the token account is frozen"),
                18 => ("MintDecimalsMismatch", "the decimals passed do not match the mint"),
                _ => return None,
            })
        }
        "11111111111111111111111111111111" => Some(match code {
            0 => ("AccountAlreadyInUse", "the account already exists — use a different seed or skip creation"),
            1 => ("ResultWithNegativeLamports", "the transfer would leave a negative balance"),
            3 => ("InvalidAccountDataLength", "the requested data length does not match"),
            4 => ("MaxSeedLengthExceeded", "a seed exceeds 32 bytes"),
            5 => ("AddressWithSeedMismatch", "the derived address does not match the account passed"),
            _ => return None,
        }),
        _ => None,
    }
}

fn anchor_error(code: u32) -> Option<(&'static str, &'static str)> {
    Some(match code {
        100 => ("InstructionMissing", "the instruction discriminator did not match any handler"),
        101 => ("InstructionFallbackNotFound", "no fallback handler for this instruction"),
        102 => ("InstructionDidNotDeserialize", "the instruction arguments failed to deserialize"),
        2000 => ("ConstraintMut", "an account marked mut was passed as read-only"),
        2001 => ("ConstraintHasOne", "a has_one constraint failed — a stored key does not match"),
        2002 => ("ConstraintSigner", "an account required to sign did not sign"),
        2003 => ("ConstraintRaw", "a raw constraint expression evaluated to false"),
        2004 => ("ConstraintOwner", "the account is owned by the wrong program"),
        2005 => ("ConstraintRentExempt", "the account is not rent exempt"),
        2006 => ("ConstraintSeeds", "the account address does not match the derived PDA"),
        2012 => ("ConstraintAddress", "the account address does not match the required address"),
        2014 => ("ConstraintTokenMint", "the token account's mint does not match"),
        2015 => ("ConstraintTokenOwner", "the token account's owner does not match"),
        2019 => ("ConstraintSpace", "the allocated space does not match the declared space"),
        3001 => ("AccountDiscriminatorNotFound", "the account has no discriminator — it is uninitialized"),
        3002 => ("AccountDiscriminatorMismatch", "the account is a different type than expected"),
        3003 => ("AccountDidNotDeserialize", "the account data failed to deserialize"),
        3005 => ("AccountNotEnoughKeys", "fewer accounts were passed than the instruction requires"),
        3006 => ("AccountNotMutable", "the account must be writable"),
        3007 => ("AccountOwnedByWrongProgram", "the account is owned by another program"),
        3010 => ("AccountNotSigner", "the account must be a signer"),
        3011 => ("AccountNotSystemOwned", "the account must be owned by the System program"),
        3012 => ("AccountNotInitialized", "the account does not exist yet — initialize it first"),
        3014 => ("AccountNotAssociatedTokenAccount", "the account is not the associated token account"),
        4100 => ("DeclaredProgramIdMismatch", "declare_id! does not match the deployed program"),
        _ => return None,
    })
}

fn anchor_hint(code: u32) -> Option<&'static str> {
    Some(match code {
        2001 => "compare the key stored on the account with the account you passed",
        2006 => "re-derive the PDA: same seeds, same bump, same program id",
        2004 | 3007 => "check which program owns the account you passed",
        3012 | 3001 => "the account is uninitialized — run the init instruction first",
        2002 | 3010 => "add the missing signer to the transaction",
        2014 | 2015 => "the token account's mint or owner does not match the instruction",
        _ => return None,
    })
}

pub fn infer_custom(code: u32, flavor: Option<crate::idl::Flavor>) -> (String, String, String) {
    use crate::idl::Flavor;

    let hex = format!("error {code} (0x{code:x})");
    match flavor {
        Some(Flavor::Anchor | Flavor::AnchorLegacy) if code >= 6000 => (
            format!("Anchor user error #{} — {hex}", code - 6000),
            "the deployed program raised a code its IDL does not list".to_string(),
            "the on-chain IDL may be older than the deployed program — redeploy or refresh it".to_string(),
        ),
        Some(Flavor::Anchor | Flavor::AnchorLegacy) => match anchor_error(code) {
            Some((name, meaning)) => (
                format!("{name} — Anchor {hex}"),
                meaning.to_string(),
                "raised by the Anchor framework, not the program's own error enum".to_string(),
            ),
            None => (
                format!("Anchor program error — {hex}"),
                "below Anchor's 6000 user range, so it came from the framework".to_string(),
                "check the account constraints on the failing instruction".to_string(),
            ),
        },
        Some(flavor @ (Flavor::Shank | Flavor::Codama)) => (
            format!("program error #{code} — {hex}"),
            format!("{} programs number their errors from 0", flavor.label()),
            format!("error {code} is the {} entry of the program's error enum", ordinal(code + 1)),
        ),
        None if code >= 6000 => (
            format!("Anchor user error #{} — {hex}", code - 6000),
            "the 6000 range is Anchor's user-defined block".to_string(),
            "no IDL found for this program — drop one in target/idl or point at it from patp.toml".to_string(),
        ),
        None => (
            format!("custom program error {code} (0x{code:x})"),
            "no IDL and no known error table for this program".to_string(),
            "programs outside Anchor usually number errors from 0, so this is likely their enum index"
                .to_string(),
        ),
    }
}

fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn runtime_error(name: &str) -> &'static str {
    match name {
        "MissingRequiredSignature" => "an account that must sign did not sign",
        "ProgramFailedToComplete" => "the program aborted — panic, unwrap, or compute exhaustion",
        "ComputeBudgetExceeded" => "the instruction ran out of compute units",
        "AccountNotRentExempt" => "an account was left below the rent-exempt minimum",
        "InsufficientFunds" => "an account does not hold enough lamports",
        "InvalidAccountData" => "the program rejected an account's data",
        "AccountDataTooSmall" => "the account's data is smaller than required",
        "InvalidArgument" => "the runtime rejected an argument",
        "AccountBorrowFailed" => "the program borrowed the same account twice",
        "PrivilegeEscalation" => "a CPI asked for more privileges than the caller had",
        "ProgramEnvironmentSetupFailure" => "the program could not be loaded",
        "InsufficientFundsForFee" => "the fee payer cannot cover the transaction fee",
        "BlockhashNotFound" => "the blockhash expired before the transaction landed",
        "AlreadyProcessed" => "this exact transaction was already processed",
        _ => "",
    }
}
