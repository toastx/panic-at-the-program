use std::collections::HashMap;

use super::diagnose::{DiagnosticId, LogId};
use super::errors::Class;
use super::idl;
use super::solana::{
    self, ATA, AccountState, Node, SYSTEM, TOKEN, TOKEN_2022, Tx, bs58_decode, bs58_encode, program_name, u64_at,
};

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Level {
    Error,
    Warn,
}

/// What a finding found. Typed rather than keyed off the message, so the
/// diagnosis can correlate findings with error classes without matching prose.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub enum FindingKind {
    NotSigner,
    NotWritable,
    AddressMismatch,
    UnresolvedSeed,
    PdaMismatch,
    NotAta,
    BelowRent,
    AccountCount,
    Missing,
    Exists,
    WrongOwner,
    HoldsType,
    TokenMint,
    TokenOwner,
    TokenShortfall,
    Frozen,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub index: usize,
    pub role: String,
    pub key: String,
    pub kind: FindingKind,
    pub level: Level,
    pub message: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

/// What the failure says: its class, Anchor's Left/Right pair, the account it named.
#[derive(Default)]
pub struct Context {
    pub class: Option<Class>,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub account: Option<String>,
}

pub fn check(cluster: &str, tx: &Tx, index: usize) -> Vec<Finding> {
    let Some(node) = tx.nodes.get(index) else {
        return Vec::new();
    };
    let idl = idl::idl(cluster, &node.program);
    let decoded = idl
        .as_ref()
        .and_then(|idl| idl::instruction(idl, &bs58_decode(&node.data)));

    let mut findings = Vec::new();
    let mut roles = Vec::new();
    if let Some(decoded) = &decoded {
        roles = decoded.specs.iter().map(|spec| spec.name.clone()).collect();
        findings.extend(declared_findings(&decoded.specs, node));
    }

    let context = tx.failure.as_ref().map_or_else(Context::default, |failure| {
        let mismatch = failure
            .notes
            .iter()
            .find(|note| note.id == DiagnosticId::Log(LogId::ValueMismatch));
        Context {
            class: failure.class,
            expected: mismatch.and_then(|note| note.expected.clone()),
            actual: mismatch.and_then(|note| note.actual.clone()),
            account: failure.account.clone(),
        }
    });
    let keys: Vec<String> = node.accounts.iter().map(|a| a.key.clone()).collect();
    let state = solana::account_states(cluster, &keys);
    let rent = |size: usize| solana::rent_exempt_minimum(cluster, size);
    findings.extend(state_findings(node, &roles, &state, rent, &context, idl.as_deref()));
    findings
}

/// Whether a declared signer that did not sign the transaction is still
/// explainable rather than a fault.
///
/// Inside a CPI a program can sign for its own PDAs with `invoke_signed`, and
/// that privilege never reaches the transaction's signer list, which is all
/// `instruction_accounts` has to copy from. An on-curve key has no such excuse:
/// it is a real keypair that had to sign. Neither does anything at depth 0,
/// where there is no caller to sign on its behalf.
fn may_sign_via_cpi(node: &Node, key: &str) -> bool {
    node.depth > 0 && <[u8; 32]>::try_from(bs58_decode(key).as_slice()).is_ok_and(|bytes| idl::off_curve(&bytes))
}

