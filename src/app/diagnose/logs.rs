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

    if text.contains("access violation") || text.contains("panicked at") {
        out.push(note(
            LogId::ProgramPanic,
            "the program panicked — check unwraps and arithmetic in that instruction".to_string(),
        ));
    }

    out
}
