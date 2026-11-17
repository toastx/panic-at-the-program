//! Notes only the program's logs can supply.

use super::*;
use crate::app::errors::Class;

pub struct Panic {
    pub source: Option<String>,
    pub message: String,
}

impl Panic {
    /// What to do about the message, and the class it implies when it does.
    pub fn advice(&self) -> (&'static str, Option<Class>) {
        let text = self.message.to_lowercase();
        if text.contains("overflow") || text.contains("divide by zero") {
            return (
                "use checked_* or saturating_* arithmetic and validate the inputs",
                Some(Class::Overflow),
            );
        }
        if text.contains("unwrap()") || text.contains("expect()") {
            return (
                "an unwrap or expect failed — handle the None/Err or return an error",
                None,
            );
        }
        if text.contains("out of bounds") || text.contains("range end index") || text.contains("range start index") {
            return (
                "a slice is read past its length — check account data sizes and offsets",
                None,
            );
        }
        ("check unwraps and arithmetic in that instruction", None)
    }
}

/// `file:line:col` without the column, the way Anchor's `thrown in` reads.
fn location(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches(':');
    let head = text.rsplit_once(':').filter(|(_, col)| col.parse::<u32>().is_ok());
    Some(head.map_or(text, |(head, _)| head).to_string()).filter(|path| !path.is_empty())
}

/// The panic a program logged, in either shape Rust has used:
/// `panicked at 'msg', file:line:col`, or `panicked at file:line:col:` with
/// the message after a newline — in the same log line, or the next.
pub fn panic(logs: &[String]) -> Option<Panic> {
    let (index, rest) = logs
        .iter()
        .enumerate()
        .find_map(|(index, line)| Some((index, line.split_once("panicked at ")?.1)))?;
    if let Some(rest) = rest.strip_prefix('\'') {
        let (message, source) = rest.split_once("', ").unwrap_or((rest, ""));
        return Some(Panic {
            source: location(source),
            message: message.trim_end_matches('\'').to_string(),
        });
    }
    let (source, message) = rest.split_once('\n').unwrap_or((rest, ""));
    let message = match message.trim() {
        "" => logs
            .get(index + 1)
            .and_then(|line| line.strip_prefix("Program log: "))
            .unwrap_or_default(),
        message => message,
    };
    Some(Panic {
        source: location(source),
        message: message.trim().to_string(),
    })
}

/// Notes that can only be read out of the program's logs.
pub fn log_notes(code: &Code, logs: &[String], cu: u32, cu_limit: u32) -> Vec<Note> {
    let mut out = Vec::new();
    let text = logs.join("\n").to_lowercase();
    let note = |id, text: String| Note::new(DiagnosticId::Log(id), Tier::Help, text);

    if text.contains("exceeded cus")
        || text.contains("exceeded maximum number of instructions")
        || matches!(code, Code::Named(name) if name == "ComputationalBudgetExceeded")
    {
        out.push(note(
            LogId::CuExceeded,
            format!(
                "ran out of compute: {cu} of {cu_limit} units used — raise it with ComputeBudget::SetComputeUnitLimit"
            ),
        ));
    }

    // The System Program's transfer check: the source's balance, then the amount
    // it tried to move. Not a rent figure — `create_account` hits the same check
    // with the rent-exempt deposit as the amount, but a plain transfer does too.
    if let Some(line) = logs.iter().find(|line| line.contains("insufficient lamports")) {
        let numbers: Vec<u64> = line
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|part| part.parse().ok())
            .collect();
        let sol = |lamports: u64| lamports as f64 / 1e9;
        out.push(match numbers.as_slice() {
            [have, need, ..] if need > have => note(
                LogId::LamportShortfall,
                format!(
                    "the source holds {:.9} SOL but the transfer needs {:.9} — fund it with {:.9} more SOL",
                    sol(*have),
                    sol(*need),
                    sol(need - have)
                ),
            ),
            _ => note(
                LogId::LamportShortfall,
                "the source does not hold enough lamports for this transfer — fund it first".to_string(),
            ),
        });
    }

    if text.contains("already in use") {
        out.push(note(
            LogId::AccountInUse,
            "the account already exists — change the seed, or use init_if_needed".to_string(),
        ));
    }

    if text.contains("slippage") {
        out.push(note(
            LogId::Slippage,
            "the quote moved before the transaction landed — widen the tolerance or requote".to_string(),
        ));
    }

    // Frames are 4 KB; the heap 32 KB unless asked for more. Both end the
    // program without a panic line of their own.
    if text.contains("access violation") {
        out.push(note(
            LogId::StackOverflow,
            "the stack overflowed — Box large structs, keep big arrays off the stack, or use zero-copy accounts"
                .to_string(),
        ));
    }
    if text.contains("out of memory") || (text.contains("memory allocation") && text.contains("failed")) {
        out.push(note(
            LogId::HeapExhausted,
            "the heap ran out — avoid growing Vecs and clones, or request more with ComputeBudget::RequestHeapFrame"
                .to_string(),
        ));
    }

    if let Some(panicked) = panic(logs) {
        let (advice, _) = panicked.advice();
        let at = panicked
            .source
            .as_deref()
            .map(|source| format!(" at {source}"))
            .unwrap_or_default();
        let said = match panicked.message.as_str() {
            "" => String::new(),
            message => format!(": {message}"),
        };
        out.push(note(
            LogId::ProgramPanic,
            format!("the program panicked{at}{said} — {advice}"),
        ));
    }

    out
}
