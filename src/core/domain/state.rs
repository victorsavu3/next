use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::core::domain::tag;

/// Global runtime state persisted in `state.toml` at the repository root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GlobalState {
    /// Active `@context` tags. When non-empty, tasks are filtered to only those
    /// matching at least one active context (plus context-neutral tasks).
    #[serde(default)]
    pub active_contexts: Vec<String>,

    /// Excluded `@context` tags. Tasks whose context tags match any entry here
    /// are hidden, even if they would otherwise pass the active-context filter.
    /// Context-neutral tasks (no `@` tags) are never excluded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_contexts: Vec<String>,

    /// Resource availability map. Key is the bare resource name (without `#`),
    /// optionally with `/`-separated path segments (e.g. `"office/printer"`).
    /// Absent keys default to available. If a parent is unavailable, all its
    /// descendants are implicitly unavailable too.
    #[serde(default)]
    pub resources: HashMap<String, bool>,

    /// Active user filter. When non-empty, the task list is limited to tasks
    /// assigned to one of these users, plus all unassigned tasks (which are
    /// shared across everyone). Multiple users can be active simultaneously.
    #[serde(default)]
    pub active_users: Vec<String>,
}

impl GlobalState {
    /// Returns `true` when `resource_tag` (with or without `#`) is available.
    ///
    /// Walks from the outermost ancestor down to the tag itself. If any ancestor
    /// is explicitly marked unavailable, the resource is considered unavailable.
    /// This means marking `#office` as unavailable automatically makes
    /// `#office/printer` and `#office/desk` unavailable too.
    pub fn is_resource_available(&self, resource_tag: &str) -> bool {
        for ancestor in tag::ancestors(resource_tag) {
            let name = tag::bare_name(ancestor);
            if !self.resources.get(name).copied().unwrap_or(true) {
                return false;
            }
        }
        true
    }

    /// Returns `true` when at least one context is active.
    pub fn any_context_active(&self) -> bool {
        !self.active_contexts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_resources(pairs: &[(&str, bool)]) -> GlobalState {
        GlobalState {
            resources: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn known_available_resource() {
        let s = state_with_resources(&[("printer", true)]);
        assert!(s.is_resource_available("#printer"));
    }

    #[test]
    fn known_unavailable_resource() {
        let s = state_with_resources(&[("vacation", false)]);
        assert!(!s.is_resource_available("#vacation"));
    }

    #[test]
    fn unknown_resource_defaults_to_available() {
        let s = GlobalState::default();
        assert!(s.is_resource_available("#printer"));
    }

    #[test]
    fn no_active_contexts_by_default() {
        assert!(!GlobalState::default().any_context_active());
    }

    #[test]
    fn parent_unavailable_makes_child_unavailable() {
        let s = state_with_resources(&[("office", false)]);
        assert!(!s.is_resource_available("#office/printer"));
        assert!(!s.is_resource_available("#office/desk"));
    }

    #[test]
    fn sibling_unavailability_does_not_affect_other_siblings() {
        let s = state_with_resources(&[("office/printer", false)]);
        assert!(!s.is_resource_available("#office/printer"));
        assert!(s.is_resource_available("#office/desk")); // sibling unaffected
        assert!(s.is_resource_available("#office")); // parent unaffected
    }

    #[test]
    fn grandparent_unavailable_propagates_down() {
        let s = state_with_resources(&[("office", false)]);
        assert!(!s.is_resource_available("#office/printer/color"));
    }

    #[test]
    fn child_can_be_unavailable_independently() {
        let s = state_with_resources(&[("office/printer", false), ("office", true)]);
        assert!(s.is_resource_available("#office"));
        assert!(!s.is_resource_available("#office/printer"));
    }
}
