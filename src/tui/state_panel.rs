//! State-management panel model for the TUI.
//!
//! The panel lets the user view and edit machine-local [`GlobalState`] — the
//! per-tag state and the active-user filter — without leaving the UI. It is
//! opened with `S` from [`Mode::Normal`](super::app::Mode) and rendered as a
//! centered popup.
//!
//! There are two sections, not three: tag-state unification means `@contexts`
//! and `#resources` are the same thing, listed and toggled identically. The
//! sigils survive only as a naming convention, which is why the tag list is
//! sorted with them intact rather than split by them.
//!
//! This module owns only the *presentation/navigation* model (which section is
//! focused, which row is highlighted, and the discovered+stored entry lists).
//! The actual mutations are applied by [`App`](super::app::App) through
//! `state_transaction`, mirroring `next tag require|exclude|accept`; the panel
//! is rebuilt from the fresh state afterwards.

use std::collections::BTreeSet;

use crate::core::domain::state::{GlobalState, TagState};
use crate::core::domain::task::Task;

/// Which of the two sections currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Every tag, whatever its sigil: each cycles through the tag states.
    Tags,
    /// Assignees: each toggles in/out of the active-user filter.
    Users,
}

impl Section {
    /// The next section in the `Tags → Users` cycle (`Tab`).
    pub fn next(self) -> Self {
        match self {
            Section::Tags => Section::Users,
            Section::Users => Section::Tags,
        }
    }

    /// The previous section (`BackTab`). With two sections this is `next`,
    /// but both exist so the key handlers read symmetrically.
    pub fn prev(self) -> Self {
        self.next()
    }

    /// The short heading shown above the section's list.
    pub fn label(self) -> &'static str {
        match self {
            Section::Tags => "Tags",
            Section::Users => "Users",
        }
    }
}

/// A tag row: the tag, the state stored *for it*, and the state that actually
/// applies once inheritance from a parent tag is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRow {
    /// The full tag, sigil included.
    pub tag: String,
    /// The entry stored for exactly this tag, if any.
    pub own: Option<TagState>,
    /// What applies after inheritance — `None` when nothing does.
    pub effective: Option<TagState>,
}

impl TagRow {
    /// Whether this row shows a state it inherited rather than one set on it.
    pub fn is_inherited(&self) -> bool {
        self.own.is_none() && self.effective.is_some()
    }

    /// The label shown in the row: the effective state, marked when inherited.
    pub fn state_label(&self) -> String {
        let name = match self.effective {
            Some(TagState::Required) => "required",
            Some(TagState::Excluded) => "excluded",
            Some(TagState::Accepted) | None => "-",
        };
        if self.is_inherited() {
            format!("{name} (inherited)")
        } else if self.own == Some(TagState::Accepted) {
            "accepted (pinned)".to_owned()
        } else {
            name.to_owned()
        }
    }

    /// The next state in the cycle none → required → excluded → accepted → none.
    ///
    /// One key covers every state because every tag has the same states; the
    /// pinned `accepted` is in the cycle because it is the only way to opt a
    /// child out of a parent's state.
    pub fn cycled(&self) -> Option<TagState> {
        match self.own {
            None => Some(TagState::Required),
            Some(TagState::Required) => Some(TagState::Excluded),
            Some(TagState::Excluded) => Some(TagState::Accepted),
            Some(TagState::Accepted) => None,
        }
    }

    /// The cycle run backwards, so a mis-press is one key away from undone.
    pub fn cycled_back(&self) -> Option<TagState> {
        match self.own {
            None => Some(TagState::Accepted),
            Some(TagState::Accepted) => Some(TagState::Excluded),
            Some(TagState::Excluded) => Some(TagState::Required),
            Some(TagState::Required) => None,
        }
    }
}

/// A user row: the assignee name plus whether it is in the active filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    /// The assignee name (free string).
    pub name: String,
    /// Whether the user is in `active_users`.
    pub active: bool,
}

/// Live state for the state-management popup. Holds the derived row lists
/// (rebuilt from the store on open and after each mutation) plus the focused
/// section and the per-section highlighted index.
pub struct StatePanel {
    pub tags: Vec<TagRow>,
    pub users: Vec<UserRow>,

