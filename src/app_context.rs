//! `AppContext` — the CLI's application context.
//!
//! It is a [`TaskRepository`] (the field `repo`) plus the loaded `config.toml`.
//! **Config-file handling lives here and is CLI-only**; everything else is core
//! `TaskRepository`. Command handlers reach the repository via `ctx.repo`
//! (e.g. `ctx.repo.store`, `ctx.repo.transaction(...)`) and CLI config via
//! `ctx.config`.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::{core::TaskRepository, Config};

pub struct AppContext {
    pub config: Config,
    pub repo: TaskRepository,
}

impl AppContext {
    /// Constructs the CLI context.
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
        let (store, vcs) =
            crate::core::storage::open(root.clone()).context("failed to open local task store")?;
        let vcs = vcs.with_subprocess(config.sync.git_subprocess);
        let repo = TaskRepository::with_parts(Box::new(store), Box::new(vcs), root);

        Ok(Self { config, repo })
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
