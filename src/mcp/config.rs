use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use serde::Deserialize;

// ── File config (TOML) ────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct GitFileConfig {
    url: Option<String>,
    user: Option<String>,
    /// Inline token. Mutually exclusive with `token_file` / `token_env`.
    token: Option<String>,
    /// Path to a file whose contents are the token (e.g. a podman secret at
    /// `/run/secrets/next_git_token`).
    token_file: Option<PathBuf>,
    /// Name of an environment variable holding the token.
    token_env: Option<String>,
    author_name: Option<String>,
    author_email: Option<String>,
    partial_clone: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct SyncFileConfig {
    /// Periodic pull+push interval in seconds; 0 = disabled.
    interval_secs: Option<u64>,
    deferred_delay_secs: Option<u64>,
    /// Staleness pull before a task-touching tool. Accepts the old key name
    /// `pull_before_query` as an alias.
    #[serde(alias = "pull_before_query")]
    autopull: Option<bool>,
    staleness_secs: Option<u64>,
    pull_timeout_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct McpFileConfig {
    /// Inline bearer token. Mutually exclusive with the `_file` / `_env` forms.
    bearer_token: Option<String>,
    bearer_token_file: Option<PathBuf>,
    bearer_token_env: Option<String>,
    webhook_token: Option<String>,
    webhook_token_file: Option<PathBuf>,
    webhook_token_env: Option<String>,
    repo_path: Option<PathBuf>,
    bind_addr: Option<String>,
    #[serde(default)]
    git: GitFileConfig,
    #[serde(default)]
    sync: SyncFileConfig,
}

/// Default config file path: `$XDG_CONFIG_HOME/next-mcp/config.toml`.
///
/// The container image sets `XDG_CONFIG_HOME=/data/config`, so this resolves to
/// `/data/config/next-mcp/config.toml` there. Mirrors `forgejo::config`.
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("next-mcp")
        .join("config.toml")
}

/// Resolves the config-file path: `--config` (via `cli`) beats `NEXT_MCP_CONFIG`,
/// which beats the deprecated `NEXT_CONFIG` alias, which beats [`config_path`].
///
/// `get_env` is injected so the precedence is unit-testable without touching the
/// process environment.
fn resolve_config_path(cli: Option<PathBuf>, get_env: impl Fn(&str) -> Option<String>) -> PathBuf {
    let from_env = |key: &str| get_env(key).filter(|s| !s.is_empty()).map(PathBuf::from);
    cli.or_else(|| from_env("NEXT_MCP_CONFIG"))
        .or_else(|| from_env("NEXT_CONFIG"))
        .unwrap_or_else(config_path)
}

fn load_file_config(path: &Path) -> McpFileConfig {
    let Ok(content) = std::fs::read_to_string(path) else {
        // A missing file is not an error: every value has an env var or a
        // built-in default. Only a present-but-unparseable file warrants a note.
        return McpFileConfig::default();
    };
    match toml::from_str::<McpFileConfig>(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "warning: failed to parse config file {}: {e}",
                path.display()
            );
            McpFileConfig::default()
        }
    }
}

/// Resolves a secret with the legacy `NEXT_*` env var taking precedence.
///
/// When `env_var` is set and non-empty it wins outright — the config's secret
/// forms are neither read nor validated, so an existing env-based deployment
/// keeps working even if the config carries a stale or ambiguous `*_file`
/// reference (ENV > CONFIG). Otherwise the value is resolved from the file forms
/// via [`resolve_secret`].
fn secret_with_env_override(
    env_var: &str,
    name: &str,
    inline: Option<String>,
    file: Option<PathBuf>,
    env_ref: Option<String>,
    config_path: &Path,
) -> anyhow::Result<Option<String>> {
    if let Some(v) = std::env::var(env_var).ok().filter(|s| !s.is_empty()) {
        return Ok(Some(v));
    }
    resolve_secret(name, inline, file, env_ref, config_path)
}

