//! External plugin support.
//!
//! Plugins are external binaries that subscribe to individual tasks via the
//! `next plugin` CLI (see [`registry`]).  When a subscribed task is updated,
//! `next` spawns the plugin to notify it (see [`notify`]).  The plugin performs
//! its work by linking this crate or invoking the `next` CLI.

pub mod notify;
pub mod registry;
pub mod run;

pub use notify::{notify, TaskEvent};
pub use registry::{Plugin, PluginRegistry};
pub use run::{resolve_sync_interval, run_due_syncs};
