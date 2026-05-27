//! Remote HTTP storage backend for `next`.
//!
//! This crate provides [`RemoteStore`] and [`RemoteVcs`] — implementations of
//! the [`next::store::Store`] and [`next::store::VcsBackend`] traits that talk
//! to a hosted `next-server` instance over HTTP instead of reading TOML files
//! from disk.
//!
//! # Current status
//!
//! **Stub** — all store operations return an "not yet implemented" error. The
//! VCS operations are no-ops (the server is the authoritative source of truth
//! and handles its own persistence). The HTTP client and server protocol will
//! be added in a future milestone.
//!
//! # Configuration
//!
//! ```toml
//! # $XDG_CONFIG_HOME/task-manager/config.toml
//! [backend]
//! kind = "remote"
//!
//! [backend.remote]
//! url   = "https://tasks.example.com"
//! token = "my-bearer-token"   # optional
//! ```

pub mod remote_store;
pub mod remote_vcs;

pub use remote_store::RemoteStore;
pub use remote_vcs::RemoteVcs;
