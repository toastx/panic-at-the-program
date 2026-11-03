//! Naming a custom code that no table or IDL names.

use crate::app::errors::anchor;

/// Names an unknown custom code from what the IDL flavor implies.
pub fn infer_custom(code: u32, flavor: Option<crate::app::idl::Flavor>) -> (String, String, String) {
    use crate::app::idl::Flavor;

    match flavor {
        Some(Flavor::Anchor | Flavor::AnchorLegacy) if code >= anchor::USER_OFFSET => (
            format!("Anchor user error #{}", code - anchor::USER_OFFSET),
            "the deployed program raised a code its IDL does not list".to_string(),
            "the on-chain IDL may be older than the deployed program — redeploy or refresh it".to_string(),
        ),
        Some(Flavor::Anchor | Flavor::AnchorLegacy) => match anchor::lookup(code) {
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
                ordinal(u64::from(code) + 1)
            ),
        ),
        None if code >= anchor::USER_OFFSET => (
            format!("Anchor user error #{}", code - anchor::USER_OFFSET),
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

fn ordinal(n: u64) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}
