//! Shared logic used by more than one feature module (`cli`, `mcp`, `forgejo`).
//!
//! These helpers live here — not in `cli` — so the feature modules depend only
//! on the always-compiled core, and the crate built with no features is a clean
//! core library others can link.

pub mod filter_args;
pub mod sync;
pub mod value;

pub use filter_args::FilterArgs;
pub use sync::{sync, SyncOutcome};
pub use value::parse_value;
