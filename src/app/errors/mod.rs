//! Error tables: what each code or name means, and which class of fix applies.
//!
//! Every table is a TSV beside the loader that reads it. Which one is consulted
//! for a custom code depends on the program's framework, read from patp.toml.

pub mod anchor;
mod class;
mod environment;
pub mod solana;
mod table;

pub use class::{Class, classify_name};
pub use environment::*;
pub use table::*;
