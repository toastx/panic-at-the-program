//! The native programs, by id and by name.

pub const SYSTEM: &str = "11111111111111111111111111111111";

pub const TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

pub const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

pub const ATA: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

pub(super) fn program_name(id: &str) -> String {
    if let Some(label) = crate::workspace::value("programs", id) {
        return label;
    }
    match id {
        SYSTEM => "System Program",
        TOKEN => "Token Program",
        TOKEN_2022 => "Token-2022",
        ATA => "Associated Token",
        "ComputeBudget111111111111111111111111111111" => "Compute Budget",
        "Vote111111111111111111111111111111111111111" => "Vote Program",
        other => {
            return format!(
                "{}…{}",
                &other[..4.min(other.len())],
                &other[other.len().saturating_sub(4)..]
            );
        }
    }
    .to_string()
}
