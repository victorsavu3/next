use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_FORECAST_HORIZON_DAYS: u32 = 90;
pub const DEFAULT_NEXT_COUNT: usize = 10;

fn default_forecast_horizon_days() -> u32 {
    DEFAULT_FORECAST_HORIZON_DAYS
}
fn default_next_count() -> usize {
    DEFAULT_NEXT_COUNT
}
fn default_autopull() -> bool {
    true
}
fn default_staleness_secs() -> u64 {
    3600
}
fn default_pull_timeout_secs() -> u64 {
    10
}
fn default_plugin_sync_default_secs() -> u64 {
    86400
}

// ── SyncConfig ────────────────────────────────────────────────────────────────

/// Controls how `next` synchronises with the remote VCS backend.
///
/// The two sync capabilities are orthogonal and each has a config key with a
/// paired pair of CLI override flags:
///
/// * `autopull` (`--autopull` / `--no-autopull`) — the staleness pull run
///   before every command except `sync`.  Default `true`.
/// * `autopush` (`--autopush` / `--no-autopush`) — the push run after a
///   successful mutation.  Default `false`.
///
/// The master flags `--autosync` / `--no-autosync` (and its alias `--offline`)
/// toggle both at once for a single invocation.
///
/// Migration: the old top-level `autosync` key is replaced by
/// `[sync] autopush`; the old `[sync] pull_before_query` is renamed to
/// `[sync] autopull` (still accepted as a serde alias); the old
/// `[sync] offline` key is removed (use `--offline` per invocation, or set
/// `autopull = false` / `autopush = false`).
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
    /// a single invocation with `--no-autopull` (or `--offline`).
    ///
    /// Accepts the old key name `pull_before_query` as a serde alias.
    #[serde(default = "default_autopull", alias = "pull_before_query")]
    pub autopull: bool,

    /// When `true`, push to the remote after each successful mutation command.
    /// Default `false`.  Enable for a single invocation with `--autopush` (or
    /// `--autosync`).  Replaces the old top-level `autosync` key.
    #[serde(default)]
    pub autopush: bool,

    /// How long (seconds) a local copy stays "fresh" after a pull before
    /// `autopull` will pull again.  Default 3600 (one hour).
    #[serde(default = "default_staleness_secs")]
    pub staleness_secs: u64,

    /// Maximum time (seconds) an autopull pull may take before being
    /// abandoned.  Stored only — not yet enforced (deferred to a later task).
    #[serde(default = "default_pull_timeout_secs")]
    pub pull_timeout_secs: u64,

    /// System-default interval (seconds) between a plugin's periodic syncs,
    /// used when a plugin sets no default and the user sets no override.  This
    /// is the lowest-priority value in the SYSTEM → PLUGIN → USER precedence.
    /// Default 86400 (one day).
    #[serde(default = "default_plugin_sync_default_secs")]
    pub plugin_sync_default_secs: u64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            git_subprocess: false,
            autopull: default_autopull(),
            autopush: false,
            staleness_secs: default_staleness_secs(),
            pull_timeout_secs: default_pull_timeout_secs(),
            plugin_sync_default_secs: default_plugin_sync_default_secs(),
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

    /// Controls push/pull behaviour for `next sync`, autopull, and autopush.
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
            sync: SyncConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autopush_defaults_to_false() {
        assert!(!Config::default().sync.autopush);
    }

    #[test]
    fn autopush_deserializes_from_toml() {
        let cfg: Config = toml::from_str("[sync]\nautopush = true").unwrap();
        assert!(cfg.sync.autopush);
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
        // autopull / autopush defaults.
        assert!(cfg.sync.autopull);
        assert!(!cfg.sync.autopush);
        assert_eq!(cfg.sync.staleness_secs, 3600);
        assert_eq!(cfg.sync.pull_timeout_secs, 10);
    }

    #[test]
    fn sync_config_defaults_match() {
        let sync = SyncConfig::default();
        assert!(!sync.git_subprocess);
        assert!(sync.autopull);
        assert!(!sync.autopush);
        assert_eq!(sync.staleness_secs, 3600);
        assert_eq!(sync.pull_timeout_secs, 10);
    }

    #[test]
    fn sync_autopull_autopush_fields_round_trip() {
        let cfg: Config = toml::from_str(
            "[sync]\nautopull = false\nautopush = true\nstaleness_secs = 42\npull_timeout_secs = 7",
        )
        .unwrap();
        assert!(!cfg.sync.autopull);
        assert!(cfg.sync.autopush);
        assert_eq!(cfg.sync.staleness_secs, 42);
        assert_eq!(cfg.sync.pull_timeout_secs, 7);

        // Serialize back out and re-parse to confirm a full round-trip.
        let text = toml::to_string(&cfg).unwrap();
        let reparsed: Config = toml::from_str(&text).unwrap();
        assert!(!reparsed.sync.autopull);
        assert!(reparsed.sync.autopush);
        assert_eq!(reparsed.sync.staleness_secs, 42);
        assert_eq!(reparsed.sync.pull_timeout_secs, 7);
    }

    #[test]
    fn sync_partial_section_uses_defaults_for_rest() {
        let cfg: Config = toml::from_str("[sync]\nstaleness_secs = 100").unwrap();
        assert_eq!(cfg.sync.staleness_secs, 100);
        assert!(cfg.sync.autopull, "unspecified field keeps its default");
        assert!(!cfg.sync.autopush);
        assert_eq!(cfg.sync.pull_timeout_secs, 10);
    }

    #[test]
    fn autopull_accepts_old_pull_before_query_alias() {
        let cfg: Config = toml::from_str("[sync]\npull_before_query = false").unwrap();
        assert!(!cfg.sync.autopull, "old key name must still be accepted");
    }

    #[test]
    fn stale_removed_keys_do_not_break_parsing() {
        // Old top-level `autosync` and `[sync] offline` keys are silently
        // ignored rather than causing a hard parse error.
        let cfg: Config =
            toml::from_str("autosync = true\n[sync]\noffline = true\nautopush = true").unwrap();
        assert!(cfg.sync.autopush);
    }
}
