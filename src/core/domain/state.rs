use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::core::domain::tag;

/// What the machine-local state says about tasks carrying a tag.
///
/// There is one model for every kind of tag. `@` and `#` are naming
/// conventions — they say what a tag is *for*, and the tag catalog groups by
/// them — but they do not change how a tag filters. A context is not special,
/// and a resource is not restricted to being excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagState {
    /// Tasks carrying this tag are the ones being worked on.
    Included,
    /// Tasks carrying this tag are hidden.
    Excluded,
    /// No state — and, unlike simply leaving the tag out, this *stops*
    /// inheritance from an ancestor. `@work` excluded plus `@work/urgent`
    /// defaulted hides the former and shows the latter.
    Default,
}

/// Global runtime state persisted in the machine-local `state.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GlobalState {
    /// Per-tag state. A tag with no entry inherits from its nearest ancestor
    /// that has one; a tag with no such ancestor has no state at all.
    ///
    /// Ordered so the file has a stable diff and listings need no re-sort.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, TagState>,

    /// Active user filter. When non-empty, the task list is limited to tasks
    /// assigned to one of these users, plus all unassigned tasks (which are
    /// shared across everyone). Multiple users can be active simultaneously.
    ///
    /// Not a tag: assignment is its own axis, so it keeps its own field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_users: Vec<String>,
}

impl GlobalState {
    /// The state that applies to `tag_name`, following the hierarchy.
    ///
    /// The most specific explicit entry wins, so `#office` excluded makes
    /// `#office/printer` excluded too, and an explicit [`TagState::Default`] on
    /// the child overrides the inherited exclusion. Returns `None` when neither
    /// the tag nor any ancestor has an entry.
    pub fn state_of(&self, tag_name: &str) -> Option<TagState> {
        // `ancestors` runs outermost-first; the last match is the most
        // specific, which is the one that applies.
        tag::ancestors(tag_name)
            .into_iter()
            .rev()
            .find_map(|t| self.tags.get(t).copied())
            .filter(|s| *s != TagState::Default)
    }

    /// Whether a task carrying `task_tags` passes the tag state.
    ///
    /// Two rules, applied to every tag kind alike:
    /// 1. any tag resolving to [`TagState::Excluded`] hides the task;
    /// 2. when anything at all is included, a task must carry at least one tag
    ///    resolving to [`TagState::Included`].
    ///
    /// Rule 2 is a disjunction because included tags are a set of toggles —
    /// "I am at work, or at home" — not a conjunction of requirements. A query
    /// needing conjunction spells it out with `+a +b`.
    pub fn admits(&self, task_tags: &[String]) -> bool {
        let mut included = false;
        for t in task_tags {
            match self.state_of(t) {
                Some(TagState::Excluded) => return false,
                Some(TagState::Included) => included = true,
                _ => {}
            }
        }
        included || !self.any_included()
    }

    /// Whether any tag is currently included.
    pub fn any_included(&self) -> bool {
        self.tags.values().any(|s| *s == TagState::Included)
    }

    /// Sets the state of `tag_name`, or removes the entry entirely when
    /// `state` is `None` (which re-enables inheritance from an ancestor).
    pub fn set_state(&mut self, tag_name: &str, state: Option<TagState>) {
        match state {
            Some(s) => {
                self.tags.insert(tag_name.to_owned(), s);
            }
            None => {
                self.tags.remove(tag_name);
            }
        }
    }

