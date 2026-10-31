use serde_json::Value;

use super::solana::{Node, Status};
use super::validate::{Finding, FindingKind, Level, SYSTEM, TOKEN};

/// Identity of one diagnostic, used for deduplication and ordering.
///
/// Carries enough context that two genuinely different diagnostics never
/// collide: custom codes are program-scoped because they index each program's
/// own error enum, and findings carry their instruction and account slot
/// because one transaction can have two missing signers.
///
/// This is identity, not presentation. [`Display`] is deliberately lossy — it
/// renders a program's base58 key as `custom` rather than printing 44
/// characters into a headline.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum DiagnosticId {
    Anchor(u32),
    Program {
        program: String,
        code: u32,
    },
    Runtime(String),
    Log(&'static str),
    Finding {
        kind: FindingKind,
        instruction: usize,
        account: Option<usize>,
    },
    Other,
}

impl std::fmt::Display for DiagnosticId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anchor(code) => write!(f, "anchor.{code}"),
            Self::Program { program, code } => match short_program(program) {
                Some(label) => write!(f, "{label}.{code}"),
                None => write!(f, "custom.{code}"),
            },
            Self::Runtime(name) => write!(f, "runtime.{name}"),
            Self::Log(id) => write!(f, "{id}"),
            Self::Finding { kind, .. } => write!(f, "fix.{kind:?}"),
            Self::Other => write!(f, "unknown"),
        }
    }
}

/// Where a note sits in the rendered diagnostic. Declaration order is render
/// order, the way rustc prints `-->`, then `note:`, then `help:`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    Span,
    Note,
    Help,
    Fix,
}

/// How much the user should trust a note, mirroring rustc's `Applicability`:
/// `Certain` came from a table or from on-chain evidence, `Likely` from
/// inferring a fix out of an error's name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confidence {
    Certain,
    Likely,
}

#[derive(Clone, Debug)]
pub struct Note {
    pub id: DiagnosticId,
    pub tier: Tier,
    pub confidence: Confidence,
    pub text: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

impl Note {
    pub fn new(id: DiagnosticId, tier: Tier, text: String) -> Self {
        Self {
            id,
            tier,
            confidence: Confidence::Certain,
            text,
            expected: None,
            actual: None,
        }
    }

    /// A note whose advice was inferred rather than looked up.
    pub fn maybe(id: DiagnosticId, tier: Tier, text: String) -> Self {
        Self {
            confidence: Confidence::Likely,
            ..Self::new(id, tier, text)
        }
    }

    pub fn values(mut self, expected: Option<String>, actual: Option<String>) -> Self {
        self.expected = expected;
        self.actual = actual;
        self
    }
}

/// Dedupe by identity, then order for presentation.
///
/// Dedupe has to happen before the presentation sort: sorting by tier first
/// would leave notes that share an id non-adjacent, and `dedup_by` only removes
/// neighbours. Within one id the highest tier wins, so a promoted note survives
/// its unpromoted twin.
///
/// Idempotent, so calling it at each collection boundary is safe.
pub fn finalize(notes: &mut Vec<Note>) {
    notes.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| b.tier.cmp(&a.tier)));
    notes.dedup_by(|a, b| a.id == b.id);
    notes.sort_by(|a, b| a.tier.cmp(&b.tier).then_with(|| a.id.cmp(&b.id)));
}

/// The wire form of a transaction error, as the RPC reported it. Distinct from
/// [`DiagnosticId`]: this is parsed before the failing program is known, so it
/// cannot carry the program scope that identity needs.
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
    pub id: DiagnosticId,
    /// The error's bare name, e.g. `ConstraintSeeds`.
    pub headline: String,
    pub meaning: String,
    pub class: Option<Class>,
    pub account: Option<String>,
    pub source: Option<String>,
    pub notes: Vec<Note>,
}

impl Failure {
    /// The one-line rustc-style label, e.g. `error[anchor.2006]: ConstraintSeeds`.
    pub fn label(&self) -> String {
        format!("error[{}]: {}", self.id, self.headline)
    }
}

// ---------------------------------------------------------------- classes

