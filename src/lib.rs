pub mod core;

#[cfg(feature = "cli")]
pub mod app_context;
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "forgejo")]
pub mod forgejo;
#[cfg(feature = "mcp")]
pub mod mcp;

// Crate-root prelude of the most-used core types.
#[cfg(feature = "cli")]
pub use app_context::AppContext;
pub use core::config::Config;
pub use core::error::{Result, TaskError};
pub use core::store::{PullResult, Store, VcsBackend};
pub use core::task_repository::TaskRepository;
