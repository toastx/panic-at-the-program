//! The row every error table shares, and the TSV parsing behind each loader.

use super::*;

/// One row of an error table.
pub struct Known {
    pub name: &'static str,
    pub message: &'static str,
    pub class: Option<Class>,
    /// Advice for this error alone. Empty falls back to the class's help.
    pub help: &'static str,
}

/// The columns every table ends with: `name message class help`.
fn known(mut col: std::str::Split<'static, char>) -> Option<Known> {
    Some(Known {
        name: col.next()?,
        message: col.next()?,
        class: col.next().filter(|c| !c.is_empty()).and_then(Class::from_name),
        help: col.next().unwrap_or_default(),
    })
}

/// A TSV's rows split into columns, with comments and blank lines dropped.
fn rows(tsv: &'static str) -> impl Iterator<Item = std::str::Split<'static, char>> {
    tsv.lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| line.split('\t'))
}

/// A table keyed by code: `code name message class help`.
pub(super) fn coded(tsv: &'static str) -> impl Iterator<Item = (u32, Known)> {
    rows(tsv).filter_map(|mut col| Some((col.next()?.parse().ok()?, known(col)?)))
}

/// A table keyed by name: `name message class help`.
pub(super) fn named(tsv: &'static str) -> impl Iterator<Item = Known> {
    rows(tsv).filter_map(known)
}
