use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::task::Priority;

/// Project metadata stored in `projects/<path>.toml`.
///
/// A project's `path` is its unique identifier, e.g. `"work/infra"`.
/// Nested projects are represented by `/`-separated path segments;
/// their TOML files mirror the hierarchy under the `projects/` directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Unique `/`-separated path, e.g. `"work"` or `"work/infra"`.
    pub path: String,

    pub name: String,

    #[serde(default)]
    pub priority: Priority,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    pub created_at: DateTime<Utc>,
}

impl Project {
    pub fn new(path: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            priority: Priority::Medium,
            description: None,
            created_at: Utc::now(),
        }
    }

    /// Returns `true` when `candidate` is this project or any of its descendants.
    ///
    /// `"work"` is an ancestor of `"work/infra"` and `"work/infra/deploy"`.
    pub fn contains(&self, candidate: &str) -> bool {
        candidate == self.path
            || candidate.starts_with(&format!("{}/", self.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_self() {
        let p = Project::new("work", "Work");
        assert!(p.contains("work"));
    }

    #[test]
    fn contains_direct_child() {
        let p = Project::new("work", "Work");
        assert!(p.contains("work/infra"));
    }

    #[test]
    fn contains_grandchild() {
        let p = Project::new("work", "Work");
        assert!(p.contains("work/infra/deploy"));
    }

    #[test]
    fn does_not_contain_sibling() {
        let p = Project::new("work", "Work");
        assert!(!p.contains("personal"));
    }

    #[test]
    fn does_not_match_partial_prefix() {
        // "workout" should not be a child of "work"
        let p = Project::new("work", "Work");
        assert!(!p.contains("workout"));
    }
}
