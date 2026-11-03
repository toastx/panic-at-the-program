//! The native programs' error enums.

use crate::app::errors::{Known, coded};
use crate::app::solana::{SYSTEM, TOKEN, TOKEN_2022};

/// Token and Token-2022, which share one error numbering.
const TOKEN_TSV: &str = include_str!("token.tsv");

const SYSTEM_TSV: &str = include_str!("system.tsv");

/// A native program's custom error. None for any other program.
pub fn program(program: &str, code: u32) -> Option<Known> {
    let tsv = match program {
        TOKEN | TOKEN_2022 => TOKEN_TSV,
        SYSTEM => SYSTEM_TSV,
        _ => return None,
    };
    coded(tsv).find(|(row, _)| *row == code).map(|(_, known)| known)
}

/// The short label a native program's codes are shown under, e.g. `token.1`.
pub fn short_name(program: &str) -> Option<&'static str> {
    match program {
        TOKEN | TOKEN_2022 => Some("token"),
        SYSTEM => Some("system"),
        _ => None,
    }
}
