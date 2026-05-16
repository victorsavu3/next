use std::path::PathBuf;

use uuid::Uuid;

use crate::{
    domain::{
        project::Project,
        state::GlobalState,
        task::Task,
    },
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

/// Persistent storage for tasks, projects, and global state.
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

    /// Returns all tasks whose UUID string starts with `prefix`.
    /// Used for short-ID disambiguation in the CLI (minimum 4 hex chars).
    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>>;

    /// Returns all tasks regardless of status or stage.
    /// Filtering and scoring are applied in the domain layer.
    fn list_tasks(&self) -> Result<Vec<Task>>;

    fn save_task(&mut self, task: &Task) -> Result<()>;

    fn delete_task(&mut self, id: Uuid) -> Result<()>;

    /// Returns the task linked to this Forgejo issue URL, if any.
    /// Used for deduplication on re-import.
    fn get_task_by_forgejo_issue(&self, url: &str) -> Result<Option<Task>>;

    /// Returns the task linked to this iCalendar UID, if any.
    /// Used for deduplication on re-import.
    fn get_task_by_webcal_uid(&self, uid: &str) -> Result<Option<Task>>;

    // --- Projects ---

    fn get_project(&self, path: &str) -> Result<Option<Project>>;

    fn list_projects(&self) -> Result<Vec<Project>>;

    fn save_project(&mut self, project: &Project) -> Result<()>;

    // --- Global state ---

    fn get_state(&self) -> Result<GlobalState>;

    fn save_state(&mut self, state: &GlobalState) -> Result<()>;
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