    /// The section with keyboard focus.
    pub section: Section,
    /// Highlighted row within the Tags list.
    pub tag_idx: usize,
    /// Highlighted row within the Users list.
    pub user_idx: usize,
}

impl StatePanel {
    /// Builds the panel by unioning the entries discovered across `tasks` with
    /// whatever the current `state` already tracks, so both stored-but-unused
    /// and used-but-unstored entries are visible and toggleable.
    pub fn build(tasks: &[Task], state: &GlobalState) -> Self {
        Self {
            tags: build_tags(tasks, state),
            users: build_users(tasks, state),
            section: Section::Tags,
            tag_idx: 0,
            user_idx: 0,
        }
    }

    /// Rebuilds the row lists from a fresh `state` (after a mutation + reload),
    /// preserving the focused section and clamping the highlighted indices.
    pub fn refresh(&mut self, tasks: &[Task], state: &GlobalState) {
        self.tags = build_tags(tasks, state);
        self.users = build_users(tasks, state);
        self.clamp();
    }

    /// Clamps every section's highlight into range for its (possibly shrunk)
    /// list.
    fn clamp(&mut self) {
        clamp_idx(&mut self.tag_idx, self.tags.len());
        clamp_idx(&mut self.user_idx, self.users.len());
    }

    /// Moves the highlight down within the focused section.
    pub fn select_next(&mut self) {
        let (idx, len) = self.focused_idx_len();
        if len > 0 && *idx + 1 < len {
            *idx += 1;
        }
    }

    /// Moves the highlight up within the focused section.
    pub fn select_prev(&mut self) {
        let (idx, _) = self.focused_idx_len();
        *idx = idx.saturating_sub(1);
    }

    /// Switches to the next section (`Tab`).
    pub fn focus_next(&mut self) {
        self.section = self.section.next();
    }

    /// Switches to the previous section (`BackTab`).
    pub fn focus_prev(&mut self) {
        self.section = self.section.prev();
    }

    /// A mutable handle to the focused section's index plus its list length.
    fn focused_idx_len(&mut self) -> (&mut usize, usize) {
        match self.section {
            Section::Tags => (&mut self.tag_idx, self.tags.len()),
            Section::Users => (&mut self.user_idx, self.users.len()),
        }
    }

    /// The highlighted tag row, if the Tags section is non-empty.
    pub fn selected_tag(&self) -> Option<&TagRow> {
        self.tags.get(self.tag_idx)
    }

    /// The highlighted user row, if the Users section is non-empty.
    pub fn selected_user(&self) -> Option<&UserRow> {
        self.users.get(self.user_idx)
    }
}

/// Clamps `idx` to the last valid row of a list of `len` rows (0 when empty).
fn clamp_idx(idx: &mut usize, len: usize) {
    if len == 0 {
        *idx = 0;
    } else if *idx >= len {
        *idx = len - 1;
    }
}

/// Every tag in use across `tasks`, unioned with every tag the state already
/// tracks, sorted for stable display. No filtering by sigil: a resource is not
/// a different kind of row from a context.
fn build_tags(tasks: &[Task], state: &GlobalState) -> Vec<TagRow> {
    let mut tags: BTreeSet<String> = BTreeSet::new();
    for task in tasks {
        for t in &task.tags {
            tags.insert(t.clone());
        }
    }
    tags.extend(state.tags.keys().cloned());

    tags.into_iter()
        .map(|tag| TagRow {
            own: state.tags.get(&tag).copied(),
            effective: state.state_of(&tag),
            tag,
        })
        .collect()
}

