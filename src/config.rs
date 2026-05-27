use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Per-field default functions ───────────────────────────────────────────────
// Defined once; used both by `#[serde(default = "...")]` and `Default` impl.

fn default_due_overdue_base() -> f64 { 12.0 }
fn default_due_overdue_per_day() -> f64 { 0.3 }
fn default_due_week_base() -> f64 { 6.0 }
fn default_due_week_per_day() -> f64 { 0.8 }
fn default_due_month_base() -> f64 { 3.0 }
fn default_due_month_per_day() -> f64 { 0.1 }
fn default_priority_low() -> f64 { 0.0 }
fn default_priority_medium() -> f64 { 1.0 }
fn default_priority_high() -> f64 { 2.0 }
fn default_project_low() -> f64 { -0.5 }
fn default_project_medium() -> f64 { 0.0 }
fn default_project_high() -> f64 { 0.5 }
fn default_age_per_day() -> f64 { 0.01 }
fn default_age_max() -> f64 { 2.0 }
fn default_forecast_horizon_days() -> u32 { 90 }
fn default_next_count() -> usize { 10 }

// ── ScoringConfig ─────────────────────────────────────────────────────────────

/// Weights used in the urgency scoring formula.
///
/// All fields are optional in the config file; any omitted field keeps its
/// default value, so a minimal `[scoring]` section only needs to list the
/// overrides (e.g. `priority_high = 3.0`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringConfig {
    // Due-date factor
    #[serde(default = "default_due_overdue_base")]
    pub due_overdue_base: f64,
    #[serde(default = "default_due_overdue_per_day")]
    pub due_overdue_per_day: f64,
    #[serde(default = "default_due_week_base")]
    pub due_week_base: f64,
    #[serde(default = "default_due_week_per_day")]
    pub due_week_per_day: f64,
    #[serde(default = "default_due_month_base")]
    pub due_month_base: f64,
    #[serde(default = "default_due_month_per_day")]
    pub due_month_per_day: f64,

    // Priority factor
    #[serde(default = "default_priority_low")]
    pub priority_low: f64,
    #[serde(default = "default_priority_medium")]
    pub priority_medium: f64,
    #[serde(default = "default_priority_high")]
    pub priority_high: f64,

    // Project-priority offset (based on parent task priority)
    #[serde(default = "default_project_low")]
    pub project_low: f64,
    #[serde(default = "default_project_medium")]
    pub project_medium: f64,
    #[serde(default = "default_project_high")]
    pub project_high: f64,

    // Age factor (per day, capped at `age_max`)
    #[serde(default = "default_age_per_day")]
    pub age_per_day: f64,
    #[serde(default = "default_age_max")]
    pub age_max: f64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            due_overdue_base: default_due_overdue_base(),
            due_overdue_per_day: default_due_overdue_per_day(),
            due_week_base: default_due_week_base(),
            due_week_per_day: default_due_week_per_day(),
            due_month_base: default_due_month_base(),
            due_month_per_day: default_due_month_per_day(),
            priority_low: default_priority_low(),
            priority_medium: default_priority_medium(),
            priority_high: default_priority_high(),
            project_low: default_project_low(),
            project_medium: default_project_medium(),
            project_high: default_project_high(),
            age_per_day: default_age_per_day(),
            age_max: default_age_max(),
        }
    }
}

// ── BackendConfig ─────────────────────────────────────────────────────────────

/// Which storage backend to use.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// Local TOML files backed by a local git repository (default).
    #[default]
    Local,
    /// Tasks stored on a remote `next-server` instance over HTTP.
    Remote,
}

/// Connection settings for the remote HTTP backend.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteBackendConfig {
    /// Base URL of the `next-server` instance, e.g. `https://tasks.example.com`.
    #[serde(default)]
    pub url: String,

    /// Bearer token for authentication (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Selects the storage backend and, for the remote backend, its connection details.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackendConfig {
    /// Which storage implementation to use.
    #[serde(default)]
    pub kind: BackendKind,

    /// Connection settings; required when `kind = "remote"`.
    #[serde(default)]
    pub remote: Option<RemoteBackendConfig>,
}

// ── ForgejoConfig ─────────────────────────────────────────────────────────────

/// Connection settings for the Forgejo integration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForgejoConfig {
    /// Base URL of the Forgejo instance, e.g. `https://forgejo.example.com`.
    #[serde(default)]
    pub base_url: String,

    /// Personal access token with read/write access to issues.
    #[serde(default)]
    pub token: String,
}

// ── Config ────────────────────────────────────────────────────────────────────

/// Application-wide configuration.
///
/// Loaded from `$XDG_CONFIG_HOME/task-manager/config.toml`.
/// All fields are optional; missing fields use the values shown in
/// [`Config::default`].
///
/// Example config file (local backend, default):
/// ```toml
/// repository = "/home/alice/tasks"   # use next from any directory
/// forecast_horizon_days = 60
/// next_count = 5
///
/// [forgejo]
/// base_url = "https://forgejo.example.com"
/// token    = "my-secret-token"
///
/// [scoring]
/// priority_high = 3.0          # only override what you want to change
/// age_max       = 3.0
/// ```
///
/// Example config file (remote backend):
/// ```toml
/// [backend]
/// kind = "remote"
///
/// [backend.remote]
/// url   = "https://tasks.example.com"
/// token = "my-bearer-token"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Storage backend selection (local TOML+git or remote HTTP server).
    #[serde(default)]
    pub backend: BackendConfig,

    /// Urgency scoring weights.
    #[serde(default)]
    pub scoring: ScoringConfig,

    /// Forgejo integration settings.
    #[serde(default)]
    pub forgejo: ForgejoConfig,

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: BackendConfig::default(),
            scoring: ScoringConfig::default(),
            forgejo: ForgejoConfig::default(),
            forecast_horizon_days: default_forecast_horizon_days(),
            next_count: default_next_count(),
            list_limit: None,
            repository: None,
        }
    }
}