pub fn declared_findings(specs: &[idl::AccountSpec], node: &Node) -> Vec<Finding> {
    // Short by some: the positions past the gap mean nothing, so the one
    // finding is the gap itself, naming what is probably missing. Long by some
    // is `remaining_accounts`, and the declared prefix still checks.
    if node.accounts.len() < specs.len() {
        let missing: Vec<&str> = specs[node.accounts.len()..]
            .iter()
            .map(|spec| spec.name.as_str())
            .collect();
        return vec![Finding {
            index: node.accounts.len(),
            role: String::new(),
            key: String::new(),
            kind: FindingKind::AccountCount,
            level: Level::Error,
            message: format!(
                "the IDL declares {} accounts but {} were passed — missing: {}",
                specs.len(),
                node.accounts.len(),
                missing.join(", ")
            ),
            expected: None,
            actual: None,
        }];
    }
    let mut findings = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let actual = &node.accounts[i];
        let finding = |kind, level, message: String, expected, actual_value| Finding {
            index: i,
            role: spec.name.clone(),
            key: actual.key.clone(),
            kind,
            level,
            message,
            expected,
            actual: actual_value,
        };

        if spec.signer && !actual.signer && !may_sign_via_cpi(node, &actual.key) {
            findings.push(finding(
                FindingKind::NotSigner,
                Level::Error,
                "declared as signer but did not sign".into(),
                None,
                None,
            ));
        }
        if spec.writable && !actual.writable {
            findings.push(finding(
                FindingKind::NotWritable,
                Level::Error,
                "declared writable but passed read-only".into(),
                None,
                None,
            ));
        }
        if let Some(fixed) = spec.address.as_deref()
            && fixed != actual.key
        {
            findings.push(finding(
                FindingKind::AddressMismatch,
                Level::Error,
                "address does not match the one the program requires".into(),
                Some(fixed.to_string()),
                Some(actual.key.clone()),
            ));
        }
        if let Some(unresolved) = unresolved_seed(spec) {
            findings.push(finding(
                FindingKind::UnresolvedSeed,
                Level::Warn,
                format!("PDA seed `{unresolved}` comes from an instruction argument — not verified"),
                None,
                None,
            ));
        }
        if let Some(expected) = derive_pda(spec, &node.program, specs, node)
            && expected != actual.key
        {
            findings.push(finding(
                FindingKind::PdaMismatch,
                Level::Error,
                "PDA does not match the seeds declared in the IDL".into(),
                Some(expected),
                Some(actual.key.clone()),
            ));
        }
    }
    findings
}

/// A Token `Transfer`, `Burn` or `TransferChecked` on `node`: the amount, its
/// decimals when the instruction carries them, and which slot is the authority.
fn token_spend(node: &Node) -> Option<(u64, Option<u32>, usize)> {
    if node.program != TOKEN && node.program != TOKEN_2022 {
        return None;
    }
    let bytes = bs58_decode(&node.data);
    match bytes.first()? {
        3 | 8 => Some((u64_at(&bytes, 1), None, 2)),
        12 => Some((u64_at(&bytes, 1), bytes.get(9).map(|d| u32::from(*d)), 3)),
        _ => None,
    }
}

fn amount(raw: u64, decimals: Option<u32>) -> String {
    match decimals {
        Some(d) => format!("{}", raw as f64 / 10f64.powi(d as i32)),
        None => raw.to_string(),
    }
}

