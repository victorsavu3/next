use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::core::domain::tag;

/// What the machine-local state says about tasks carrying a tag.
///
/// Each name states its own rule, because the rules are not symmetric and a
/// vaguer word (the old `Included` / `Default`) left the reader guessing which
/// one applied.
///
/// There is one model for every kind of tag. `@` and `#` are naming
/// conventions — they say what a tag is *for*, and the tag catalog groups by
/// them — but they do not change how a tag filters. A context is not special,
/// and a resource is not restricted to being excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagState {
    /// While *anything* is required, a task must carry one of the required tags
    /// to be listed. Requiring `@work` therefore hides everything else,
    /// including untagged tasks — which is what makes it a filter rather than
    /// the additive whitelist the old name `Included` suggested.
    #[serde(alias = "included")]
    Required,
    /// Tasks carrying this tag are hidden. Exclusion beats requirement.
    Excluded,
    /// Neither required nor hidden — and, unlike simply leaving the tag out,
    /// this *stops* inheritance from an ancestor. `@work` excluded plus
    /// `@work/urgent` accepted hides the former and shows the latter. That is
    /// the whole reason it is a value rather than an absent key, and why the
    /// old name `Default` was actively misleading: absence is not what it means.
    #[serde(alias = "default")]
    Accepted,
}

/// Global runtime state persisted in the machine-local `state.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GlobalState {
    /// Per-tag state. A tag with no entry inherits from its nearest ancestor
    /// that has one; a tag with no such ancestor has no state at all.
    ///
    /// Ordered so the file has a stable diff and listings need no re-sort.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "tolerant_tag_states"
    )]
    pub tags: BTreeMap<String, TagState>,

    /// Active user filter. When non-empty, the task list is limited to tasks
    /// assigned to one of these users, plus all unassigned tasks (which are
    /// shared across everyone). Multiple users can be active simultaneously.
    ///
    /// Not a tag: assignment is its own axis, so it keeps its own field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_users: Vec<String>,
}

/// Reads the tag-state map, dropping entries whose value this build does not
/// recognise instead of failing the whole file.
///
/// `state.toml` is machine-local and rewritten on every state change, so one
/// unreadable entry is worth losing. Failing the parse is not: the file is read
/// on *every* command, so a value written by a newer build — or left by a
/// rename that did not keep an alias — would make the tool unusable rather than
/// merely forgetful.
fn tolerant_tag_states<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, TagState>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: BTreeMap<String, toml::Value> = BTreeMap::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(
            |(tag_name, value)| match TagState::deserialize(value.clone()) {
                Ok(state) => Some((tag_name, state)),
                Err(_) => {
                    tracing::warn!(
                        tag = %tag_name,
                        value = %value,
                        "dropping unrecognised tag state from state.toml"
                    );
                    None
                }
            },
        )
        .collect())
}

impl GlobalState {
    /// The state that applies to `tag_name`, following the hierarchy.
    ///
    /// The most specific explicit entry wins, so `#office` excluded makes
    /// `#office/printer` excluded too, and an explicit [`TagState::Accepted`] on
    /// the child overrides the inherited exclusion. Returns `None` when neither
    /// the tag nor any ancestor has an entry.
    pub fn state_of(&self, tag_name: &str) -> Option<TagState> {
        // `ancestors` runs outermost-first; the last match is the most
        // specific, which is the one that applies.
        tag::ancestors(tag_name)
            .into_iter()
            .rev()
            .find_map(|t| self.tags.get(t).copied())
            .filter(|s| *s != TagState::Accepted)
    }

    /// Whether a task carrying `task_tags` passes the tag state.
    ///
    /// Two rules, applied to every tag kind alike:
    /// 1. any tag resolving to [`TagState::Excluded`] hides the task;
    /// 2. when anything at all is required, a task must carry at least one tag
    ///    resolving to [`TagState::Required`].
    ///
    /// Rule 2 is a disjunction because required tags are a set of toggles —
    /// "I am at work, or at home" — not a conjunction of requirements. A query
    /// needing conjunction spells it out with `+a +b`.
    pub fn admits(&self, task_tags: &[String]) -> bool {
        let mut satisfied = false;
        for t in task_tags {
            match self.state_of(t) {
                Some(TagState::Excluded) => return false,
                Some(TagState::Required) => satisfied = true,
                _ => {}
            }
        }
        satisfied || !self.any_required()
    }