/// Declares the semantic classes in one place: the enum, its vocabulary, its
/// advice, the iteration list, and the name parse used by the Anchor table.
///
/// A hand-written `ALL` can fall out of step with the enum in a way exhaustive
/// `match` cannot catch, and the TSV's class keyword would be a third place to
/// typo. Generating all of them from one declaration removes both risks.
macro_rules! classes {
    ($($variant:ident [$($token:literal),+] $help:literal;)+) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
        pub enum Class { $($variant),+ }

        impl Class {
            pub const ALL: &'static [Class] = &[$(Class::$variant),+];

            /// The vocabulary that identifies this class in an error name.
            pub fn tokens(self) -> &'static [&'static str] {
                match self { $(Class::$variant => &[$($token),+]),+ }
            }

            /// What to do about an error of this class.
            pub fn help(self) -> &'static str {
                match self { $(Class::$variant => $help),+ }
            }

            pub fn from_name(name: &str) -> Option<Class> {
                match name { $(stringify!($variant) => Some(Class::$variant),)+ _ => None }
            }
        }
    };
}

classes! {
    Address ["address"] "the account address does not match the one the program requires";
    Already ["already", "exists", "duplicate"] "it already exists — change the seed, or use init_if_needed";
    Ata ["associated", "ata"] "this is not the associated token account for that owner and mint";
    Decimals ["decimals"] "the decimals passed do not match the mint";
    Expired ["expired", "stale", "deadline", "timeout"] "past its validity window — refresh it and resend";
    Instruction ["instruction", "discriminator", "fallback"] "the deployed program does not recognise this instruction — redeploy, or regenerate the client from the deployed IDL";
    Insufficient ["insufficient", "funds", "liquidity"] "the source does not hold enough — check the amount against the live balance";
    Mint ["mint"] "the token account's mint does not match the one the instruction expects";
    NotSigner ["signer", "sign", "signature"] "add the missing signer to the transaction";
    NotWritable ["writable", "mutable", "readonly"] "the account must be passed as writable";
    Overflow ["overflow", "underflow"] "the arithmetic wrapped — check the amount and the decimals";
    Paused ["paused", "halted", "disabled", "frozen"] "administratively disabled — nothing to fix client-side";
    Rent ["rent", "exempt"] "the account is below the rent-exempt minimum — fund it first";
    Seeds ["seed", "seeds", "pda", "bump", "derived"] "re-derive the PDA: same seeds, same bump, same program id";
    Slippage ["slippage", "price", "quote"] "the quote moved before the transaction landed — widen the tolerance or requote";
    Unauthorized ["unauthorized", "authority", "admin", "permission"] "the signer is not the authority the program stores for this account";
    Uninitialized ["uninitialized", "init"] "the account does not exist yet — run the init instruction first";
    WrongOwner ["owner", "ownership"] "check which program owns the account you passed";
}

/// Shortest token overlap we treat as a prefix match. Below this, fragments
/// like `mut` collide with unrelated words.
const MIN_PREFIX: usize = 4;

