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
/// State (`active_contexts`, `resources`, `active_users`) is stored outside
/// the repository at `state_path_for_repo(root)` so it is never synced via git.
pub fn open(root: PathBuf) -> Result<(CachedStore, GitBackend)> {
    let state_path = state_path_for_repo(&root);
    let inner = TomlStore::open(root.clone(), state_path)?;
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

/// Returns the path where the state file for `root` is stored.
///
/// Uses `$XDG_STATE_HOME/task-manager/<hash>/state.toml` where `<hash>` is an
/// FNV-1a hash of the canonical repository root path.  Each repository gets its
/// own isolated state directory, so multiple repos can coexist without conflict.
pub fn state_path_for_repo(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hash = fnv1a_hash(&canonical.to_string_lossy());
    let base = dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/state")
        })
        .join("task-manager");
    base.join(hash).join("state.toml")
}

/// FNV-1a 64-bit hash — deterministic, no dependencies.
fn fnv1a_hash(s: &str) -> String {
    let mut hash: u64 = 14695981039346656037;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}
