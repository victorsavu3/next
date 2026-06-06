use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

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
    /// Whether to run a staleness pull before a task-touching tool (Req A).
    pub pull_before_query: bool,
    /// How long a local copy stays "fresh" after a pull, before a query triggers one.
    pub staleness: Duration,
    /// Timeout for the pre-query pull. Stored only; not yet enforced by the core.
    pub pull_timeout: Duration,
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
    pub fn from_env() -> anyhow::Result<Self> {
        let bearer_token = std::env::var("NEXT_BEARER_TOKEN")
            .map_err(|_| anyhow::anyhow!("NEXT_BEARER_TOKEN is required"))?;
        if bearer_token.is_empty() {
            anyhow::bail!("NEXT_BEARER_TOKEN must not be empty");
        }

        let webhook_token = std::env::var("NEXT_WEBHOOK_TOKEN").ok().filter(|s| !s.is_empty());

        let repo_path = std::env::var("NEXT_REPO_PATH")
            .unwrap_or_else(|_| "/data/tasks".to_owned());
        let repo_path = PathBuf::from(repo_path);

        let bind_addr: SocketAddr = std::env::var("NEXT_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:3000".to_owned())
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid NEXT_BIND_ADDR: {e}"))?;

        let git_url = std::env::var("NEXT_GIT_URL").ok().filter(|s| !s.is_empty());
        let git_user = std::env::var("NEXT_GIT_USER").ok().filter(|s| !s.is_empty());
        let git_token = std::env::var("NEXT_GIT_TOKEN").ok().filter(|s| !s.is_empty());

        let sync_interval = match std::env::var("NEXT_SYNC_INTERVAL") {
            Ok(s) => {
                let secs: u64 = s.parse()
                    .map_err(|_| anyhow::anyhow!("NEXT_SYNC_INTERVAL must be a non-negative integer (seconds)"))?;
                if secs == 0 { None } else { Some(Duration::from_secs(secs)) }
            }
            Err(_) => Some(Duration::from_secs(86400)), // 1 day default
        };

        let deferred_sync_delay = match std::env::var("NEXT_DEFERRED_SYNC_DELAY_SECS") {
            Ok(s) => {
                let secs: u64 = s.parse().map_err(|_| {
                    anyhow::anyhow!("NEXT_DEFERRED_SYNC_DELAY_SECS must be a positive integer")
                })?;
                Duration::from_secs(secs.max(1))
            }
            Err(_) => Duration::from_secs(30),
        };

        let git_author_name  = std::env::var("NEXT_GIT_AUTHOR_NAME").ok().filter(|s| !s.is_empty());
        let git_author_email = std::env::var("NEXT_GIT_AUTHOR_EMAIL").ok().filter(|s| !s.is_empty());

        // ── Pull-before-query staleness (Req A) ──────────────────────────────
        let pull_before_query =
            parse_bool_default_true(std::env::var("NEXT_PULL_BEFORE_QUERY").ok().as_deref());
        let staleness = Duration::from_secs(parse_secs_or(
            std::env::var("NEXT_STALENESS_SECS").ok().as_deref(),
            3600,
        ));
        let pull_timeout = Duration::from_secs(parse_secs_or(
            std::env::var("NEXT_PULL_TIMEOUT_SECS").ok().as_deref(),
            10,
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
            pull_before_query,
            staleness,
            pull_timeout,
        })
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
}
