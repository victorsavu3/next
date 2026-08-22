//! Shared repository/config bootstrap helpers.
//!
//! These are the reusable building blocks for opening a task repository and
//! loading a `Config` from a TOML file. They live in core (not the `cli`
//! feature) so every front-end — the CLI's `AppContext`, the TUI, etc. — can
//! reuse the exact same plumbing without duplicating it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;

use crate::core::progress::{NoProgress, ProgressSink};
use crate::core::TaskRepository;
use crate::Config;

/// Walks up from the current directory until a `.git` directory is found.
///
/// Returns the first ancestor (starting at the current directory) that contains
/// a `.git` entry, or `None` if none is found (or the current directory can't be
/// determined).
pub fn find_repo_root() -> Option<PathBuf> {
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

/// Reads and parses a single TOML config file into a [`Config`].
///
/// * Missing/unreadable file → [`Config::default`] (silently).
/// * Parse error → prints `warning: failed to parse config file ...` to stderr
///   and returns [`Config::default`].
pub fn parse_config_file(path: &Path) -> Config {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Config::default();
    };

    match toml::from_str::<Config>(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "warning: failed to parse config file {}: {e}",
                path.display()
            );
            Config::default()
        }
    }
}

/// Resolves the repository root.
///
/// Precedence: explicit `repo_override`, then `config.repository`, then an
/// upward search for a `.git` directory via [`find_repo_root`].
pub fn resolve_root(repo_override: Option<&Path>, config: &Config) -> anyhow::Result<PathBuf> {
    if let Some(p) = repo_override {
        Ok(p.to_path_buf())
    } else if let Some(ref p) = config.repository {
        Ok(p.clone())
    } else {
        find_repo_root().context(
            "not inside a task repository — run `next init` to set one up, \
             or set `repository` in the config file",
        )
    }
}

/// Opens the task store and git backend at `root`, applying the config's
/// git-subprocess setting, and assembles a [`TaskRepository`].
///
/// Silent: see [`open_repository_with_progress`].
pub fn open_repository(root: PathBuf, config: &Config) -> anyhow::Result<TaskRepository> {
    open_repository_with_progress(root, config, Arc::new(NoProgress))
}

/// [`open_repository`] with `sink` installed before the cache reconciles with
/// git HEAD, so the rebuild that opening a stale or cache-less repository
/// triggers is reported rather than looking like a hang.
///
/// The front-ends that own no terminal — the TUI's loader, `next-mcp`,
/// `next-forgejo` — keep calling [`open_repository`] and stay silent.
pub fn open_repository_with_progress(
    root: PathBuf,
    config: &Config,
    sink: Arc<dyn ProgressSink>,
) -> anyhow::Result<TaskRepository> {
    let (store, vcs) = crate::core::storage::open_with_progress(root.clone(), Arc::clone(&sink))
        .context("failed to open local task store")?;
    // `with_subprocess` moves the backend, sink and all.
    let vcs = vcs.with_subprocess(config.sync.git_subprocess);
    Ok(TaskRepository::with_parts(Box::new(store), Box::new(vcs), root).with_progress(sink))
}
