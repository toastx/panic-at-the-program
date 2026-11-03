//! Folding account validation into a diagnosis.

use super::*;
use crate::app::errors::Class;
use crate::app::validate::{Finding, FindingKind, Level};

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
        failure.notes.push(
            Note::new(
                DiagnosticId::Finding {
                    kind: finding.kind,
                    instruction,
                    account: Some(finding.index),
                },
                tier,
                text,
            )
            .values(finding.expected.clone(), finding.actual.clone()),
        );
    }

    if corroborated {
        // The evidence names the account, so the class's inferred advice is now
        // a fact. Other guesses are not corroborated by it.
        for note in &mut failure.notes {
            if note.confidence == Confidence::Likely && note.id == DiagnosticId::Log(LogId::ClassInferred) {
                note.confidence = Confidence::Certain;
            }
        }
        // A declared role beats our own guess at which keys did not sign.
        failure
            .notes
            .retain(|note| note.id != DiagnosticId::Log(LogId::SignatureMissing));
    }

    finalize(&mut failure.notes);
}
