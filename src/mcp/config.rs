use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

// ── File config (TOML) ────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct GitFileConfig {
    url: Option<String>,
    user: Option<String>,
    token: Option<String>,
    author_name: Option<String>,
    author_email: Option<String>,
    partial_clone: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct SyncFileConfig {
    /// Periodic pull+push interval in seconds; 0 = disabled.
    interval_secs: Option<u64>,
    deferred_delay_secs: Option<u64>,
    pull_before_query: Option<bool>,
    staleness_secs: Option<u64>,
    pull_timeout_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct McpFileConfig {
    bearer_token: Option<String>,
    webhook_token: Option<String>,
    repo_path: Option<PathBuf>,
    bind_addr: Option<String>,
    #[serde(default)]
    git: GitFileConfig,
    #[serde(default)]
    sync: SyncFileConfig,
}

fn default_config_path() -> PathBuf {
    PathBuf::from("/data/config/config.toml")
}

fn load_file_config(path_override: Option<PathBuf>) -> McpFileConfig {
    let path = path_override.unwrap_or_else(default_config_path);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return McpFileConfig::default();
    };
    match toml::from_str::<McpFileConfig>(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("warning: failed to parse config file {}: {e}", path.display());
            McpFileConfig::default()
        }
    }
}

// ── McpConfig ─────────────────────────────────────────────────────────────────

pub struct McpConfig {
    /// Bearer token that MCP clients must supply.
    pub bearer_token: String,
    /// Optional separate token for the `/webhook/sync` endpoint.
    pub webhook_token: Option<String>,
    /// Local path where the tasks git repository lives (or will be cloned).
    pub repo_path: PathBuf,
    /// Address to listen on.
    pub bind_addr: SocketAddr,
    /// HTTPS URL to clone from if the repo does not yet exist.
    pub git_url: Option<String>,
    /// Git username for HTTPS auth (falls back to credentials embedded in git_url).
    pub git_user: Option<String>,
    /// Git token / password for HTTPS auth.
    pub git_token: Option<String>,
    /// How often to run a background pull+push (None = disabled).
    pub sync_interval: Option<Duration>,
    /// How long to wait before firing the deferred sync after a mutation with autosync=false.
    pub deferred_sync_delay: Duration,
    /// git committer name written to the repo-local config when no identity is set.
    pub git_author_name: Option<String>,
    /// git committer email written to the repo-local config when no identity is set.
    pub git_author_email: Option<String>,
    /// Whether the first-start clone tries `git clone --filter=blob:none`
    /// (subprocess; needs a git binary). `false` goes straight to the
    /// built-in libgit2 full clone, removing the git-binary dependency.
    pub partial_clone: bool,
    /// Whether to run a staleness pull before a task-touching tool (Req A).
    pub pull_before_query: bool,
    /// How long a local copy stays "fresh" after a pull, before a query triggers one.
    pub staleness: Duration,
    /// Timeout for the pre-query pull. Stored only; not yet enforced by the core.
    pub pull_timeout: Duration,
}

/// Returns the first non-empty `Some` value from `env_var`, then `file_val`.
fn env_or_file_str(env_var: &str, file_val: Option<String>) -> Option<String> {
    std::env::var(env_var).ok().filter(|s| !s.is_empty()).or(file_val)
}

