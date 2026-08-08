pub mod archive;
pub mod cached_store;
pub mod filenames;
pub mod git_backend;
pub mod lock;
pub mod machine_state;
pub(crate) mod sql_filter;
pub mod toml_store;

pub use cached_store::CachedStore;
pub use git_backend::GitBackend;
pub use lock::FileLock;
pub(crate) use machine_state::{
    load_machine_state, load_machine_state_at, update_machine_state_at,
};
pub use toml_store::TomlStore;

use std::path::{Path, PathBuf};

use crate::core::store::VcsBackend as _;
use crate::core::{
    domain::task::Task,
    error::{Result, TaskError},
};

/// Encodes a tag string to a filesystem-safe path component.
///
/// `@` context tags become `__context__<name>` and `#` resource tags become
/// `__resource__<name>`.  Slashes in the name remain real directory separators.
/// Freeform tags (no prefix) are returned unchanged.
///
/// Examples: `@home/kitchen` → `__context__home/kitchen`,
///           `#laptop/personal` → `__resource__laptop/personal`.
pub fn encode_tag_path(tag: &str) -> String {
    if let Some(rest) = tag.strip_prefix('@') {
        format!("__context__{rest}")
    } else if let Some(rest) = tag.strip_prefix('#') {
        format!("__resource__{rest}")
    } else {
        tag.to_owned()
    }
}

/// Reverses [`encode_tag_path`].
pub fn decode_tag_path(encoded: &str) -> String {
    if let Some(rest) = encoded.strip_prefix("__context__") {
        format!("@{rest}")
    } else if let Some(rest) = encoded.strip_prefix("__resource__") {
        format!("#{rest}")
    } else {
        encoded.to_owned()
    }
}

/// Opens the repository at `root` and returns a cached store and a git backend.
///
/// The TOML files in `tasks/` remain the source of truth; the SQLite cache at
/// `.next.db` is rebuilt automatically when the git HEAD changes.
///
/// State (the per-tag `tags` map and `active_users`) is stored outside
/// the repository at `state_path_for_repo(root)` so it is never synced via git.
pub fn open(root: PathBuf) -> Result<(CachedStore, GitBackend)> {
    let state_path = state_path_for_repo(&root);
    let inner = TomlStore::open(root.clone(), state_path)?;
    let vcs = GitBackend::open(&root)?;
    migrate_tag_paths(&root, &vcs)?;
    let head_hash = vcs.head_hash()?;
    let db_path = root.join(".next.db");
    let store = CachedStore::open(inner, db_path, &head_hash)?;
    Ok((store, vcs))
}

/// Renames any top-level `tags/@*` or `tags/#*` entries to percent-encoded
/// equivalents (`%40*` / `%23*`) and commits the rename so git history stays
/// consistent.  Runs at most once per repository: after the first run all
/// top-level entries already start with `%`.
fn migrate_tag_paths(root: &Path, vcs: &GitBackend) -> Result<()> {
    let tags_dir = root.join("tags");
    if !tags_dir.exists() {
        return Ok(());
    }

    let mut to_stage: Vec<PathBuf> = Vec::new();

    for entry in std::fs::read_dir(&tags_dir)? {
        let old = entry?.path();
        let name = old.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // Migrate both literal @/# (original format) and %40/%23 (intermediate format).
        if !name.starts_with('@')
            && !name.starts_with('#')
            && !name.starts_with("%40")
            && !name.starts_with("%23")
        {
            continue;
        }
        // Decode to canonical tag name, then re-encode to new format.
        let canonical = if let Some(rest) = name.strip_prefix("%40") {
            format!("@{rest}")
        } else if let Some(rest) = name.strip_prefix("%23") {
            format!("#{rest}")
        } else {
            name.to_owned()
        };
        let new_name = encode_tag_path(&canonical);
        let new = tags_dir.join(&new_name);
        if new.exists() {
            continue;
        }

        if old.is_file() {
            to_stage.push(old.clone());
            to_stage.push(new.clone());
            std::fs::rename(&old, &new).map_err(|e| {
                TaskError::Other(format!("migrate tag path {}: {e}", old.display()))
            })?;
        } else if old.is_dir() {
            let old_files = collect_toml_files(&old);
            for old_file in &old_files {
                let rel = old_file.strip_prefix(&old).unwrap();
                to_stage.push(old_file.clone());
                to_stage.push(new.join(rel));
            }
            std::fs::rename(&old, &new)
                .map_err(|e| TaskError::Other(format!("migrate tag dir {}: {e}", old.display())))?;
        }
    }

    if !to_stage.is_empty() {
        vcs.commit(
            &to_stage,
            "next: migrate tag paths to __context__/__resource__ encoding",
        )?;
    }
    Ok(())
}

fn collect_toml_files(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                result.extend(collect_toml_files(&path));
            } else if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                result.push(path);
            }
        }
    }
    result
}

