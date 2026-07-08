//! `ccswitch` — switch between multiple Claude Code accounts.
//!
//! This crate is organized with a ports-and-adapters layout: every real
//! operating-system or network side effect lives behind a trait whose
//! concrete implementation is in a `*_shim.rs` file (excluded from coverage),
//! while all decision logic lives in the plain modules and is unit-tested
//! with fakes.

pub mod cli;
pub mod cli_shim;
pub mod config;
pub mod creds;
pub mod creds_shim;
pub mod error;
pub mod model;
pub mod store;
pub mod switch;

pub use error::{Error, Result};
pub use model::{Account, Profile};
