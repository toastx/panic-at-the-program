//! Notes only the program's logs can supply.

use super::*;

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

    if let Some(line) = logs.iter().find(|line| line.contains("insufficient lamports")) {
        let numbers: Vec<u64> = line
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|part| part.parse().ok())
            .collect();
        out.push(match numbers.as_slice() {
            [have, need, ..] if need > have => note(
                LogId::RentShortfall,
                format!(
                    "fund the account with {:.9} more SOL to reach the rent-exempt minimum",
                    (need - have) as f64 / 1e9
                ),
            ),
            _ => note(
                LogId::RentShortfall,
                "an account is below the rent-exempt minimum — fund it first".to_string(),
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

    if text.contains("access violation") || text.contains("panicked at") {
        out.push(note(
            LogId::ProgramPanic,
            "the program panicked — check unwraps and arithmetic in that instruction".to_string(),
        ));
    }

    out
}
