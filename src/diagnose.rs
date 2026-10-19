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
    pub checks: Vec<String>,
}

pub fn failure(err: &Value, nodes: &[Node], cu: u32, cu_limit: u32) -> Option<Failure> {
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
    let (headline, meaning) = describe(&code, program);

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

    Some(Failure { instruction, node, code, headline, meaning, checks })
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