/// Resolves a single secret from its three mutually-exclusive config forms.
///
/// At most one of the inline value, a `*_file` path, or a `*_env` variable name
/// may be set; more than one is a hard error qualified with the config path.
/// Returns `None` when the config sets none of them.
fn resolve_secret(
    name: &str,
    inline: Option<String>,
    file: Option<PathBuf>,
    env_ref: Option<String>,
    config_path: &Path,
) -> anyhow::Result<Option<String>> {
    let sources =
        u8::from(inline.is_some()) + u8::from(file.is_some()) + u8::from(env_ref.is_some());
    if sources > 1 {
        anyhow::bail!(
            "at most one of `{name}`, `{name}_file`, `{name}_env` may be set (in {})",
            config_path.display()
        );
    }
    if let Some(v) = inline {
        return Ok(Some(v));
    }
    if let Some(path) = file {
        let raw = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "reading `{name}_file` {} (referenced from {})",
                path.display(),
                config_path.display()
            )
        })?;
        // Secret files (podman secrets, `echo secret > file`) commonly carry a
        // trailing newline that is not part of the token.
        return Ok(Some(raw.trim_end_matches(['\n', '\r']).to_string()));
    }
    if let Some(var) = env_ref {
        let val = std::env::var(&var).map_err(|_| {
            anyhow::anyhow!(
                "`{name}_env` names environment variable `{var}`, which is unset \
                 (referenced from {})",
                config_path.display()
            )
        })?;
        return Ok(Some(val));
    }
    Ok(None)
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
    pub autopull: bool,
    /// How long a local copy stays "fresh" after a pull, before a query triggers one.
    pub staleness: Duration,
    /// Timeout for the pre-query pull. Stored only; not yet enforced by the core.
    pub pull_timeout: Duration,
}

