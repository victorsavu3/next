use std::{collections::HashMap, path::PathBuf};

use uuid::Uuid;

use crate::{
    domain::{state::GlobalState, tag::TagMeta, task::Task},
    error::Result,
};

/// Result of a `VcsBackend::pull` operation.
#[derive(Debug, Clone)]
pub enum PullResult {
    /// Pull completed without conflicts.
    Clean,
    /// Pull produced merge conflicts in these files; the user must resolve them.
    Conflicts(Vec<PathBuf>),
}

/// Persistent storage for tasks and global state.
///
/// A "project" is simply a task that has children via `parent_id`. There is no
/// separate project type or storage path — all tasks are stored and retrieved
/// through the same methods.
///
/// Reads use `&self`; writes use `&mut self`. Implementations may use interior
/// mutability internally (e.g. a SQLite connection behind a Mutex) but the trait
/// itself is explicit about mutability to keep the interface honest.
///
/// The canonical implementation in `next-storage` stores tasks as TOML files
/// (source of truth) backed by an SQLite cache for fast queries.
/// `next-test-utils` provides an in-memory implementation for unit tests.
pub trait Store: Send + Sync {
    // --- Tasks ---

    fn get_task(&self, id: Uuid) -> Result<Task>;

    /// Returns the task with the given user-provided slug, if any.
    /// Slugs are unique across all tasks.
    fn get_task_by_slug(&self, slug: &str) -> Result<Option<Task>>;

    /// Returns all tasks whose UUID string starts with `prefix`.
    /// Used for short-ID disambiguation in the CLI (minimum 4 hex chars).
    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>>;

    /// Returns all tasks regardless of status or stage.
    /// Filtering and scoring are applied in the domain layer.
    fn list_tasks(&self) -> Result<Vec<Task>>;

    fn save_task(&mut self, task: &Task) -> Result<()>;

    fn delete_task(&mut self, id: Uuid) -> Result<()>;

    // --- Global state ---

    fn get_state(&self) -> Result<GlobalState>;

    fn save_state(&mut self, state: &GlobalState) -> Result<()>;

    // --- Tag metadata ---
    //
    // Each tag's metadata is stored as an individual file (`tags/<tag>.toml`)
    // so that different tags can be committed and synced independently.
    // Slashes in tag names (e.g. `@home/kitchen`) map to actual subdirectories.

    /// Returns the full metadata for `tag`, or `None` if no file exists.
    fn get_tag_meta(&self, tag: &str) -> Result<Option<TagMeta>>;

    /// Writes (or replaces) the full metadata for `tag`.
    fn set_tag_meta(&mut self, tag: &str, meta: TagMeta) -> Result<()>;

    /// Deletes the metadata file for `tag`. Errors if no file exists.
    fn delete_tag_meta(&mut self, tag: &str) -> Result<()>;

    /// Returns all tag metadata as a map of tag → [`TagMeta`].
    fn list_tag_metas(&self) -> Result<HashMap<String, TagMeta>>;

    // --- Convenience wrappers for description-only operations ---

    /// Returns the description for `tag`, or `None` if not set.
    fn get_tag_description(&self, tag: &str) -> Result<Option<String>> {
        Ok(self.get_tag_meta(tag)?.and_then(|m| m.description))
    }

    /// Sets or replaces the description for `tag`, preserving other metadata.
    fn set_tag_description(&mut self, tag: &str, description: &str) -> Result<()> {
        let mut meta = self.get_tag_meta(tag)?.unwrap_or_default();
        meta.description = Some(description.to_owned());
        self.set_tag_meta(tag, meta)
    }

    /// Removes the description field for `tag`. Errors if no metadata file exists.
    fn delete_tag_description(&mut self, tag: &str) -> Result<()> {
        let mut meta = self
            .get_tag_meta(tag)?
            .ok_or_else(|| crate::error::AppError::Other(format!("no description set for tag {tag:?}")))?;
        meta.description = None;
        if meta == TagMeta::default() {
            self.delete_tag_meta(tag)
        } else {
            self.set_tag_meta(tag, meta)
        }
    }

    /// Returns all tag descriptions as a map of tag → description.
    fn list_tag_descriptions(&self) -> Result<HashMap<String, String>> {
        Ok(self
            .list_tag_metas()?
            .into_iter()
            .filter_map(|(tag, meta)| meta.description.map(|d| (tag, d)))
            .collect())
    }
}

/// Version-control backend.
///
/// Separated from `Store` so command handlers that do not perform git operations
/// (scoring, filtering, etc.) can be tested without a VCS stub.
pub trait VcsBackend: Send + Sync {
    /// Stages `paths` and creates a commit with the given message.
    fn commit(&self, paths: &[PathBuf], message: &str) -> Result<()>;

    fn pull(&self) -> Result<PullResult>;

    fn push(&self) -> Result<()>;

    /// Returns the SHA-1 hex string of the current HEAD commit.
    /// Used by the cache layer to detect whether a rebuild is needed.
    fn head_hash(&self) -> Result<String>;
}
