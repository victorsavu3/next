pub mod app_context;
pub mod cli;
pub mod config;
pub mod domain;
pub mod error;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod plugin;
pub mod resolve;
pub mod storage;
pub mod store;

pub use app_context::AppContext;
pub use config::{BackendConfig, BackendKind, Config};
pub use error::{AppError, Result};
pub use store::{PullResult, Store, VcsBackend};