/// Returns the first non-empty `Some` value from `env_var`, then `file_val`.
fn env_or_file_str(env_var: &str, file_val: Option<String>) -> Option<String> {
    std::env::var(env_var)
        .ok()
        .filter(|s| !s.is_empty())
        .or(file_val)
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
    raw.and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

impl McpConfig {
    /// Loads configuration using env vars > config file > built-in defaults.
    ///
    /// The config file path is resolved as (highest precedence first):
    /// 1. `cli_config_path` (the `--config <path>` flag)
    /// 2. the `NEXT_MCP_CONFIG` env var (`NEXT_CONFIG` is a deprecated alias)
    /// 3. `$XDG_CONFIG_HOME/next-mcp/config.toml` (see [`config_path`])
    ///
    /// Secrets (`bearer_token`, `webhook_token`, `git.token`) may be given in the
    /// file as an inline value, a `*_file` path, or a `*_env` variable name
    /// (exactly one form each). The legacy `NEXT_*` env vars still override
    /// whatever the file resolves to.
    pub fn load(cli_config_path: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = resolve_config_path(cli_config_path, |k| std::env::var(k).ok());
        let file = load_file_config(&path);

        // ── bearer_token (required from env or file) ─────────────────────────
        let bearer_token = secret_with_env_override(
            "NEXT_BEARER_TOKEN",
            "bearer_token",
            file.bearer_token,
            file.bearer_token_file,
            file.bearer_token_env,
            &path,
        )?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "NEXT_BEARER_TOKEN is required (set the env var, or bearer_token/\
             bearer_token_file/bearer_token_env in config.toml)"
            )
        })?;
        if bearer_token.is_empty() {
            anyhow::bail!("NEXT_BEARER_TOKEN / bearer_token must not be empty");
        }

        let webhook_token = secret_with_env_override(
            "NEXT_WEBHOOK_TOKEN",
            "webhook_token",
            file.webhook_token,
            file.webhook_token_file,
            file.webhook_token_env,
            &path,
        )?;

        let repo_path = std::env::var("NEXT_REPO_PATH")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or(file.repo_path)
            .unwrap_or_else(|| PathBuf::from("/data/tasks"));

        let bind_addr: SocketAddr = std::env::var("NEXT_BIND_ADDR")
            .ok()
            .filter(|s| !s.is_empty())
            .or(file.bind_addr)
            .unwrap_or_else(|| "0.0.0.0:3000".to_owned())
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid bind address: {e}"))?;

        let git_url = env_or_file_str("NEXT_GIT_URL", file.git.url);
        let git_user = env_or_file_str("NEXT_GIT_USER", file.git.user);
        let git_token = secret_with_env_override(
            "NEXT_GIT_TOKEN",
            "git.token",
            file.git.token,
            file.git.token_file,
            file.git.token_env,
            &path,
        )?;

        let git_author_name = env_or_file_str("NEXT_GIT_AUTHOR_NAME", file.git.author_name);
        let git_author_email = env_or_file_str("NEXT_GIT_AUTHOR_EMAIL", file.git.author_email);

        let partial_clone = match std::env::var("NEXT_GIT_PARTIAL_CLONE") {
            Ok(ref s) => parse_bool_default_true(Some(s)),
            Err(_) => file.git.partial_clone.unwrap_or(true),
        };

        // ── sync_interval ────────────────────────────────────────────────────
        let sync_interval = match std::env::var("NEXT_SYNC_INTERVAL") {
            Ok(s) => {
                let secs: u64 = s.parse().map_err(|_| {
                    anyhow::anyhow!("NEXT_SYNC_INTERVAL must be a non-negative integer (seconds)")
                })?;
                if secs == 0 {
                    None
                } else {
                    Some(Duration::from_secs(secs))
                }
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

        // ── autopull (staleness pull before a query) ─────────────────────────
        // `NEXT_AUTOPULL` is the current env var; `NEXT_PULL_BEFORE_QUERY` is
        // still read as a fallback alias for older deployments.
        let autopull = match std::env::var("NEXT_AUTOPULL")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("NEXT_PULL_BEFORE_QUERY")
                    .ok()
                    .filter(|s| !s.is_empty())
            }) {
            Some(ref s) => parse_bool_default_true(Some(s)),
            None => file.sync.autopull.unwrap_or(true),
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
            autopull,
            staleness,
            pull_timeout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autopull_defaults_to_true_when_unset() {
        assert!(parse_bool_default_true(None));
    }

    #[test]
    fn autopull_falsey_values() {
        for v in ["0", "false", "no", "False", "NO", " false "] {
            assert!(
                !parse_bool_default_true(Some(v)),
                "expected false for {v:?}"
            );
        }
    }

    #[test]
    fn autopull_truthy_values() {
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
            autopull            = false
            staleness_secs      = 7200
            pull_timeout_secs   = 20
        "#;
        let cfg: McpFileConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.bearer_token.as_deref(), Some("tok"));
        assert_eq!(cfg.webhook_token.as_deref(), Some("wh"));
        assert_eq!(cfg.repo_path, Some(PathBuf::from("/repos/tasks")));
        assert_eq!(cfg.bind_addr.as_deref(), Some("127.0.0.1:4000"));
        assert_eq!(
            cfg.git.url.as_deref(),
            Some("https://git.example.com/tasks.git")
        );
        assert_eq!(cfg.git.user.as_deref(), Some("alice"));
        assert_eq!(cfg.git.token.as_deref(), Some("glpat-xxx"));
        assert_eq!(cfg.git.author_name.as_deref(), Some("bot"));
        assert_eq!(cfg.git.author_email.as_deref(), Some("bot@example.com"));
        assert_eq!(cfg.sync.interval_secs, Some(1800));
        assert_eq!(cfg.sync.deferred_delay_secs, Some(60));
        assert_eq!(cfg.sync.autopull, Some(false));
        assert_eq!(cfg.sync.staleness_secs, Some(7200));
        assert_eq!(cfg.sync.pull_timeout_secs, Some(20));
    }

    #[test]
    fn file_config_parses_secret_ref_forms() {
        let toml = r#"
            bearer_token_file  = "/run/secrets/bearer"
            webhook_token_env  = "WEBHOOK_TOK"

            [git]
            token_file = "/run/secrets/git"
        "#;
        let cfg: McpFileConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.bearer_token_file,
            Some(PathBuf::from("/run/secrets/bearer"))
        );
        assert_eq!(cfg.webhook_token_env.as_deref(), Some("WEBHOOK_TOK"));
        assert_eq!(cfg.git.token_file, Some(PathBuf::from("/run/secrets/git")));
        assert!(cfg.bearer_token.is_none());
    }

    #[test]
    fn file_config_empty_toml_gives_defaults() {
        let cfg: McpFileConfig = toml::from_str("").unwrap();
        assert!(cfg.bearer_token.is_none());
        assert!(cfg.git.url.is_none());
        assert!(cfg.sync.interval_secs.is_none());
        assert!(cfg.sync.autopull.is_none());
    }

    #[test]
    fn autopull_accepts_old_pull_before_query_alias() {
        let cfg: McpFileConfig = toml::from_str("[sync]\npull_before_query = false").unwrap();
        assert_eq!(
            cfg.sync.autopull,
            Some(false),
            "old key name must still parse"
        );
    }

    #[test]
    fn sync_interval_zero_in_file_disables_sync() {
        // Simulates file having interval_secs = 0 and no env override.
        let file = McpFileConfig {
            sync: SyncFileConfig {
                interval_secs: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        // Mirror the interval resolution logic from McpConfig::load().
        let interval: Option<Duration> = match file.sync.interval_secs {
            Some(0) => None,
            Some(s) => Some(Duration::from_secs(s)),
            None => Some(Duration::from_secs(86400)),
        };
        assert!(
            interval.is_none(),
            "interval_secs=0 should disable periodic sync"
        );
    }

    // ── path discovery ────────────────────────────────────────────────────────

    #[test]
    fn config_path_precedence_cli_over_env_over_default() {
        let env = |vars: &[(&str, &str)]| {
            let owned: Vec<(String, String)> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            move |k: &str| owned.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.clone())
        };

        // CLI flag wins over everything.
        assert_eq!(
            resolve_config_path(
                Some(PathBuf::from("/cli.toml")),
                env(&[
                    ("NEXT_MCP_CONFIG", "/mcp.toml"),
                    ("NEXT_CONFIG", "/old.toml")
                ]),
            ),
            PathBuf::from("/cli.toml")
        );
        // NEXT_MCP_CONFIG wins over the deprecated NEXT_CONFIG.
        assert_eq!(
            resolve_config_path(
                None,
                env(&[
                    ("NEXT_MCP_CONFIG", "/mcp.toml"),
                    ("NEXT_CONFIG", "/old.toml")
                ]),
            ),
            PathBuf::from("/mcp.toml")
        );
        // Deprecated alias still honoured when it is the only one set.
        assert_eq!(
            resolve_config_path(None, env(&[("NEXT_CONFIG", "/old.toml")])),
            PathBuf::from("/old.toml")
        );
        // Empty env values are ignored, falling through to the default.
        assert_eq!(
            resolve_config_path(None, env(&[("NEXT_MCP_CONFIG", "")])),
            config_path()
        );
        // Nothing set → built-in default.
        assert_eq!(resolve_config_path(None, env(&[])), config_path());
    }

    #[test]
    fn config_path_default_ends_with_next_mcp() {
        let p = config_path();
        assert!(
            p.ends_with("next-mcp/config.toml"),
            "unexpected default path: {}",
            p.display()
        );
    }

    #[test]
    fn missing_file_yields_default_config() {
        let cfg = load_file_config(Path::new("/nonexistent/does-not-exist.toml"));
        assert!(cfg.bearer_token.is_none());
        assert!(cfg.git.url.is_none());
    }

    // ── secret resolution ─────────────────────────────────────────────────────

    #[test]
    fn secret_inline_is_returned() {
        let got = resolve_secret(
            "bearer_token",
            Some("inline-tok".into()),
            None,
            None,
            Path::new("/cfg.toml"),
        )
        .unwrap();
        assert_eq!(got.as_deref(), Some("inline-tok"));
    }

    #[test]
    fn secret_none_when_no_source() {
        let got = resolve_secret("bearer_token", None, None, None, Path::new("/cfg.toml")).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn secret_file_is_read_and_trimmed() {
        let dir = tempfile::TempDir::new().unwrap();
        let secret = dir.path().join("bearer");
        std::fs::write(&secret, "file-tok\n").unwrap();
        let got = resolve_secret(
            "bearer_token",
            None,
            Some(secret),
            None,
            Path::new("/cfg.toml"),
        )
        .unwrap();
        assert_eq!(
            got.as_deref(),
            Some("file-tok"),
            "trailing newline must be stripped"
        );
    }

    #[test]
    fn secret_file_missing_errors_with_context() {
        let err = resolve_secret(
            "bearer_token",
            None,
            Some(PathBuf::from("/nope/secret")),
            None,
            Path::new("/cfg.toml"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("bearer_token_file"),
            "error should name the field: {err}"
        );
        assert!(
            err.contains("/cfg.toml"),
            "error should qualify with the config path: {err}"
        );
    }

    #[test]
    fn secret_env_ref_is_read() {
        // A unique var name so parallel tests don't collide.
        let var = "NEXT_TEST_SECRET_ENV_REF_UNIQUE";
        std::env::set_var(var, "env-ref-tok");
        let got = resolve_secret(
            "bearer_token",
            None,
            None,
            Some(var.to_string()),
            Path::new("/cfg.toml"),
        )
        .unwrap();
        std::env::remove_var(var);
        assert_eq!(got.as_deref(), Some("env-ref-tok"));
    }

    #[test]
    fn secret_env_ref_unset_errors() {
        let err = resolve_secret(
            "bearer_token",
            None,
            None,
            Some("NEXT_TEST_DEFINITELY_UNSET_VAR_XYZ".into()),
            Path::new("/cfg.toml"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("is unset"),
            "error should say the var is unset: {err}"
        );
        assert!(
            err.contains("bearer_token_env"),
            "error should name the field: {err}"
        );
    }

    #[test]
    fn secret_multiple_sources_conflict() {
        let err = resolve_secret(
            "bearer_token",
            Some("inline".into()),
            Some(PathBuf::from("/run/secrets/x")),
            None,
            Path::new("/cfg.toml"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("at most one"),
            "conflict should be reported: {err}"
        );
        assert!(
            err.contains("bearer_token"),
            "error should name the secret: {err}"
        );
    }

    #[test]
    fn secret_env_override_short_circuits_conflicting_file_forms() {
        // With the legacy env var set, an otherwise-fatal file conflict (and a
        // missing *_file path) must be ignored — env wins outright.
        let var = "NEXT_TEST_BEARER_OVERRIDE_UNIQUE";
        std::env::set_var(var, "env-token");
        let got = secret_with_env_override(
            var,
            "bearer_token",
            Some("inline".into()),
            Some(PathBuf::from("/nope/missing")),
            None,
            Path::new("/cfg.toml"),
        );
        std::env::remove_var(var);
        assert_eq!(got.unwrap().as_deref(), Some("env-token"));
    }

    #[test]
    fn secret_env_override_falls_through_to_file_when_unset() {
        let got = secret_with_env_override(
            "NEXT_TEST_BEARER_UNSET_UNIQUE",
            "bearer_token",
            Some("from-file".into()),
            None,
            None,
            Path::new("/cfg.toml"),
        )
        .unwrap();
        assert_eq!(got.as_deref(), Some("from-file"));
    }

    #[test]
    fn env_or_file_str_prefers_env_over_file() {
        // Precedence ENV > CONFIG: with the env var set, the file value loses.
        let var = "NEXT_TEST_ENV_OVER_FILE_UNIQUE";
        std::env::set_var(var, "from-env");
        let got = env_or_file_str(var, Some("from-file".into()));
        std::env::remove_var(var);
        assert_eq!(got.as_deref(), Some("from-env"));
        // With no env var, the file (CONFIG) value is used.
        assert_eq!(
            env_or_file_str("NEXT_TEST_ENV_OVER_FILE_UNSET", Some("from-file".into())).as_deref(),
            Some("from-file")
        );
    }
}