/// The distinct assignees across `tasks`, unioned with the active users already
/// in `state`, sorted.
fn build_users(tasks: &[Task], state: &GlobalState) -> Vec<UserRow> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for task in tasks {
        if let Some(a) = &task.assignee {
            names.insert(a.clone());
        }
    }
    names.extend(state.active_users.iter().cloned());

    names
        .into_iter()
        .map(|name| UserRow {
            active: state.active_users.contains(&name),
            name,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_with(tags: &[&str], assignee: Option<&str>) -> Task {
        let mut t = Task::new("t");
        t.tags = tags.iter().map(|s| s.to_string()).collect();
        t.assignee = assignee.map(str::to_owned);
        t
    }

    #[test]
    fn tags_union_tasks_and_state_across_kinds() {
        let tasks = vec![task_with(&["@work", "#printer", "errand"], None)];
        let mut state = GlobalState::default();
        state.set_state("@home", Some(TagState::Required));
        state.set_state("@work", Some(TagState::Excluded));

        let rows = build_tags(&tasks, &state);
        let tags: Vec<&str> = rows.iter().map(|r| r.tag.as_str()).collect();
        // One sorted list: discovered tags of every kind, unioned with the
        // stored ones, not split by sigil.
        assert_eq!(tags, vec!["#printer", "@home", "@work", "errand"]);

        let work = rows.iter().find(|r| r.tag == "@work").unwrap();
        assert_eq!(work.own, Some(TagState::Excluded));
        let printer = rows.iter().find(|r| r.tag == "#printer").unwrap();
        assert_eq!(printer.own, None, "no state is the default for any kind");
    }

    #[test]
    fn inherited_state_is_shown_but_not_owned() {
        let tasks = vec![task_with(&["#office/printer"], None)];
        let mut state = GlobalState::default();
        state.set_state("#office", Some(TagState::Excluded));

        let rows = build_tags(&tasks, &state);
        let child = rows.iter().find(|r| r.tag == "#office/printer").unwrap();
        assert_eq!(child.own, None);
        assert_eq!(child.effective, Some(TagState::Excluded));
        assert!(child.is_inherited());
        assert!(child.state_label().contains("inherited"));
    }

    #[test]
    fn the_cycle_covers_every_state_and_returns_to_none() {
        let row = TagRow {
            tag: "@work".to_owned(),
            own: None,
            effective: None,
        };
        let mut seen = vec![row.cycled()];
        let mut current = row;
        for _ in 0..3 {
            current = TagRow {
                own: seen.last().copied().flatten(),
                ..current
            };
            seen.push(current.cycled());
        }
        assert_eq!(
            seen,
            vec![
                Some(TagState::Required),
                Some(TagState::Excluded),
                Some(TagState::Accepted),
                None,
            ]
        );
    }

    #[test]
    fn the_reverse_cycle_undoes_the_forward_one() {
        for own in [
            None,
            Some(TagState::Required),
            Some(TagState::Excluded),
            Some(TagState::Accepted),
        ] {
            let row = TagRow {
                tag: "@work".to_owned(),
                own,
                effective: own.filter(|s| *s != TagState::Accepted),
            };
            let forward = TagRow {
                own: row.cycled(),
                ..row.clone()
            };
            assert_eq!(forward.cycled_back(), own, "{own:?}");
        }
    }

    #[test]
    fn users_union_tasks_and_active() {
        let tasks = vec![task_with(&[], Some("alice")), task_with(&[], Some("bob"))];
        let state = GlobalState {
            active_users: vec!["bob".to_owned(), "carol".to_owned()],
            ..Default::default()
        };
        let rows = build_users(&tasks, &state);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alice", "bob", "carol"]);
        assert!(!rows.iter().find(|r| r.name == "alice").unwrap().active);
        assert!(rows.iter().find(|r| r.name == "bob").unwrap().active);
    }

    #[test]
    fn navigation_clamps_within_focused_section() {
        let tasks = vec![task_with(&["@a", "@b"], None)];
        let mut panel = StatePanel::build(&tasks, &GlobalState::default());
        assert_eq!(panel.section, Section::Tags);
        panel.select_next();
        assert_eq!(panel.tag_idx, 1);
        panel.select_next(); // clamps at last
        assert_eq!(panel.tag_idx, 1);
        panel.select_prev();
        panel.select_prev(); // clamps at first
        assert_eq!(panel.tag_idx, 0);
    }

    #[test]
    fn section_cycles_both_ways() {
        let mut panel = StatePanel::build(&[], &GlobalState::default());
        panel.focus_next();
        assert_eq!(panel.section, Section::Users);
        panel.focus_next();
        assert_eq!(panel.section, Section::Tags);
        panel.focus_prev();
        assert_eq!(panel.section, Section::Users);
    }
}
