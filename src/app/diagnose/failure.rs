//! From the RPC's error value to a [`Failure`]: which instruction failed,
//! what the code means, and the notes the logs support.

use super::*;
use crate::app::errors::{self, Class, Known, anchor};
use crate::app::solana::{Node, Status};
use serde_json::Value;

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
        .map(|a| crate::ui::widgets::short(&a.key))
        .take(4)
        .collect();
    (!missing.is_empty()).then(|| missing.join(", "))
}

/// Notes that only Anchor's own log output can supply.
fn anchor_notes(anchor: &anchor::Log) -> Vec<Note> {
    let mut out = Vec::new();
    if let (Some(expected), Some(actual)) = (&anchor.expected, &anchor.actual)
        && expected != actual
    {
        out.push(
            Note::new(
                DiagnosticId::Log(LogId::ValueMismatch),
                Tier::Fix,
                "the value the program expected differs from the one passed".to_string(),
            )
            .values(Some(expected.clone()), Some(actual.clone())),
        );
    }
    if let Some(account) = &anchor.account {
        out.push(Note::new(
            DiagnosticId::Log(LogId::AccountConstraint),
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
    let anchor = anchor::log(logs_slice);
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
        && n >= anchor::USER_OFFSET
        && errors::anchor_applies(program)
    {
        notes.push(Note::new(
            DiagnosticId::Log(LogId::AnchorUserRange),
            Tier::Note,
            format!(
                "program-defined error #{} — check the program's error enum",
                n - anchor::USER_OFFSET
            ),
        ));
    }
    // Superseded by the IDL's declared signer roles, when there are any.
    if let (Code::Named(name), Some(index)) = (&code, node)
        && name == "MissingRequiredSignature"
        && let Some(missing) = unsigned_accounts(&nodes[index])
    {
        notes.push(Note::new(
            DiagnosticId::Log(LogId::SignatureMissing),
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

/// Resolves a wire code into its identity, name, meaning, class, and advice.
fn describe(code: &Code, program: &str) -> (DiagnosticId, String, String, Option<Class>, &'static str) {
    let from = |id, known: Known| {
        (
            id,
            known.name.to_string(),
            known.message.to_string(),
            known.class,
            known.help,
        )
    };
    let unknown = |id, headline: String, meaning: &str| (id, headline, meaning.to_string(), None, "");
    match code {
        Code::Custom(n) => {
            let scoped = DiagnosticId::Program {
                program: program.to_string(),
                code: *n,
            };
            if let Some(known) = errors::solana::program(program, *n) {
                return from(scoped, known);
            }
            if !errors::anchor_applies(program) {
                return unknown(
                    scoped,
                    format!("custom program error {n}"),
                    "an entry in the program's own error enum",
                );
            }
            if *n >= anchor::USER_OFFSET {
                return unknown(
                    scoped,
                    format!("Anchor user error #{}", n - anchor::USER_OFFSET),
                    "defined by the program itself; the IDL carries its name and message",
                );
            }
            match anchor::lookup(*n) {
                Some(known) => from(DiagnosticId::Anchor(*n), known),
                None => unknown(
                    scoped,
                    format!("custom program error {n}"),
                    "not a known Anchor or SPL code",
                ),
            }
        }
        Code::Named(name) => {
            let id = DiagnosticId::Runtime(name.clone());
            match errors::solana::runtime(name) {
                Some(known) => from(id, known),
                None => unknown(id, name.clone(), ""),
            }
        }
        Code::Other(detail) => unknown(DiagnosticId::Other, detail.clone(), ""),
    }
}
