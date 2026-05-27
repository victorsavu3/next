pub mod cached_store;
pub mod git_backend;
pub mod toml_store;

pub use cached_store::CachedStore;
pub use git_backend::GitBackend;
pub use toml_store::TomlStore;

use std::path::{Path, PathBuf};

use crate::{domain::task::Task, error::Result};
use crate::store::VcsBackend as _;

/// Opens the repository at `root` and returns a cached store and a git backend.
///
/// The TOML files in `tasks/` remain the source of truth; the SQLite cache at
/// `.next.db` is rebuilt automatically when the git HEAD changes.
///
/// Creates `tasks/` if it does not already exist.
pub fn open(root: PathBuf) -> Result<(CachedStore, GitBackend)> {
    let inner = TomlStore::open(root.clone())?;
    let vcs = GitBackend::open(&root)?;
    let head_hash = vcs.head_hash()?;
    let db_path = root.join(".next.db");
    let store = CachedStore::open(inner, db_path, &head_hash)?;
    Ok((store, vcs))
}

/// Returns the full path where `task` is (or will be) stored under `root`.
pub fn task_path(root: &Path, task: &Task) -> PathBuf {
    root.join("tasks").join(TomlStore::task_filename(task))
}

/// Returns the path where the description for `tag` is stored under `root`.
///
/// The tag string maps directly to a path: `@home/kitchen` →
/// `<root>/tags/@home/kitchen.toml`.  Slashes in the tag name become real
/// directory separators, so hierarchical tags form a natural directory tree.
pub fn tag_description_path(root: &Path, tag: &str) -> PathBuf {
    root.join("tags").join(format!("{tag}.toml"))
}
