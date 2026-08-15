//! Core library — everything that is not a feature-gated binary
//! (`cli` / `mcp` / `forgejo`). Built with no features, the crate is just this.

/// The directory this application owns under `$XDG_CONFIG_HOME` and
/// `$XDG_STATE_HOME`.
///
/// One constant rather than a literal at each of the four sites that build
/// such a path. It was `task-manager` until the crate, the CLI and the
/// repository were all renamed to `next` and this was left behind — four
/// copies is exactly how that happens, and how a future rename would half-land
/// again.
pub const APP_DIR: &str = "next";

pub mod archiver;
pub mod bootstrap;
pub mod config;
pub mod domain;
pub mod error;
pub mod filter_args;
pub mod forecast;
pub mod listing;
pub mod plugin;
pub mod projection;
pub mod recurrence;
pub mod resolve;
pub mod scoring;
pub mod service;
pub mod storage;
pub mod store;
pub mod sync;
pub mod sync_state;
pub mod tag_rename;
pub mod task_repository;
#[cfg(test)]
pub(crate) mod test_git;
pub mod value;

pub use error::{Result, TaskError};
pub use filter_args::{reject_flag_like_tokens, FilterArgs};
pub use store::{Page, PullResult, Store, TaskQuery, VcsBackend};
pub use sync::{sync, SyncOutcome};
pub use task_repository::TaskRepository;
pub use value::parse_value;
