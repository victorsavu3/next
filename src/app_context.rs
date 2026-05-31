use std::path::{Path, PathBuf};

use anyhow::Context as _;
use crate::{Config, Store, VcsBackend};

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
    /// Returns a shared reference to the task store.
    pub fn store(&self) -> &dyn Store {
        &*self.store
    }

    /// Returns an exclusive reference to the task store.
    pub fn store_mut(&mut self) -> &mut dyn Store {
        &mut *self.store
    }

    /// Construct an application context.
    ///
    /// * `config_path` — use this config file instead of the XDG default.
    /// * `repo` — use this repository root instead of the config value or the
    ///   upward directory search.
    pub fn new(config_path: Option<&Path>, repo: Option<&Path>) -> anyhow::Result<Self> {
        let config = load_config(config_path);

        let root = if let Some(p) = repo {
            p.to_path_buf()
        } else if let Some(ref p) = config.repository {
            p.clone()
        } else {
            find_repo_root().context(
                "not inside a task repository — run `next init` to set one up, \
                 or set `repository` in the config file",
            )?
        };
        let (s, v) = crate::storage::open(root.clone())
            .context("failed to open local task store")?;
        let v = v.with_subprocess(config.sync.git_subprocess);
        let store: Box<dyn Store> = Box::new(s);
        let vcs: Box<dyn VcsBackend> = Box::new(v);
        let repo_root = root;

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

/// Loads configuration from the given path, or from
/// `$XDG_CONFIG_HOME/task-manager/config.toml` when `override_path` is `None`.
/// Returns `Config::default()` when the file is absent or unreadable.
fn load_config(override_path: Option<&Path>) -> Config {
    let path = if let Some(p) = override_path {
        Some(p.to_path_buf())
    } else {
        dirs::config_dir().map(|d| d.join("task-manager").join("config.toml"))
    };

    let Some(path) = path else {
        return Config::default();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Config::default();
    };

    match toml::from_str::<Config>(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("warning: failed to parse config file {}: {e}", path.display());
            Config::default()
        }
    }
}