pub fn state_findings(
    node: &Node,
    roles: &[String],
    state: &HashMap<String, AccountState>,
    rent: impl Fn(usize) -> Option<u64>,
    context: &Context,
    idl: Option<&idl::Program>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let spend = token_spend(node);
    for (i, account) in node.accounts.iter().enumerate() {
        let role = roles.get(i).cloned().unwrap_or_default();
        // Anchor names the account it rejected; the class checks look at it alone.
        let class = context
            .class
            .filter(|_| context.account.as_ref().is_none_or(|named| *named == role));
        let finding = |kind, level, message: String, expected, actual| Finding {
            index: i,
            role: role.clone(),
            key: account.key.clone(),
            kind,
            level,
            message,
            expected,
            actual,
        };

        let Some(info) = state.get(&account.key) else {
            if class == Some(Class::Uninitialized) {
                findings.push(finding(
                    FindingKind::Missing,
                    Level::Error,
                    "does not exist on chain — nothing has created it yet".to_string(),
                    None,
                    None,
                ));
            }
            continue;
        };

        // An `init` target is writable and never signs; the payer signs.
        if class == Some(Class::Already) && account.writable && !account.signer {
            findings.push(finding(
                FindingKind::Exists,
                Level::Error,
                format!("already exists, owned by {}", program_name(&info.owner)),
                None,
                None,
            ));
        }

        let blamed = match (&context.account, &context.actual) {
            (Some(_), _) => true,
            (None, Some(owner)) => info.owner == *owner,
            (None, None) => account.writable && info.owner != node.program,
        };
        if class == Some(Class::WrongOwner) && blamed {
            findings.push(finding(
                FindingKind::WrongOwner,
                Level::Error,
                format!("owned by {}", program_name(&info.owner)),
                context.expected.as_deref().map(program_name),
                Some(program_name(&info.owner)),
            ));
        }

        if class == Some(Class::Layout)
            && info.owner == node.program
            && let Some(idl) = idl
        {
            let message = match idl::account(idl, &info.data) {
                Some((name, _)) => format!("holds a `{name}`"),
                None => format!("holds {} bytes that match no account type in the IDL", info.data.len()),
            };
            findings.push(finding(FindingKind::HoldsType, Level::Error, message, None, None));
        }

        if (info.owner == TOKEN || info.owner == TOKEN_2022) && info.data.len() == 165 {
            let mint = bs58_encode(&info.data[0..32]);
            let owner = bs58_encode(&info.data[32..64]);
            let delegate = (u32::from_le_bytes(info.data[72..76].try_into().unwrap_or_default()) == 1)
                .then(|| bs58_encode(&info.data[76..108]));

            if role.contains("associated") {
                let ata = find_ata(&owner, &mint);
                if ata.as_deref() != Some(account.key.as_str()) {
                    findings.push(finding(
                        FindingKind::NotAta,
                        Level::Error,
                        "not the associated token account for this owner and mint".to_string(),
                        ata,
                        Some(account.key.clone()),
                    ));
                }
            }
            if class == Some(Class::Mint) && context.actual.as_ref().is_none_or(|actual| *actual == mint) {
                findings.push(finding(
                    FindingKind::TokenMint,
                    Level::Error,
                    format!("holds mint {}", program_name(&mint)),
                    context.expected.clone(),
                    Some(mint.clone()),
                ));
            }
            if info.data[108] == 2 {
                findings.push(finding(
                    FindingKind::Frozen,
                    Level::Warn,
                    "frozen — transfers and burns are blocked until the freeze authority thaws it".to_string(),
                    None,
                    None,
                ));
            }
            // A spend's source: its balance against the amount, its owner against the authority.
            if i == 0
                && let Some((moved, decimals, authority_slot)) = spend
            {
                let held = u64_at(&info.data, 64);
                if moved > held {
                    findings.push(finding(
                        FindingKind::TokenShortfall,
                        Level::Error,
                        format!(
                            "holds {} but the instruction moves {}",
                            amount(held, decimals),
                            amount(moved, decimals)
                        ),
                        Some(amount(moved, decimals)),
                        Some(amount(held, decimals)),
                    ));
                }
                if let Some(authority) = node.accounts.get(authority_slot)
                    && authority.key != owner
                    && delegate.as_deref() != Some(authority.key.as_str())
                {
                    findings.push(finding(
                        FindingKind::TokenOwner,
                        Level::Error,
                        "the authority passed is neither this account's owner nor its delegate".to_string(),
                        Some(owner.clone()),
                        Some(authority.key.clone()),
                    ));
                }
            }
        }

        if account.writable
            && info.owner != SYSTEM
            && let Some(minimum) = rent(info.data.len())
            && info.lamports < minimum
        {
            findings.push(finding(
                FindingKind::BelowRent,
                Level::Warn,
                "below the rent-exempt minimum".to_string(),
                Some(format!("{:.9} SOL", minimum as f64 / 1e9)),
                Some(format!("{:.9} SOL", info.lamports as f64 / 1e9)),
            ));
        }
    }
    findings
}

fn find_ata(owner: &str, mint: &str) -> Option<String> {
    let (address, _) = idl::find_program_address(
        &[&bs58_decode(owner), &bs58_decode(TOKEN), &bs58_decode(mint)],
        &bs58_decode(ATA),
    )?;
    Some(bs58_encode(&address))
}

fn unresolved_seed(spec: &idl::AccountSpec) -> Option<String> {
    spec.seeds.as_ref()?.iter().find_map(|seed| match seed {
        idl::Seed::Arg(name) => Some(name.clone()),
        _ => None,
    })
}

fn derive_pda(spec: &idl::AccountSpec, program: &str, specs: &[idl::AccountSpec], node: &Node) -> Option<String> {
    let resolve = |seed: &idl::Seed| match seed {
        idl::Seed::Const(bytes) => Some(bytes.clone()),
        idl::Seed::Account(path) => {
            let position = specs.iter().position(|s| s.name == *path)?;
            Some(bs58_decode(&node.accounts.get(position)?.key))
        }
        idl::Seed::Arg(_) => None,
    };
    let parts: Vec<Vec<u8>> = spec.seeds.as_ref()?.iter().map(resolve).collect::<Option<_>>()?;
    let under = match &spec.pda_program {
        Some(seed) => resolve(seed)?,
        None => bs58_decode(program),
    };
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    let (address, _) = idl::find_program_address(&refs, &under)?;
    Some(bs58_encode(&address))
}
