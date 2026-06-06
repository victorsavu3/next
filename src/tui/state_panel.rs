//! State-management panel model for the TUI.
//!
//! The panel lets the user view and edit machine-local [`GlobalState`] —
//! active/excluded `@contexts`, `#resource` availability, and the active-user
//! filter — without leaving the UI. It is opened with `S` from
//! [`Mode::Normal`](super::app::Mode) and rendered as a centered popup.
//!
//! This module owns only the *presentation/navigation* model (which section is
//! focused, which row is highlighted, and the discovered+stored entry lists).
//! The actual mutations are applied by [`App`](super::app::App) through
//! `state_transaction`, mirroring the CLI `context`/`resource`/`user` commands;
//! the panel is rebuilt from the fresh state afterwards.

use std::collections::BTreeSet;

use crate::core::domain::state::GlobalState;
use crate::core::domain::tag;
use crate::core::domain::task::Task;

/// Which of the three sections currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// `@context` tags: each can be toggled Active and/or Excluded.
    Contexts,
    /// `#resource` tags: each toggles between available / unavailable.
    Resources,
    /// Assignees: each toggles in/out of the active-user filter.
    Users,
}

impl Section {
    /// The next section in the `Contexts → Resources → Users` cycle (`Tab`).
    pub fn next(self) -> Self {
        match self {
            Section::Contexts => Section::Resources,
            Section::Resources => Section::Users,
            Section::Users => Section::Contexts,
        }
    }

    /// The previous section (`BackTab`).
    pub fn prev(self) -> Self {
        match self {
            Section::Contexts => Section::Users,
            Section::Resources => Section::Contexts,
            Section::Users => Section::Resources,
        }
    }

    /// The short heading shown above the section's list.
    pub fn label(self) -> &'static str {
        match self {
            Section::Contexts => "Contexts",
            Section::Resources => "Resources",
            Section::Users => "Users",
        }
    }
}

/// A `@context` row: the tag plus its current active/excluded flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow {
    /// The full `@`-prefixed tag.
    pub tag: String,
    /// Whether the context is in `active_contexts`.
    pub active: bool,
    /// Whether the context is in `excluded_contexts`.
    pub excluded: bool,
}

/// A `#resource` row: the tag plus its availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRow {
    /// The full `#`-prefixed tag.
    pub tag: String,
    /// Whether the resource is currently available (absent key ⇒ available).
    pub available: bool,
}

/// A user row: the assignee name plus whether it is in the active filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    /// The assignee name (free string).
    pub name: String,
    /// Whether the user is in `active_users`.
    pub active: bool,
}

/// Live state for the state-management popup. Holds the three derived row lists
/// (rebuilt from the store on open and after each mutation) plus the focused
/// section and the per-section highlighted index.
pub struct StatePanel {
    pub contexts: Vec<ContextRow>,
    pub resources: Vec<ResourceRow>,
    pub users: Vec<UserRow>,

    /// The section with keyboard focus.
    pub section: Section,
    /// Highlighted row within the Contexts list.
    pub ctx_idx: usize,
    /// Highlighted row within the Resources list.
    pub res_idx: usize,
    /// Highlighted row within the Users list.
    pub user_idx: usize,
}

impl StatePanel {
    /// Builds the panel by unioning the entries discovered across `tasks` with
    /// whatever the current `state` already tracks, so both stored-but-unused
    /// and used-but-unstored entries are visible and toggleable.
    pub fn build(tasks: &[Task], state: &GlobalState) -> Self {
        Self {
            contexts: build_contexts(tasks, state),
            resources: build_resources(tasks, state),
            users: build_users(tasks, state),
            section: Section::Contexts,
            ctx_idx: 0,
            res_idx: 0,
            user_idx: 0,
        }
    }

    /// Rebuilds the row lists from a fresh `state` (after a mutation + reload),
    /// preserving the focused section and clamping the highlighted indices.
    pub fn refresh(&mut self, tasks: &[Task], state: &GlobalState) {
        self.contexts = build_contexts(tasks, state);
        self.resources = build_resources(tasks, state);
        self.users = build_users(tasks, state);
        self.clamp();
    }

