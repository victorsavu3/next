pub mod git_backend;
pub mod toml_store;

pub use git_backend::GitBackend;
pub use toml_store::TomlStore;

use std::path::{Path, PathBuf};

use next::{domain::task::Task, error::Result};

/// Opens the repository at `root` and returns a TOML-backed store and a git backend.
///
/// Creates `tasks/` if it does not already exist.
pub fn open(root: PathBuf) -> Result<(TomlStore, GitBackend)> {
    let store = TomlStore::open(root.clone())?;
    let vcs = GitBackend::open(&root)?;
    Ok((store, vcs))
}

/// Returns the full path where `task` is (or will be) stored under `root`.
pub fn task_path(root: &Path, task: &Task) -> PathBuf {
    root.join("tasks").join(TomlStore::task_filename(task))
}
