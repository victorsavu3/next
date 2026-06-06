//! Core library — everything that is not a feature-gated binary
//! (`cli` / `mcp` / `forgejo`). Built with no features, the crate is just this.

pub mod bootstrap;
pub mod config;
pub mod domain;
pub mod error;
pub mod filter_args;
pub mod forecast;
pub mod plugin;
pub mod recurrence;
pub mod resolve;
pub mod scoring;
pub mod service;
pub mod storage;
pub mod store;
pub mod sync;
pub mod task_repository;
pub mod value;

pub use error::{Result, TaskError};
pub use filter_args::FilterArgs;
pub use store::{PullResult, Store, VcsBackend};
pub use sync::{sync, SyncOutcome};
pub use task_repository::TaskRepository;
pub use value::parse_value;