    /// Whether any tag is currently required.
    pub fn any_required(&self) -> bool {
        self.tags.values().any(|s| *s == TagState::Required)
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
        assert!(!s.any_required());
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
    fn requirement_works_for_every_kind_too() {
        // The point of unification: a resource is not exclude-only, and a
        // freeform tag cannot only be required.
        for tag_name in ["@work", "#laptop", "urgent"] {
            let s = state(&[(tag_name, TagState::Required)]);
            assert!(s.admits(&tags(&[tag_name])), "{tag_name}");
            assert!(!s.admits(&tags(&["other"])), "{tag_name}");
            assert!(
                !s.admits(&[]),
                "{tag_name}: untagged task carries nothing required"
            );
        }
    }

    #[test]
    fn required_tags_are_a_disjunction() {
        let s = state(&[("@work", TagState::Required), ("@home", TagState::Required)]);
        assert!(s.admits(&tags(&["@work"])));
        assert!(s.admits(&tags(&["@home"])));
        assert!(!s.admits(&tags(&["@errands"])));
    }

    #[test]
    fn exclusion_beats_requirement() {
        let s = state(&[
            ("@work", TagState::Required),
            ("#printer", TagState::Excluded),
        ]);
        assert!(s.admits(&tags(&["@work"])));
        assert!(
            !s.admits(&tags(&["@work", "#printer"])),
            "an excluded tag hides the task even when another tag is required"
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
            ("@work/urgent", TagState::Required),
        ]);
        assert_eq!(s.state_of("@work"), Some(TagState::Excluded));
        assert_eq!(s.state_of("@work/urgent"), Some(TagState::Required));
        assert!(s.admits(&tags(&["@work/urgent"])));
        assert!(!s.admits(&tags(&["@work/admin"])));
    }

    #[test]
    fn explicit_acceptance_stops_inheritance() {
        // This is why Accepted exists as a value rather than as an absent key:
        // absence inherits, an explicit acceptance does not.
        let s = state(&[
            ("@work", TagState::Excluded),
            ("@work/urgent", TagState::Accepted),
        ]);
        assert_eq!(s.state_of("@work/urgent"), None);
        assert!(s.admits(&tags(&["@work/urgent"])));
        assert!(!s.admits(&tags(&["@work"])));
    }

    #[test]
    fn set_state_clears_with_none() {
        let mut s = state(&[("@work", TagState::Required)]);
        s.set_state("@work", None);
        assert!(s.tags.is_empty());
        assert!(s.admits(&tags(&["@anything"])));
    }

    #[test]
    fn tags_with_lists_in_order() {
        let s = state(&[
            ("@work", TagState::Required),
            ("#printer", TagState::Excluded),
            ("@home", TagState::Required),
        ]);
        assert_eq!(s.tags_with(TagState::Required), vec!["@home", "@work"]);
        assert_eq!(s.tags_with(TagState::Excluded), vec!["#printer"]);
    }

    // ── Serialisation ────────────────────────────────────────────────────────

    #[test]
    fn states_serialise_under_their_new_names() {
        let s = state(&[
            ("@work", TagState::Required),
            ("#printer", TagState::Excluded),
            ("@home", TagState::Accepted),
        ]);
        let out = toml::to_string(&s).unwrap();
        assert!(out.contains(r#""@work" = "required""#), "{out}");
        assert!(out.contains(r##""#printer" = "excluded""##), "{out}");
        assert!(out.contains(r#""@home" = "accepted""#), "{out}");
    }

    #[test]
    fn the_old_spellings_still_load() {
        // A pure rename should not silently drop the state someone had set, so
        // the previous names stay readable. Nothing writes them any more, so a
        // file self-heals on the next state change.
        let toml = "[tags]\n\"@work\" = \"included\"\n\"@home\" = \"default\"\n\"#printer\" = \"excluded\"\n";
        let s: GlobalState = toml::from_str(toml).unwrap();
        assert_eq!(s.tags.get("@work"), Some(&TagState::Required));
        assert_eq!(s.tags.get("@home"), Some(&TagState::Accepted));
        assert_eq!(s.tags.get("#printer"), Some(&TagState::Excluded));
    }

    #[test]
    fn an_unreadable_state_is_dropped_not_fatal() {
        // state.toml is read on every command, so one bad entry must not brick
        // the tool. The rest of the file has to survive.
        let toml = "[tags]\n\"@work\" = \"required\"\n\"@mystery\" = \"sideways\"\n\"@odd\" = 7\n";
        let s: GlobalState = toml::from_str(toml).unwrap();
        assert_eq!(s.tags.get("@work"), Some(&TagState::Required));
        assert!(!s.tags.contains_key("@mystery"));
        assert!(!s.tags.contains_key("@odd"));
    }
}
