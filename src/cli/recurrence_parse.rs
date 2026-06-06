//! CLI recurrence-argument parsing.
//!
//! The parsing logic now lives in [`crate::core::recurrence::parse_recurrence`]
//! so the TUI (which must not depend on `cli`) can share it. This module
//! re-exports it to keep the existing `crate::cli::recurrence_parse` import path
//! used by the `add` and `edit` commands unchanged.
pub use crate::core::recurrence::parse_recurrence;