/// Splits an error name into its distinct lowercase words.
///
/// Handles `camelCase`, `snake_case`, digit runs, and acronyms, so
/// `InvalidPDAOwner` yields `invalid`, `pda`, `owner`. Sorted and deduplicated:
/// each distinct word must score once, or a vocabulary with overlapping entries
/// would count one word twice.
fn tokenize(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_alphanumeric() {
            i += 1;
            continue;
        }
        let end = word_end(&chars, i);
        tokens.push(chars[i..end].iter().collect::<String>().to_lowercase());
        i = end;
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

/// How far a run of characters matching `is_part` extends from `from`.
fn run(chars: &[char], from: usize, is_part: impl Fn(char) -> bool) -> usize {
    from + chars[from..].iter().take_while(|c| is_part(**c)).count()
}

/// Where the word starting at `start` ends: a digit run, a lowercase run, a
/// capitalised word, or an acronym run.
fn word_end(chars: &[char], start: usize) -> usize {
    if chars[start].is_ascii_digit() {
        return run(chars, start, |c| c.is_ascii_digit());
    }
    if !chars[start].is_uppercase() {
        return run(chars, start, char::is_lowercase);
    }
    if chars.get(start + 1).is_some_and(|c| c.is_lowercase()) {
        return run(chars, start + 1, char::is_lowercase);
    }
    acronym_end(chars, start + 1)
}

/// Where an acronym run ends: the last capital before a capitalised word, since
/// that capital starts the next word rather than ending this one.
fn acronym_end(chars: &[char], from: usize) -> usize {
    let mut i = from;
    while chars.get(i).is_some_and(|c| c.is_uppercase()) && !chars.get(i + 1).is_some_and(|c| c.is_lowercase()) {
        i += 1;
    }
    i
}

/// Scores a class against a name's words: 2 for an exact match, 1 for a common
/// prefix. Best match per word, so overlapping vocabulary entries such as
/// `init` and `uninitialized` cannot both count for one word.
fn score(tokens: &[String], class: Class) -> u32 {
    tokens
        .iter()
        .map(|token| {
            class
                .tokens()
                .iter()
                .map(|entry| {
                    if token == entry {
                        2
                    } else if token.len() >= MIN_PREFIX
                        && entry.len() >= MIN_PREFIX
                        && (token.starts_with(entry) || entry.starts_with(token.as_str()))
                    {
                        1
                    } else {
                        0
                    }
                })
                .max()
                .unwrap_or(0)
        })
        .sum()
}

/// The class whose vocabulary best fits an error name.
///
/// None when nothing matches, and also when two classes tie: a tie means the
/// name is ambiguous, and staying silent beats picking arbitrarily between two
/// different fixes.
pub fn classify_name(name: &str) -> Option<Class> {
    let tokens = tokenize(name);
    let mut best: Option<(u32, Class)> = None;
    let mut tied = false;
    for &class in Class::ALL {
        let scored = score(&tokens, class);
        if scored == 0 {
            continue;
        }
        match best {
            Some((top, _)) if scored > top => {
                best = Some((scored, class));
                tied = false;
            }
            Some((top, _)) if scored == top => tied = true,
            Some(_) => {}
            None => best = Some((scored, class)),
        }
    }
    match best {
        Some((_, class)) if !tied => Some(class),
        _ => None,
    }
}

// ---------------------------------------------------------------- tables

/// One error code from a table: what it is called, what it means, and the
/// class whose advice applies.
pub struct Known {
    pub code: u32,
    pub name: &'static str,
    pub message: &'static str,
    pub class: Option<Class>,
}

/// Anchor's own error enum, regenerated by `scripts/anchor_errors.py`.
const ANCHOR_TSV: &str = include_str!("anchor_errors.tsv");

/// Parsed rows of [`ANCHOR_TSV`]. Public so tests can assert the file is whole.
pub fn anchor_rows() -> impl Iterator<Item = Known> {
    ANCHOR_TSV
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .filter_map(|line| {
            let mut col = line.split('\t');
            let code = col.next()?.parse().ok()?;
            let name = col.next()?;
            let message = col.next()?;
            let class = col.next().filter(|c| !c.is_empty()).and_then(Class::from_name);
            Some(Known {
                code,
                name,
                message,
                class,
            })
        })
}

fn anchor_error(code: u32) -> Option<Known> {
    anchor_rows().find(|known| known.code == code)
}

/// Advice for codes whose class is too generic, or which have no class at all.
fn code_help(code: u32) -> Option<&'static str> {
    Some(match code {
        2001 => "compare the key stored on the account with the account you passed",
        2003 => "the program's own `constraint = ..` expression rejected this input",
        2013 => "the account must be zeroed before this instruction can initialise it",
        2019 => "the `space = ..` on the account does not match what the type needs",
        3002 => "the account is a different type than expected — check you passed the right one",
        3005 => "fewer accounts were passed than the instruction requires",
        4100 => "declare_id! does not match the deployed program — redeploy or fix the id",
        4101 => "the payer cannot also be the account being initialised",
        _ => return None,
    })
}

/// Token-2022, which shares the original token program's error numbering.
const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

fn short_program(program: &str) -> Option<&'static str> {
    match program {
        TOKEN | TOKEN_2022 => Some("token"),
        SYSTEM => Some("system"),
        _ => None,
    }
}

