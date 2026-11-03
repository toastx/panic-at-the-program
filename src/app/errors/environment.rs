//! Which tables apply to a program, from what patp.toml records about it.

use crate::app::idl::Flavor;

/// The framework `program` was built with, as patp.toml records it under
/// `[idl.kind]`. None until the program's IDL has been seen once.
pub fn flavor(program: &str) -> Option<Flavor> {
    Flavor::from_label(&crate::workspace::value("idl.kind", program)?)
}

/// Whether Anchor's codes can explain a custom error from `program`.
///
/// Shank and Codama programs number their own errors from 0, so their 2006 is
/// an entry in their own enum, not Anchor's `ConstraintSeeds`. Programs with no
/// recorded flavor are read as Anchor, which most are.
pub fn anchor_applies(program: &str) -> bool {
    !matches!(flavor(program), Some(Flavor::Shank | Flavor::Codama))
}
