//! Forgejo integration plugin (`next-plugin-forgejo`), gated by the `forgejo`
//! feature.
//!
//! Maps Forgejo repositories to `next` contexts: imports open issues as tasks,
//! marks tasks done when their issue is closed, and closes the issue when the
//! task is resolved locally. Issues and tasks are linked through `__forgejo-*`
//! task data attributes.
//!
//! Because the plugin lives in the same crate as `next`, it links the library
//! directly rather than shelling out to the CLI.

pub mod config;
pub mod issues;
pub mod reconcile;
pub mod tasks;

pub use config::{Config, Mapping};
pub use issues::{ForgejoIssue, IssueSource, IssueState};
pub use tasks::{forgejo_link, TaskStore};

/// Task `data` keys used to link a task to its Forgejo issue. All carry the
/// `__forgejo-` prefix and satisfy `domain::task::validate_key`.
pub mod keys {
    pub const REPO: &str = "__forgejo-repo";
    pub const ISSUE: &str = "__forgejo-issue";
    pub const URL: &str = "__forgejo-url";
    pub const LABELS: &str = "__forgejo-labels";
}

/// The plugin name used for export-hook registration and as the loop-guard
/// origin token.
pub const PLUGIN_NAME: &str = "next-plugin-forgejo";