const TOKEN_ERRORS: &[Known] = &[
    Known {
        code: 0,
        name: "NotRentExempt",
        message: "the account holds less than the rent-exempt minimum",
        class: Some(Class::Rent),
    },
    Known {
        code: 1,
        name: "InsufficientFunds",
        message: "the source token account does not hold enough tokens",
        class: Some(Class::Insufficient),
    },
    Known {
        code: 2,
        name: "InvalidMint",
        message: "the mint account is not a valid mint",
        class: Some(Class::Mint),
    },
    Known {
        code: 3,
        name: "MintMismatch",
        message: "the token account belongs to a different mint",
        class: Some(Class::Mint),
    },
    Known {
        code: 4,
        name: "OwnerMismatch",
        message: "the signer is not the owner or delegate of the token account",
        class: None,
    },
    Known {
        code: 5,
        name: "FixedSupply",
        message: "the mint has a fixed supply and cannot mint more",
        class: None,
    },
    Known {
        code: 6,
        name: "AlreadyInUse",
        message: "the account is already initialized",
        class: Some(Class::Already),
    },
    Known {
        code: 9,
        name: "UninitializedState",
        message: "the account has not been initialized",
        class: Some(Class::Uninitialized),
    },
    Known {
        code: 10,
        name: "NativeNotSupported",
        message: "this instruction does not accept native SOL accounts",
        class: None,
    },
    Known {
        code: 11,
        name: "NonNativeHasBalance",
        message: "the account still holds tokens and cannot be closed",
        class: None,
    },
    Known {
        code: 13,
        name: "InvalidState",
        message: "the account is in an invalid state for this instruction",
        class: None,
    },
    Known {
        code: 14,
        name: "Overflow",
        message: "the operation overflowed",
        class: Some(Class::Overflow),
    },
    Known {
        code: 17,
        name: "AccountFrozen",
        message: "the token account is frozen",
        class: Some(Class::Paused),
    },
    Known {
        code: 18,
        name: "MintDecimalsMismatch",
        message: "the decimals passed do not match the mint",
        class: Some(Class::Decimals),
    },
];

const SYSTEM_ERRORS: &[Known] = &[
    Known {
        code: 0,
        name: "AccountAlreadyInUse",
        message: "the account already exists — use a different seed or skip creation",
        class: Some(Class::Already),
    },
    Known {
        code: 1,
        name: "ResultWithNegativeLamports",
        message: "the transfer would leave a negative balance",
        class: Some(Class::Insufficient),
    },
    Known {
        code: 3,
        name: "InvalidAccountDataLength",
        message: "the requested data length does not match",
        class: None,
    },
    Known {
        code: 4,
        name: "MaxSeedLengthExceeded",
        message: "a seed exceeds 32 bytes",
        class: Some(Class::Seeds),
    },
    Known {
        code: 5,
        name: "AddressWithSeedMismatch",
        message: "the derived address does not match the account passed",
        class: Some(Class::Seeds),
    },
];

fn program_error(program: &str, code: u32) -> Option<Known> {
    let table = match program {
        TOKEN | TOKEN_2022 => TOKEN_ERRORS,
        SYSTEM => SYSTEM_ERRORS,
        _ => return None,
    };
    table
        .iter()
        .find(|known| known.code == code)
        .map(|known| Known { ..*known })
}

/// Runtime errors the validator reports by name rather than by code: name,
/// message, class, and advice where the class's generic help would not fit.
const RUNTIME_ERRORS: &[(&str, &str, Option<Class>, &str)] = &[
    (
        "MissingRequiredSignature",
        "an account that must sign did not sign",
        Some(Class::NotSigner),
        "",
    ),
    (
        "ProgramFailedToComplete",
        "the program aborted — panic, unwrap, or compute exhaustion",
        None,
        "",
    ),
    (
        "ComputeBudgetExceeded",
        "the instruction ran out of compute units",
        None,
        "",
    ),
    (
        "AccountNotRentExempt",
        "an account was left below the rent-exempt minimum",
        Some(Class::Rent),
        "",
    ),
    (
        "InsufficientFunds",
        "an account does not hold enough lamports",
        Some(Class::Insufficient),
        "",
    ),
    ("InvalidAccountData", "the program rejected an account's data", None, ""),
    (
        "AccountDataTooSmall",
        "the account's data is smaller than required",
        None,
        "",
    ),
    ("InvalidArgument", "the runtime rejected an argument", None, ""),
    (
        "AccountBorrowFailed",
        "the program borrowed the same account twice",
        None,
        "",
    ),
    (
        "PrivilegeEscalation",
        "a CPI asked for more privileges than the caller had",
        None,
        "",
    ),
    (
        "ProgramEnvironmentSetupFailure",
        "the program could not be loaded",
        None,
        "",
    ),
    (
        "InsufficientFundsForFee",
        "the fee payer cannot cover the transaction fee",
        Some(Class::Insufficient),
        "the fee payer cannot cover the fee — fund it before retrying",
    ),
    (
        "BlockhashNotFound",
        "the blockhash expired before the transaction landed",
        Some(Class::Expired),
        "the blockhash expired before the transaction landed — fetch a fresh one and re-sign",
    ),
    (
        "AlreadyProcessed",
        "this exact transaction was already processed",
        Some(Class::Already),
        "this transaction already landed — reuse the earlier signature instead of resending",
    ),
];

