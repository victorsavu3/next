use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::domain::tag;

/// Global runtime state persisted in `state.toml` at the repository root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GlobalState {
    /// Active `@context` tags. When non-empty, tasks are filtered by these contexts.
    #[serde(default)]
    pub active_contexts: Vec<String>,

    /// Resource availability map. Key is the bare resource name (without `$`).
    /// Absent keys default to available (`true`).
    #[serde(default)]
    pub resources: HashMap<String, bool>,
}

impl GlobalState {
    /// Returns `true` when the given resource tag (with or without `$`) is available.
    /// Resources not listed in the map are considered available.
    pub fn is_resource_available(&self, resource_tag: &str) -> bool {
        let name = tag::bare_name(resource_tag);
        *self.resources.get(name).unwrap_or(&true)
    }

    /// Returns `true` when there are no active contexts (all tasks pass context filter).
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
        assert!(s.is_resource_available("$printer"));
        assert!(s.is_resource_available("printer"));
    }

    #[test]
    fn known_unavailable_resource() {
        let s = state_with_resources(&[("vacation", false)]);
        assert!(!s.is_resource_available("$vacation"));
    }

    #[test]
    fn unknown_resource_defaults_to_available() {
        let s = GlobalState::default();
        assert!(s.is_resource_available("$printer"));
    }

    #[test]
    fn no_active_contexts_by_default() {
        assert!(!GlobalState::default().any_context_active());
    }
}