    /// Clamps every section's highlight into range for its (possibly shrunk)
    /// list.
    fn clamp(&mut self) {
        clamp_idx(&mut self.ctx_idx, self.contexts.len());
        clamp_idx(&mut self.res_idx, self.resources.len());
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
            Section::Contexts => (&mut self.ctx_idx, self.contexts.len()),
            Section::Resources => (&mut self.res_idx, self.resources.len()),
            Section::Users => (&mut self.user_idx, self.users.len()),
        }
    }

    /// The highlighted context row, if the Contexts section is non-empty.
    pub fn selected_context(&self) -> Option<&ContextRow> {
        self.contexts.get(self.ctx_idx)
    }

    /// The highlighted resource row, if the Resources section is non-empty.
    pub fn selected_resource(&self) -> Option<&ResourceRow> {
        self.resources.get(self.res_idx)
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

/// The distinct `@context` tags found across `tasks`, unioned with the active
/// and excluded contexts already in `state`, sorted for stable display.
fn build_contexts(tasks: &[Task], state: &GlobalState) -> Vec<ContextRow> {
    let mut tags: BTreeSet<String> = BTreeSet::new();
    for task in tasks {
        for t in &task.tags {
            if tag::is_context(t) {
                tags.insert(t.clone());
            }
        }
    }
    tags.extend(state.active_contexts.iter().cloned());
    tags.extend(state.excluded_contexts.iter().cloned());

    tags.into_iter()
        .map(|tag| ContextRow {
            active: state.active_contexts.contains(&tag),
            excluded: state.excluded_contexts.contains(&tag),
            tag,
        })
        .collect()
}

/// The distinct `#resource` tags found across `tasks`, unioned with the
/// resource keys already in `state` (re-prefixed with `#`), sorted.
fn build_resources(tasks: &[Task], state: &GlobalState) -> Vec<ResourceRow> {
    let mut tags: BTreeSet<String> = BTreeSet::new();
    for task in tasks {
        for t in &task.tags {
            if tag::is_resource(t) {
                tags.insert(t.clone());
            }
        }
    }
    for key in state.resources.keys() {
        tags.insert(format!("#{key}"));
    }

    tags.into_iter()
        .map(|tag| {
            let bare = tag.trim_start_matches('#');
            ResourceRow {
                // Absent key ⇒ available; an explicit `false` ⇒ unavailable.
                available: state.resources.get(bare).copied().unwrap_or(true),
                tag,
            }
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
    fn contexts_union_tasks_and_state() {
        let tasks = vec![task_with(&["@work", "#printer"], None)];
        let state = GlobalState {
            active_contexts: vec!["@home".to_owned()],
            excluded_contexts: vec!["@work".to_owned()],
            ..Default::default()
        };
        let rows = build_contexts(&tasks, &state);
        let tags: Vec<&str> = rows.iter().map(|r| r.tag.as_str()).collect();
        // Sorted union of discovered (@work) + active (@home) + excluded (@work).
        assert_eq!(tags, vec!["@home", "@work"]);
        let work = rows.iter().find(|r| r.tag == "@work").unwrap();
        assert!(work.excluded && !work.active);
        let home = rows.iter().find(|r| r.tag == "@home").unwrap();
        assert!(home.active && !home.excluded);
    }

    #[test]
    fn resources_default_available_and_reflect_state() {
        let tasks = vec![task_with(&["#printer", "#scanner"], None)];
        let mut state = GlobalState::default();
        state.resources.insert("printer".to_owned(), false);
        let rows = build_resources(&tasks, &state);
        let printer = rows.iter().find(|r| r.tag == "#printer").unwrap();
        let scanner = rows.iter().find(|r| r.tag == "#scanner").unwrap();
        assert!(!printer.available); // explicit false
        assert!(scanner.available); // absent ⇒ available
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
        assert_eq!(panel.section, Section::Contexts);
        panel.select_next();
        assert_eq!(panel.ctx_idx, 1);
        panel.select_next(); // clamps at last
        assert_eq!(panel.ctx_idx, 1);
        panel.select_prev();
        panel.select_prev(); // clamps at first
        assert_eq!(panel.ctx_idx, 0);
    }

    #[test]
    fn section_cycles_both_ways() {
        let mut panel = StatePanel::build(&[], &GlobalState::default());
        panel.focus_next();
        assert_eq!(panel.section, Section::Resources);
        panel.focus_next();
        assert_eq!(panel.section, Section::Users);
        panel.focus_next();
        assert_eq!(panel.section, Section::Contexts);
        panel.focus_prev();
        assert_eq!(panel.section, Section::Users);
    }
}