fn runtime_error(name: &str) -> Option<(&'static str, Option<Class>, &'static str)> {
    RUNTIME_ERRORS
        .iter()
        .find(|(key, ..)| *key == name)
        .map(|&(_, message, class, help)| (message, class, help))
}

// ---------------------------------------------------------------- anchor logs

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
    let mut lines = logs
        .iter()
        .filter_map(|line| line.strip_prefix("Program log: "))
        .peekable();

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
            parsed.number = field(rest, "Error Number: ")
                .and_then(|v| v.parse().ok())
                .or(parsed.number);
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

// ---------------------------------------------------------------- diagnosis

/// The node that failed: the last one the runtime marked failed, or failing
/// that, the top-level instruction the error index points at.
fn failing_node(nodes: &[Node], instruction: Option<usize>) -> Option<usize> {
    nodes.iter().rposition(|n| n.status == Status::Failed).or_else(|| {
        let index = instruction?;
        nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.depth == 0)
            .nth(index)
            .map(|(i, _)| i)
    })
}

/// The accounts on a node that did not sign, as a readable list.
fn unsigned_accounts(node: &Node) -> Option<String> {
    let missing: Vec<String> = node
        .accounts
        .iter()
        .filter(|a| !a.signer)
        .map(|a| crate::app::widgets::short(&a.key))
        .take(4)
        .collect();
    (!missing.is_empty()).then(|| missing.join(", "))
}

/// Notes that only Anchor's own log output can supply.
fn anchor_notes(anchor: &AnchorLog) -> Vec<Note> {
    let mut out = Vec::new();
    if let (Some(expected), Some(actual)) = (&anchor.expected, &anchor.actual)
        && expected != actual
    {
        out.push(Note {
            expected: Some(expected.clone()),
            actual: Some(actual.clone()),
            ..Note::new(
                DiagnosticId::Log("value.mismatch"),
                Tier::Fix,
                "the value the program expected differs from the one passed".to_string(),
            )
        });
    }
    if let Some(account) = &anchor.account {
        out.push(Note::new(
            DiagnosticId::Log("account.constraint"),
            Tier::Span,
            format!("account `{account}` failed the constraint"),
        ));
    }
    out
}

pub fn failure(err: &Value, nodes: &[Node], logs: &[String], cu: u32, cu_limit: u32) -> Option<Failure> {
    if err.is_null() {
        return None;
    }
    let (instruction, code) = classify(err).unwrap_or((None, Code::Other(err.to_string())));
    let node = failing_node(nodes, instruction);
    let program = node.map(|i| nodes[i].program.as_str()).unwrap_or_default();

    let (id, mut headline, mut meaning, class, help) = describe(&code, program);
    let mut notes = Vec::new();

    // The program's own words beat our table when it logged them.
    let logs_slice = node.and_then(|i| logs.get(nodes[i].logs.clone())).unwrap_or(logs);
    let anchor = anchor_log(logs_slice);
    // A name is only trustworthy alongside a number: that pair means a real
    // AnchorError line rather than a stray log.
    if anchor.number.is_some()
        && let Some(name) = &anchor.name
    {
        headline = name.clone();
    }
    if let Some(message) = &anchor.message
        && !message.is_empty()
    {
        meaning = message.clone();
    }

    // Code-specific advice wins; a class's help is the generic fallback.
    if let Some(advice) = Some(help).filter(|h| !h.is_empty()).or_else(|| class.map(Class::help)) {
        notes.push(Note::new(id.clone(), Tier::Help, advice.to_string()));
    }
    // Dropped by idl::enrich once the IDL names the code.
    if let Code::Custom(n) = code
        && n >= ANCHOR_USER_OFFSET
    {
        notes.push(Note::new(
            DiagnosticId::Log("anchor.user_range"),
            Tier::Note,
            format!(
                "program-defined error #{} — check the program's error enum",
                n - ANCHOR_USER_OFFSET
            ),
        ));
    }
    // Superseded by the IDL's declared signer roles, when there are any.
    if let (Code::Named(name), Some(index)) = (&code, node)
        && name == "MissingRequiredSignature"
        && let Some(missing) = unsigned_accounts(&nodes[index])
    {
        notes.push(Note::new(
            DiagnosticId::Log("signature.missing"),
            Tier::Note,
            format!("no signature from: {missing}"),
        ));
    }
    notes.extend(log_notes(&code, logs_slice, cu, cu_limit));
    notes.extend(anchor_notes(&anchor));

    finalize(&mut notes);
    Some(Failure {
        instruction,
        node,
        code,
        id,
        headline,
        meaning,
        class,
        account: anchor.account,
        source: anchor.source,
        notes,
    })
}

