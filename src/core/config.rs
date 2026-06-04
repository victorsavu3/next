use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::scoring::ScoringConfig;

pub const DEFAULT_FORECAST_HORIZON_DAYS: u32 = 90;
pub const DEFAULT_NEXT_COUNT: usize = 10;

fn default_forecast_horizon_days() -> u32 { DEFAULT_FORECAST_HORIZON_DAYS }
fn default_next_count() -> usize { DEFAULT_NEXT_COUNT }

// ── SyncConfig ────────────────────────────────────────────────────────────────

/// Controls how `next sync` (and autosync) performs push/pull operations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncConfig {
    /// When `true`, `next sync` runs `git pull` / `git push` as shell
    /// subprocesses instead of using the libgit2 bindings.
    ///
    /// Use this when the built-in bindings fail to authenticate (e.g. because
    /// your SSH key is managed by a keychain, 1Password, or a non-standard
    /// agent socket) while plain `git` commands work fine.
    #[serde(default)]
    pub git_subprocess: bool,
}

// ── Config ────────────────────────────────────────────────────────────────────

/// Application-wide configuration.
///
/// Loaded from `$XDG_CONFIG_HOME/task-manager/config.toml`.
/// All fields are optional; missing fields use the values shown in
/// [`Config::default`].
///
/// Example config file:
/// ```toml
/// repository = "/home/alice/tasks"   # use next from any directory
/// forecast_horizon_days = 60
/// next_count = 5
///
/// [scoring]
/// priority_high = 3.0          # only override what you want to change
/// age_max       = 3.0
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Urgency scoring weights.
    #[serde(default)]
    pub scoring: ScoringConfig,

    /// Number of days ahead shown by `next forecast`.
    #[serde(default = "default_forecast_horizon_days")]
    pub forecast_horizon_days: u32,

    /// Default number of tasks shown by `next next`.
    #[serde(default = "default_next_count")]
    pub next_count: usize,

    /// Optional cap on `next list` output.  When set, `next list` truncates
    /// results to this many tasks (same as passing `--limit N`).  `None`
    /// (the default) means no cap — all matching tasks are shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_limit: Option<usize>,

    /// Default repository root.  When set, `next` uses this path instead of
    /// walking up from the current directory.  Can be overridden at runtime
    /// with the `--repo` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<PathBuf>,

    /// When true, automatically sync with the remote after each mutation
    /// command.  Can be overridden at runtime with the `--autosync` flag.
    #[serde(default)]
    pub autosync: bool,

    /// Controls push/pull behaviour during `next sync` and autosync.
    #[serde(default)]
    pub sync: SyncConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scoring: ScoringConfig::default(),
            forecast_horizon_days: default_forecast_horizon_days(),
            next_count: default_next_count(),
            list_limit: None,
            repository: None,
            autosync: false,
            sync: SyncConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosync_defaults_to_false() {
        assert!(!Config::default().autosync);
    }

    #[test]
    fn autosync_deserializes_from_toml() {
        let cfg: Config = toml::from_str("autosync = true").unwrap();
        assert!(cfg.autosync);
    }

    #[test]
    fn sync_git_subprocess_deserializes_from_toml() {
        let cfg: Config = toml::from_str("[sync]\ngit_subprocess = true").unwrap();
        assert!(cfg.sync.git_subprocess);
    }

    #[test]
    fn sync_section_absent_defaults_correctly() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.sync.git_subprocess);
    }
}
