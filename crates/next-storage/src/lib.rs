pub mod git_backend;
pub mod toml_store;

pub use git_backend::GitBackend;
pub use toml_store::TomlStore;

use std::path::PathBuf;

use next::error::Result;

/// Opens the repository at `root` and returns a TOML-backed store and a git backend.
///
/// Creates `tasks/` if it does not already exist.
pub fn open(root: PathBuf) -> Result<(TomlStore, GitBackend)> {
    let store = TomlStore::open(root.clone())?;
    let vcs = GitBackend::open(&root)?;
    Ok((store, vcs))
}
