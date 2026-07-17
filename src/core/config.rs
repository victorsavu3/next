use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_FORECAST_HORIZON_DAYS: u32 = 90;
pub const DEFAULT_NEXT_COUNT: usize = 10;

fn default_forecast_horizon_days() -> u32 { DEFAULT_FORECAST_HORIZON_DAYS }
fn default_next_count() -> usize { DEFAULT_NEXT_COUNT }
fn default_pull_before_query() -> bool { true }
fn default_staleness_secs() -> u64 { 3600 }
fn default_pull_timeout_secs() -> u64 { 10 }
fn default_plugin_sync_default_secs() -> u64 { 86400 }

// ── SyncConfig ────────────────────────────────────────────────────────────────

/// Controls how `next sync` (and autosync) performs push/pull operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// When `true`, `next sync` runs `git pull` / `git push` as shell
    /// subprocesses instead of using the libgit2 bindings.
    ///
    /// Use this when the built-in bindings fail to authenticate (e.g. because
    /// your SSH key is managed by a keychain, 1Password, or a non-standard
    /// agent socket) while plain `git` commands work fine.
    #[serde(default)]
    pub git_subprocess: bool,

    /// When `true` (the default), a command pulls from the remote before
    /// reading/writing if the local copy is stale (see `staleness_secs`).  The
    /// pull is best-effort: it never fails or blocks the command.  Bypass it for
    /// a single invocation with `--offline`.
    #[serde(default = "default_pull_before_query")]
    pub pull_before_query: bool,

    /// How long (seconds) a local copy stays "fresh" after a pull before
    /// `pull_before_query` will pull again.  Default 3600 (one hour).
    #[serde(default = "default_staleness_secs")]
    pub staleness_secs: u64,

    /// Maximum time (seconds) a pull-before-query pull may take before being
    /// abandoned.  Stored only — not yet enforced (deferred to a later task).
    #[serde(default = "default_pull_timeout_secs")]
    pub pull_timeout_secs: u64,

    /// System-default interval (seconds) between a plugin's periodic syncs,
    /// used when a plugin sets no default and the user sets no override.  This
    /// is the lowest-priority value in the SYSTEM → PLUGIN → USER precedence.
    /// Default 86400 (one day).
    #[serde(default = "default_plugin_sync_default_secs")]
    pub plugin_sync_default_secs: u64,

    /// When `true`, behaves as if `--offline`/`--no-sync` was passed on every
    /// invocation: skips both the pull-before-query and the autosync push.
    /// Default `false`.
    #[serde(default)]
    pub offline: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            git_subprocess: false,
            pull_before_query: default_pull_before_query(),
            staleness_secs: default_staleness_secs(),
            pull_timeout_secs: default_pull_timeout_secs(),
            plugin_sync_default_secs: default_plugin_sync_default_secs(),
            offline: false,
        }
    }
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
/// ```
///
/// Scoring weights are **not** configured here — they live in the repository at
/// `config/scoring.toml` so every consumer (cli/mcp/forgejo) shares one view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Number of days ahead shown by `next forecast`.
    #[serde(default = "default_forecast_horizon_days")]
    pub forecast_horizon_days: u32,

    /// Default number of tasks shown by `next next`.
    #[serde(default = "default_next_count")]
    pub next_count: usize,

    /// Optional cap on `next list` output.  When set, `next list` truncates
    /// results to this many tasks (same as passing `--limit N`).  `None`
    /// (the default) falls back to the default page size of 50
    /// ([`crate::core::store::DEFAULT_PAGE_SIZE`]).
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
        // Pull-before-query defaults.
        assert!(cfg.sync.pull_before_query);
        assert_eq!(cfg.sync.staleness_secs, 3600);
        assert_eq!(cfg.sync.pull_timeout_secs, 10);
    }

    #[test]
    fn sync_config_defaults_match() {
        let sync = SyncConfig::default();
        assert!(!sync.git_subprocess);
        assert!(sync.pull_before_query);
        assert_eq!(sync.staleness_secs, 3600);
        assert_eq!(sync.pull_timeout_secs, 10);
    }

    #[test]
    fn sync_pull_before_query_fields_round_trip() {
        let cfg: Config = toml::from_str(
            "[sync]\npull_before_query = false\nstaleness_secs = 42\npull_timeout_secs = 7",
        )
        .unwrap();
        assert!(!cfg.sync.pull_before_query);
        assert_eq!(cfg.sync.staleness_secs, 42);
        assert_eq!(cfg.sync.pull_timeout_secs, 7);

        // Serialize back out and re-parse to confirm a full round-trip.
        let text = toml::to_string(&cfg).unwrap();
        let reparsed: Config = toml::from_str(&text).unwrap();
        assert!(!reparsed.sync.pull_before_query);
        assert_eq!(reparsed.sync.staleness_secs, 42);
        assert_eq!(reparsed.sync.pull_timeout_secs, 7);
    }

    #[test]
    fn sync_partial_section_uses_defaults_for_rest() {
        let cfg: Config = toml::from_str("[sync]\nstaleness_secs = 100").unwrap();
        assert_eq!(cfg.sync.staleness_secs, 100);
        assert!(cfg.sync.pull_before_query, "unspecified field keeps its default");
        assert_eq!(cfg.sync.pull_timeout_secs, 10);
    }

    #[test]
    fn sync_offline_defaults_to_false() {
        assert!(!SyncConfig::default().offline);
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.sync.offline);
    }

    #[test]
    fn sync_offline_round_trips() {
        let cfg: Config = toml::from_str("[sync]\noffline = true").unwrap();
        assert!(cfg.sync.offline);
        let text = toml::to_string(&cfg).unwrap();
        let reparsed: Config = toml::from_str(&text).unwrap();
        assert!(reparsed.sync.offline);
    }
}