/// Parses a boolean env var that defaults to `true` and treats
/// `"0"`/`"false"`/`"no"` (case-insensitive) as `false`. Any other value is `true`.
fn parse_bool_default_true(raw: Option<&str>) -> bool {
    match raw {
        Some(s) => !matches!(s.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"),
        None => true,
    }
}

/// Parses a `u64` seconds env var, falling back to `default` when unset, empty,
/// or unparseable.
fn parse_secs_or(raw: Option<&str>, default: u64) -> u64 {
    raw.and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(default)
}

impl McpConfig {
    /// Loads configuration using env vars > config file > built-in defaults.
    ///
    /// The config file path is resolved as:
    /// 1. `NEXT_CONFIG` env var (explicit override)
    /// 2. `$XDG_CONFIG_HOME/next-mcp/config.toml` (XDG default)
    pub fn load() -> anyhow::Result<Self> {
        let config_path = std::env::var("NEXT_CONFIG").ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let file = load_file_config(config_path);

        // ── bearer_token (required from env or file) ─────────────────────────
        let bearer_token = env_or_file_str("NEXT_BEARER_TOKEN", file.bearer_token)
            .ok_or_else(|| anyhow::anyhow!(
                "NEXT_BEARER_TOKEN is required (set the env var or bearer_token in config.toml)"
            ))?;
        if bearer_token.is_empty() {
            anyhow::bail!("NEXT_BEARER_TOKEN / bearer_token must not be empty");
        }

        let webhook_token = env_or_file_str("NEXT_WEBHOOK_TOKEN", file.webhook_token);

        let repo_path = std::env::var("NEXT_REPO_PATH").ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or(file.repo_path)
            .unwrap_or_else(|| PathBuf::from("/data/tasks"));

        let bind_addr: SocketAddr = std::env::var("NEXT_BIND_ADDR").ok()
            .filter(|s| !s.is_empty())
            .or(file.bind_addr)
            .unwrap_or_else(|| "0.0.0.0:3000".to_owned())
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid bind address: {e}"))?;

        let git_url   = env_or_file_str("NEXT_GIT_URL",   file.git.url);
        let git_user  = env_or_file_str("NEXT_GIT_USER",  file.git.user);
        let git_token = env_or_file_str("NEXT_GIT_TOKEN", file.git.token);

        let git_author_name  = env_or_file_str("NEXT_GIT_AUTHOR_NAME",  file.git.author_name);
        let git_author_email = env_or_file_str("NEXT_GIT_AUTHOR_EMAIL", file.git.author_email);

        let partial_clone = match std::env::var("NEXT_GIT_PARTIAL_CLONE") {
            Ok(ref s) => parse_bool_default_true(Some(s)),
            Err(_) => file.git.partial_clone.unwrap_or(true),
        };

        // ── sync_interval ────────────────────────────────────────────────────
        let sync_interval = match std::env::var("NEXT_SYNC_INTERVAL") {
            Ok(s) => {
                let secs: u64 = s.parse()
                    .map_err(|_| anyhow::anyhow!("NEXT_SYNC_INTERVAL must be a non-negative integer (seconds)"))?;
                if secs == 0 { None } else { Some(Duration::from_secs(secs)) }
            }
            Err(_) => match file.sync.interval_secs {
                Some(0) => None,
                Some(s) => Some(Duration::from_secs(s)),
                None => Some(Duration::from_secs(86400)), // 1 day default
            },
        };

        let deferred_sync_delay = match std::env::var("NEXT_DEFERRED_SYNC_DELAY_SECS") {
            Ok(s) => {
                let secs: u64 = s.parse().map_err(|_| {
                    anyhow::anyhow!("NEXT_DEFERRED_SYNC_DELAY_SECS must be a positive integer")
                })?;
                Duration::from_secs(secs.max(1))
            }
            Err(_) => Duration::from_secs(file.sync.deferred_delay_secs.unwrap_or(30).max(1)),
        };

        // ── pull-before-query ────────────────────────────────────────────────
        let pull_before_query = match std::env::var("NEXT_PULL_BEFORE_QUERY") {
            Ok(ref s) => parse_bool_default_true(Some(s)),
            Err(_) => file.sync.pull_before_query.unwrap_or(true),
        };

        let staleness = Duration::from_secs(parse_secs_or(
            std::env::var("NEXT_STALENESS_SECS").ok().as_deref(),
            file.sync.staleness_secs.unwrap_or(3600),
        ));
        let pull_timeout = Duration::from_secs(parse_secs_or(
            std::env::var("NEXT_PULL_TIMEOUT_SECS").ok().as_deref(),
            file.sync.pull_timeout_secs.unwrap_or(10),
        ));

        Ok(Self {
            bearer_token,
            webhook_token,
            repo_path,
            bind_addr,
            git_url,
            git_user,
            git_token,
            sync_interval,
            deferred_sync_delay,
            git_author_name,
            git_author_email,
            partial_clone,
            pull_before_query,
            staleness,
            pull_timeout,
        })
    }

    /// Kept for backwards compatibility with callers that used the old name.
    #[inline]
    pub fn from_env() -> anyhow::Result<Self> {
        Self::load()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_before_query_defaults_to_true_when_unset() {
        assert!(parse_bool_default_true(None));
    }

    #[test]
    fn pull_before_query_falsey_values() {
        for v in ["0", "false", "no", "False", "NO", " false "] {
            assert!(!parse_bool_default_true(Some(v)), "expected false for {v:?}");
        }
    }

    #[test]
    fn pull_before_query_truthy_values() {
        for v in ["1", "true", "yes", "on", "anything"] {
            assert!(parse_bool_default_true(Some(v)), "expected true for {v:?}");
        }
    }

    #[test]
    fn staleness_secs_default_and_parse() {
        assert_eq!(parse_secs_or(None, 3600), 3600);
        assert_eq!(parse_secs_or(Some("7200"), 3600), 7200);
        assert_eq!(parse_secs_or(Some("0"), 3600), 0);
        // Unparseable falls back to the default.
        assert_eq!(parse_secs_or(Some("nope"), 3600), 3600);
        assert_eq!(parse_secs_or(Some(""), 3600), 3600);
    }

    #[test]
    fn pull_timeout_secs_default() {
        assert_eq!(parse_secs_or(None, 10), 10);
        assert_eq!(parse_secs_or(Some("30"), 10), 30);
    }

    #[test]
    fn file_config_parses_all_sections() {
        let toml = r#"
            bearer_token  = "tok"
            webhook_token = "wh"
            repo_path     = "/repos/tasks"
            bind_addr     = "127.0.0.1:4000"

            [git]
            url          = "https://git.example.com/tasks.git"
            user         = "alice"
            token        = "glpat-xxx"
            author_name  = "bot"
            author_email = "bot@example.com"

            [sync]
            interval_secs       = 1800
            deferred_delay_secs = 60
            pull_before_query   = false
            staleness_secs      = 7200
            pull_timeout_secs   = 20
        "#;
        let cfg: McpFileConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.bearer_token.as_deref(), Some("tok"));
        assert_eq!(cfg.webhook_token.as_deref(), Some("wh"));
        assert_eq!(cfg.repo_path, Some(PathBuf::from("/repos/tasks")));
        assert_eq!(cfg.bind_addr.as_deref(), Some("127.0.0.1:4000"));
        assert_eq!(cfg.git.url.as_deref(), Some("https://git.example.com/tasks.git"));
        assert_eq!(cfg.git.user.as_deref(), Some("alice"));
        assert_eq!(cfg.git.token.as_deref(), Some("glpat-xxx"));
        assert_eq!(cfg.git.author_name.as_deref(), Some("bot"));
        assert_eq!(cfg.git.author_email.as_deref(), Some("bot@example.com"));
        assert_eq!(cfg.sync.interval_secs, Some(1800));
        assert_eq!(cfg.sync.deferred_delay_secs, Some(60));
        assert_eq!(cfg.sync.pull_before_query, Some(false));
        assert_eq!(cfg.sync.staleness_secs, Some(7200));
        assert_eq!(cfg.sync.pull_timeout_secs, Some(20));
    }

    #[test]
    fn file_config_empty_toml_gives_defaults() {
        let cfg: McpFileConfig = toml::from_str("").unwrap();
        assert!(cfg.bearer_token.is_none());
        assert!(cfg.git.url.is_none());
        assert!(cfg.sync.interval_secs.is_none());
        assert!(cfg.sync.pull_before_query.is_none());
    }

    #[test]
    fn sync_interval_zero_in_file_disables_sync() {
        // Simulates file having interval_secs = 0 and no env override.
        let file = McpFileConfig {
            sync: SyncFileConfig { interval_secs: Some(0), ..Default::default() },
            ..Default::default()
        };
        // Mirror the interval resolution logic from McpConfig::load().
        let interval: Option<Duration> = match file.sync.interval_secs {
            Some(0) => None,
            Some(s) => Some(Duration::from_secs(s)),
            None    => Some(Duration::from_secs(86400)),
        };
        assert!(interval.is_none(), "interval_secs=0 should disable periodic sync");
    }
}
