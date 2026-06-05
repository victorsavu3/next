//! `AppContext` — the CLI's application context.
//!
//! It is a [`TaskRepository`] (the field `repo`) plus the loaded `config.toml`.
//! **Config-file handling lives here and is CLI-only**; everything else is core
//! `TaskRepository`. Command handlers reach the repository via `ctx.repo`
//! (e.g. `ctx.repo.store`, `ctx.repo.transaction(...)`) and CLI config via
//! `ctx.config`.

use std::path::{Path, PathBuf};

use crate::core::bootstrap;
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
        let root = bootstrap::resolve_root(repo, &config)?;
        let repo = bootstrap::open_repository(root, &config)?;
        Ok(Self { config, repo })
    }
}

/// The CLI's default config-file location:
/// `$XDG_CONFIG_HOME/task-manager/config.toml`.
fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("task-manager").join("config.toml"))
}

/// Loads configuration from the given path, or from
/// `$XDG_CONFIG_HOME/task-manager/config.toml` when `override_path` is `None`.
/// Returns `Config::default()` when the file is absent or unreadable.
fn load_config(override_path: Option<&Path>) -> Config {
    let path = match override_path {
        Some(p) => Some(p.to_path_buf()),
        None => default_config_path(),
    };

    let Some(path) = path else {
        return Config::default();
    };
    bootstrap::parse_config_file(&path)
}