/// Anchor reserves codes below this for the framework; at and above it are the
/// program's own `#[error_code]` variants.
pub const ANCHOR_USER_OFFSET: u32 = 6000;

/// Notes that can only be read out of the program's logs.
pub fn log_notes(code: &Code, logs: &[String], cu: u32, cu_limit: u32) -> Vec<Note> {
    let mut out = Vec::new();
    let text = logs.join("\n").to_lowercase();
    let note = |id, text: String| Note::new(DiagnosticId::Log(id), Tier::Help, text);

    if text.contains("exceeded cus")
        || text.contains("exceeded maximum number of instructions")
        || matches!(code, Code::Named(name) if name == "ComputeBudgetExceeded")
    {
        out.push(note(
            "cu.exceeded",
            format!(
                "ran out of compute: {cu} of {cu_limit} units used — raise it with ComputeBudget::SetComputeUnitLimit"
            ),
        ));
    }

    if let Some(line) = logs.iter().find(|line| line.contains("insufficient lamports")) {
        let numbers: Vec<u64> = line
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|part| part.parse().ok())
            .collect();
        out.push(match numbers.as_slice() {
            [have, need, ..] if need > have => note(
                "rent.shortfall",
                format!(
                    "fund the account with {:.9} more SOL to reach the rent-exempt minimum",
                    (need - have) as f64 / 1e9
                ),
            ),
            _ => note(
                "rent.shortfall",
                "an account is below the rent-exempt minimum — fund it first".to_string(),
            ),
        });
    }

    if text.contains("already in use") {
        out.push(note(
            "account.in_use",
            "the account already exists — change the seed, or use init_if_needed".to_string(),
        ));
    }

    if text.contains("slippage") {
        out.push(note(
            "slippage",
            "the quote moved before the transaction landed — widen the tolerance or requote".to_string(),
        ));
    }

    if text.contains("access violation") || text.contains("panicked at") {
        out.push(note(
            "program.panic",
            "the program panicked — check unwraps and arithmetic in that instruction".to_string(),
        ));
    }

    out
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

/// Resolves a wire code into its identity, name, meaning, and semantic class.
fn describe(code: &Code, program: &str) -> (DiagnosticId, String, String, Option<Class>, &'static str) {
    match code {
        Code::Custom(n) => {
            let scoped = DiagnosticId::Program {
                program: program.to_string(),
                code: *n,
            };
            if let Some(known) = program_error(program, *n) {
                return (
                    scoped,
                    known.name.to_string(),
                    known.message.to_string(),
                    known.class,
                    "",
                );
            }
            if *n >= ANCHOR_USER_OFFSET {
                return (
                    scoped,
                    format!("Anchor user error #{}", n - ANCHOR_USER_OFFSET),
                    "defined by the program itself; the IDL carries its name and message".to_string(),
                    None,
                    "",
                );
            }
            match anchor_error(*n) {
                Some(known) => (
                    DiagnosticId::Anchor(*n),
                    known.name.to_string(),
                    known.message.to_string(),
                    known.class,
                    code_help(*n).unwrap_or_default(),
                ),
                None => (
                    scoped,
                    format!("custom program error {n}"),
                    "not a known Anchor or SPL code".to_string(),
                    None,
                    "",
                ),
            }
        }
        Code::Named(name) => {
            let (message, class, help) = runtime_error(name).unwrap_or(("", None, ""));
            (
                DiagnosticId::Runtime(name.clone()),
                name.clone(),
                message.to_string(),
                class,
                help,
            )
        }
        Code::Other(detail) => (DiagnosticId::Other, detail.clone(), String::new(), None, ""),
    }
}

// ---------------------------------------------------------------- evidence

