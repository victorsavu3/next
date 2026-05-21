use std::path::PathBuf;

use anyhow::Context as _;
use next::{BackendKind, Config, Store, VcsBackend};

use crate::log::Logger;

pub struct AppContext {
    pub config: Config,
    pub store: Box<dyn Store>,
    pub vcs: Box<dyn VcsBackend>,
    /// Absolute path to the repository root (contains `.git` and `state.toml`).
    pub repo_root: PathBuf,
    pub log: Logger,
}

impl AppContext {
    pub fn new() -> anyhow::Result<Self> {
        let config = load_config();

        let (store, vcs, repo_root): (Box<dyn Store>, Box<dyn VcsBackend>, PathBuf) =
            match config.backend.kind {
                BackendKind::Local => {
                    let root = find_repo_root().context(
                        "not inside a task repository — run `next init` to set one up",
                    )?;
                    let (s, v) = next_storage::open(root.clone())
                        .context("failed to open local task store")?;
                    (Box::new(s), Box::new(v), root)
                }
                BackendKind::Remote => {
                    let remote = config.backend.remote.as_ref().context(
                        "backend.kind = \"remote\" but no [backend.remote] section in config",
                    )?;
                    let root = std::env::current_dir()
                        .context("cannot determine current directory")?;
                    let store = next_remote_storage::RemoteStore::new(
                        &remote.url,
                        remote.token.as_deref(),
                    );
                    let vcs = next_remote_storage::RemoteVcs::new(
                        &remote.url,
                        remote.token.as_deref(),
                    );
                    (Box::new(store), Box::new(vcs), root)
                }
            };

        let log = Logger::new(&repo_root);
        Ok(Self {
            config,
            store,
            vcs,
            repo_root,
            log,
        })
    }
}

/// Walks up from the current directory until a `.git` directory is found.
fn find_repo_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Loads configuration from `$XDG_CONFIG_HOME/task-manager/config.toml`.
/// Returns `Config::default()` when the file is absent or unreadable.
fn load_config() -> Config {
    let path = dirs::config_dir()
        .map(|d| d.join("task-manager").join("config.toml"));

    let Some(path) = path else { return Config::default() };
    let Ok(content) = std::fs::read_to_string(&path) else { return Config::default() };

    match toml::from_str::<Config>(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("warning: failed to parse config file {}: {e}", path.display());
            Config::default()
        }
    }
}