/// Returns the path to the repository-level lock file under `root`.
pub fn repo_lock_path(root: &Path) -> PathBuf {
    root.join(".next.lock")
}

/// Acquires the repository-level exclusive lock for the repo rooted at `root`.
///
/// The returned guard holds the lock until dropped.  It is re-entrant within a
/// single thread, so a transaction can hold it across a read-modify-write while
/// the nested `save_task` / `commit` calls re-acquire it harmlessly.
pub fn lock_repo(root: &Path) -> Result<FileLock> {
    FileLock::acquire(&repo_lock_path(root))
}

/// Returns the advisory lock-file path co-located with `state_path`.
///
/// The lock is a hidden sibling of the state file: `.../state.toml` →
/// `.../.state.toml.lock`.  It is machine-local and never committed.
pub fn state_lock_path(state_path: &Path) -> PathBuf {
    let name = state_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("state.toml");
    let lock_name = format!(".{name}.lock");
    match state_path.parent() {
        Some(parent) => parent.join(lock_name),
        None => PathBuf::from(lock_name),
    }
}

/// Returns the path to the machine-local state lock file for the repo at `root`.
///
/// Co-located with the state file (`.state.toml.lock` next to `state.toml`).
/// This is a *separate* lock from [`repo_lock_path`]: the state file lives
/// outside the git repository, so concurrent state writes are serialised on
/// their own lock and never block (or are blocked by) repository mutations.
pub fn state_lock_path_for_repo(root: &Path) -> PathBuf {
    state_lock_path(&state_path_for_repo(root))
}

/// Acquires the exclusive state-file lock for the repo rooted at `root`.
///
/// Held across a state read-modify-write so concurrent state mutations
/// (`next context`, `resource`, `user`) cannot lose each other's updates.
/// Re-entrant within a thread, like [`lock_repo`], so the nested `get_state` /
/// `save_state` calls re-acquire it harmlessly.
pub fn lock_state(root: &Path) -> Result<FileLock> {
    let lock_path = state_lock_path_for_repo(root);
    // The per-repo state dir may not exist yet when a plugin/sync operation
    // takes this lock before `TomlStore::open` has created it; flock cannot
    // create a file in a missing directory.
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TaskError::Other(format!("create state dir: {e}")))?;
    }
    FileLock::acquire(&lock_path)
}

/// Returns the full path where `task` is (or will be) stored under `root`.
pub fn task_path(root: &Path, task: &Task) -> PathBuf {
    filenames::task_path(root, task)
}

/// Returns the path where the metadata file for `tag` is stored under `root`.
///
/// `@` / `#` prefixes are replaced with `__context__` / `__resource__` so the
/// path is safe on all platforms and renders correctly in Forgejo:
/// `@home/kitchen` → `<root>/tags/__context__home/kitchen.toml`.
pub fn tag_meta_path(root: &Path, tag: &str) -> PathBuf {
    root.join("tags")
        .join(format!("{}.toml", encode_tag_path(tag)))
}

/// Returns the path to the repository's committed scoring config,
/// `<root>/config/scoring.toml`.
pub fn scoring_path(root: &Path) -> PathBuf {
    root.join("config").join("scoring.toml")
}

/// Loads the repository's scoring weights from `<root>/config/scoring.toml`.
///
/// This file is committed to the repo (and synced), so every consumer
/// (cli/mcp/forgejo) shares the same scoring view. Returns
/// [`ScoringConfig::default`] when the file is absent; on a parse error it warns
/// and falls back to the default rather than failing to open the repository.
pub fn load_scoring(root: &Path) -> crate::core::scoring::ScoringConfig {
    use crate::core::scoring::ScoringConfig;
    let path = scoring_path(root);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ScoringConfig::default(),
    };
    match toml::from_str(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!(
                "failed to parse {}: {e}; using default scoring",
                path.display()
            );
            ScoringConfig::default()
        }
    }
}

/// Returns the path where the state file for `root` is stored.
///
/// Uses `$XDG_STATE_HOME/task-manager/<hash>/state.toml` where `<hash>` is an
/// FNV-1a hash of the canonical repository root path.  Each repository gets its
/// own isolated state directory, so multiple repos can coexist without conflict.
pub fn state_path_for_repo(root: &Path) -> PathBuf {
    state_dir_for_repo(root).join("state.toml")
}

/// Returns the per-repository machine-local directory under
/// `$XDG_STATE_HOME/task-manager/<hash>` where `<hash>` is an FNV-1a hash of the
/// canonical repository root path.  Only `state.toml` lives here now; the
/// former `plugins.toml` and `sync_state.toml` have been merged into it.
pub(crate) fn state_dir_for_repo(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hash = fnv1a_hash(&canonical.to_string_lossy());
    let base = dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/state")
        })
        .join("task-manager");
    base.join(hash)
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
