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

        Ok(Self {
            bearer_token,
            webhook_token,
            repo_path,
            bind_addr,
            git_url,
            git_user,
            git_token,
            sync_interval,
        })
    }
}
