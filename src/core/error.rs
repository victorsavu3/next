use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, TaskError>;

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("invalid date expression '{0}': {1}")]
    InvalidDate(String, String),

    #[error("task not found: {0}")]
    TaskNotFound(String),

    /// Returned when a short ID prefix matches more than one task.
    #[error("ambiguous task ID prefix '{0}': matches {1} tasks")]
    AmbiguousId(String, usize),

    /// Returned when a slug is already used by another task.
    #[error("slug '{0}' is already taken by another task")]
    SlugConflict(String),

    #[error("git conflict in files: {0:?}")]
    GitConflict(Vec<PathBuf>),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