/// Whether an on-chain finding confirms a class's diagnosis.
///
/// Exhaustive on [`Class`] on purpose: a new class has to declare what, if
/// anything, corroborates it. Only semantically tight pairs are listed — an
/// address mismatch does not imply prior existence, and an ATA mismatch does
/// not imply a wrong mint, so promoting those would state a guess as a fix.
fn corroborates(class: Class, kind: FindingKind) -> bool {
    match class {
        Class::Seeds => kind == FindingKind::PdaMismatch,
        Class::NotSigner => kind == FindingKind::NotSigner,
        Class::NotWritable => kind == FindingKind::NotWritable,
        Class::Rent => kind == FindingKind::BelowRent,
        Class::Address => kind == FindingKind::AddressMismatch,
        Class::Ata => kind == FindingKind::NotAta,
        Class::Already
        | Class::Decimals
        | Class::Expired
        | Class::Instruction
        | Class::Insufficient
        | Class::Mint
        | Class::Overflow
        | Class::Paused
        | Class::Slippage
        | Class::Unauthorized
        | Class::Uninitialized
        | Class::WrongOwner => false,
    }
}

/// Folds account validation into the diagnosis.
///
/// A finding that corroborates the failure's class is the concrete fix, so it
/// is promoted to `Certain` even when the advice was inferred from a name.
/// Everything else still shows, one tier down.
pub fn add_findings(failure: &mut Failure, instruction: usize, findings: &[Finding]) {
    let corroborated = failure
        .class
        .is_some_and(|class| findings.iter().any(|f| corroborates(class, f.kind)));

    for finding in findings {
        let hit = failure.class.is_some_and(|class| corroborates(class, finding.kind));
        let tier = match (hit, finding.level) {
            (true, _) | (_, Level::Error) => Tier::Fix,
            (_, Level::Warn) => Tier::Note,
        };
        let text = if finding.role.is_empty() {
            finding.message.clone()
        } else {
            format!("{}: {}", finding.role, finding.message)
        };
        failure.notes.push(Note {
            expected: finding.expected.clone(),
            actual: finding.actual.clone(),
            ..Note::new(
                DiagnosticId::Finding {
                    kind: finding.kind,
                    instruction,
                    account: Some(finding.index),
                },
                tier,
                text,
            )
        });
    }

    if corroborated {
        // The evidence names the account, so the class's inferred advice is now
        // a fact. Other guesses are not corroborated by it.
        for note in &mut failure.notes {
            if note.confidence == Confidence::Likely && note.id == DiagnosticId::Log("class.inferred") {
                note.confidence = Confidence::Certain;
            }
        }
        // A declared role beats our own guess at which keys did not sign.
        failure
            .notes
            .retain(|note| note.id != DiagnosticId::Log("signature.missing"));
    }

    finalize(&mut failure.notes);
}

/// Names an unknown custom code from what the IDL flavor implies.
pub fn infer_custom(code: u32, flavor: Option<super::idl::Flavor>) -> (String, String, String) {
    use super::idl::Flavor;

    match flavor {
        Some(Flavor::Anchor | Flavor::AnchorLegacy) if code >= ANCHOR_USER_OFFSET => (
            format!("Anchor user error #{}", code - ANCHOR_USER_OFFSET),
            "the deployed program raised a code its IDL does not list".to_string(),
            "the on-chain IDL may be older than the deployed program — redeploy or refresh it".to_string(),
        ),
        Some(Flavor::Anchor | Flavor::AnchorLegacy) => match anchor_error(code) {
            Some(known) => (
                known.name.to_string(),
                known.message.to_string(),
                "raised by the Anchor framework, not the program's own error enum".to_string(),
            ),
            None => (
                "Anchor program error".to_string(),
                "below Anchor's 6000 user range, so it came from the framework".to_string(),
                "check the account constraints on the failing instruction".to_string(),
            ),
        },
        Some(flavor @ (Flavor::Shank | Flavor::Codama)) => (
            format!("program error #{code}"),
            format!("{} programs number their errors from 0", flavor.label()),
            format!(
                "error {code} is the {} entry of the program's error enum",
                ordinal(code + 1)
            ),
        ),
        None if code >= ANCHOR_USER_OFFSET => (
            format!("Anchor user error #{}", code - ANCHOR_USER_OFFSET),
            "the 6000 range is Anchor's user-defined block".to_string(),
            "no IDL found for this program — drop one in target/idl or point at it from patp.toml".to_string(),
        ),
        None => (
            format!("custom program error {code}"),
            "no IDL and no known error table for this program".to_string(),
            "programs outside Anchor usually number errors from 0, so this is likely their enum index".to_string(),
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
