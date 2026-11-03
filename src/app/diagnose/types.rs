//! What a diagnosis is made of: the failure, its notes, and their identities.

use crate::app::errors::{self, Class};
use crate::app::validate::FindingKind;

/// Identity of a diagnostic that only a log line, an IDL, or a config can
/// supply — the ones with no numeric code to key on.
///
/// An enum rather than a string because these ids are matched across modules:
/// [`crate::app::idl`] drops `AnchorUserRange` and promotes `ClassInferred`
/// by id, and a typo in a free-form string would silently stop matching
/// instead of failing to compile.
///
/// Declared in label order: `finalize` sorts by id within a tier, so the
/// variant order is the render order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LogId {
    AccountConstraint,
    AccountInUse,
    AnchorUserRange,
    ClassInferred,
    CuExceeded,
    FlavorInferred,
    LamportShortfall,
    NamePatpToml,
    NameUnnamed,
    ProgramPanic,
    SignatureMissing,
    Slippage,
    ValueMismatch,
}

impl LogId {
    fn label(self) -> &'static str {
        match self {
            Self::AccountConstraint => "account.constraint",
            Self::AccountInUse => "account.in_use",
            Self::AnchorUserRange => "anchor.user_range",
            Self::ClassInferred => "class.inferred",
            Self::CuExceeded => "cu.exceeded",
            Self::FlavorInferred => "flavor.inferred",
            Self::LamportShortfall => "lamports.shortfall",
            Self::NamePatpToml => "name.patp_toml",
            Self::NameUnnamed => "name.unnamed",
            Self::ProgramPanic => "program.panic",
            Self::SignatureMissing => "signature.missing",
            Self::Slippage => "slippage",
            Self::ValueMismatch => "value.mismatch",
        }
    }
}

/// Identity of one diagnostic, used for deduplication and ordering.
///
/// Carries enough context that two genuinely different diagnostics never
/// collide: custom codes are program-scoped because they index each program's
/// own error enum, and findings carry their instruction and account slot
/// because one transaction can have two missing signers.
///
/// This is identity, not presentation. [`Display`](std::fmt::Display) is deliberately lossy — it
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
    Log(LogId),
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
            Self::Program { program, code } => match errors::solana::short_name(program) {
                Some(label) => write!(f, "{label}.{code}"),
                None => write!(f, "custom.{code}"),
            },
            Self::Runtime(name) => write!(f, "runtime.{name}"),
            Self::Log(id) => write!(f, "{}", id.label()),
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