    /// Every tag with the given state, in tag order.
    pub fn tags_with(&self, state: TagState) -> Vec<&str> {
        self.tags
            .iter()
            .filter(|(_, s)| **s == state)
            .map(|(t, _)| t.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(pairs: &[(&str, TagState)]) -> GlobalState {
        GlobalState {
            tags: pairs.iter().map(|(t, s)| (t.to_string(), *s)).collect(),
            ..Default::default()
        }
    }

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_state_admits_everything() {
        let s = GlobalState::default();
        assert!(s.admits(&tags(&["@work"])));
        assert!(s.admits(&[]));
        assert!(!s.any_included());
    }

    #[test]
    fn exclusion_hides_regardless_of_kind() {
        for tag_name in ["@home", "#printer", "chore"] {
            let s = state(&[(tag_name, TagState::Excluded)]);
            assert!(!s.admits(&tags(&[tag_name])), "{tag_name}");
            assert!(s.admits(&tags(&["other"])), "{tag_name}");
        }
    }

    #[test]
    fn inclusion_works_for_every_kind_too() {
        // The point of unification: a resource is not exclude-only, and a
        // freeform tag is not inclusion-only.
        for tag_name in ["@work", "#laptop", "urgent"] {
            let s = state(&[(tag_name, TagState::Included)]);
            assert!(s.admits(&tags(&[tag_name])), "{tag_name}");
            assert!(!s.admits(&tags(&["other"])), "{tag_name}");
            assert!(!s.admits(&[]), "{tag_name}: untagged task is not included");
        }
    }

    #[test]
    fn included_tags_are_a_disjunction() {
        let s = state(&[("@work", TagState::Included), ("@home", TagState::Included)]);
        assert!(s.admits(&tags(&["@work"])));
        assert!(s.admits(&tags(&["@home"])));
        assert!(!s.admits(&tags(&["@errands"])));
    }

    #[test]
    fn exclusion_beats_inclusion() {
        let s = state(&[
            ("@work", TagState::Included),
            ("#printer", TagState::Excluded),
        ]);
        assert!(s.admits(&tags(&["@work"])));
        assert!(
            !s.admits(&tags(&["@work", "#printer"])),
            "an excluded tag hides the task even when another tag is included"
        );
    }

    #[test]
    fn state_is_inherited_by_descendants() {
        let s = state(&[("#office", TagState::Excluded)]);
        assert_eq!(s.state_of("#office/printer"), Some(TagState::Excluded));
        assert_eq!(
            s.state_of("#office/printer/color"),
            Some(TagState::Excluded)
        );
        assert_eq!(s.state_of("#officedesk"), None, "no slash boundary");
        assert!(!s.admits(&tags(&["#office/printer"])));
    }

    #[test]
    fn the_most_specific_entry_wins() {
        let s = state(&[
            ("@work", TagState::Excluded),
            ("@work/urgent", TagState::Included),
        ]);
        assert_eq!(s.state_of("@work"), Some(TagState::Excluded));
        assert_eq!(s.state_of("@work/urgent"), Some(TagState::Included));
        assert!(s.admits(&tags(&["@work/urgent"])));
        assert!(!s.admits(&tags(&["@work/admin"])));
    }

    #[test]
    fn explicit_default_stops_inheritance() {
        // This is why Default exists as a value rather than as an absent key:
        // absence inherits, an explicit default does not.
        let s = state(&[
            ("@work", TagState::Excluded),
            ("@work/urgent", TagState::Default),
        ]);
        assert_eq!(s.state_of("@work/urgent"), None);
        assert!(s.admits(&tags(&["@work/urgent"])));
        assert!(!s.admits(&tags(&["@work"])));
    }

    #[test]
    fn set_state_clears_with_none() {
        let mut s = state(&[("@work", TagState::Included)]);
        s.set_state("@work", None);
        assert!(s.tags.is_empty());
        assert!(s.admits(&tags(&["@anything"])));
    }

    #[test]
    fn tags_with_lists_in_order() {
        let s = state(&[
            ("@work", TagState::Included),
            ("#printer", TagState::Excluded),
            ("@home", TagState::Included),
        ]);
        assert_eq!(s.tags_with(TagState::Included), vec!["@home", "@work"]);
        assert_eq!(s.tags_with(TagState::Excluded), vec!["#printer"]);
    }
}
