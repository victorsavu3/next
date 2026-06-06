//! Tree-view state for the TUI.
//!
//! [`TreeView`] owns the `tui-tree-widget` selection/expansion state plus the
//! tree-local `--all` toggle (include done/cancelled). The parent/child
//! structure is rebuilt from the app's cached task list on every reload (and
//! whenever the `--all` toggle flips), mirroring `next tree`'s roots/children
//! logic. Node identifiers are task [`Uuid`]s, so the highlighted node maps
//! straight back to a task for the detail pane and the shared actions.

use std::collections::{HashMap, HashSet};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_tree_widget::{TreeItem, TreeState};
use uuid::Uuid;

use crate::core::domain::tag;
use crate::core::domain::task::{Status, Task};

/// Live state for the tree view.
#[derive(Default)]
pub struct TreeView {
    /// Widget selection + expansion state, keyed by task id.
    state: TreeState<Uuid>,
    /// Tree-local `--all`: include done/cancelled tasks (mirrors `next tree --all`).
    include_all: bool,
}

impl TreeView {
    /// Whether done/cancelled tasks are currently included.
    pub fn include_all(&self) -> bool {
        self.include_all
    }

    /// Flips the tree-local `--all` toggle.
    pub fn toggle_all(&mut self) {
        self.include_all = !self.include_all;
    }

    /// Mutable access to the widget state, for the draw layer's stateful render.
    pub fn state_mut(&mut self) -> &mut TreeState<Uuid> {
        &mut self.state
    }

    /// The id of the highlighted node, if any. The selection is a path of ids;
    /// the last element is the node itself.
    pub fn selected_id(&self) -> Option<Uuid> {
        self.state.selected().last().copied()
    }

    /// Move the highlight down one visible node.
    pub fn key_down(&mut self) {
        self.state.key_down();
    }

    /// Move the highlight up one visible node.
    pub fn key_up(&mut self) {
        self.state.key_up();
    }

    /// Collapse the highlighted node (or step to its parent).
    pub fn collapse(&mut self) {
        self.state.key_left();
    }

    /// Expand the highlighted node (or step into its first child).
    pub fn expand(&mut self) {
        self.state.key_right();
    }

    /// Toggle expand/collapse of the highlighted node.
    pub fn toggle(&mut self) {
        self.state.toggle_selected();
    }

    /// Ensures a node is selected when the tree is non-empty (e.g. after the
    /// first build or a reload that left the selection on a vanished node).
    pub fn ensure_selection(&mut self, items: &[TreeItem<'_, Uuid>]) {
        if self.state.selected().is_empty() && !items.is_empty() {
            self.state.select_first();
        }
    }
}

/// Builds the displayable tree (roots + nested children) from `all_tasks`,
/// honouring the `include_all` toggle. Returns the widget items; node ids are
/// task [`Uuid`]s.
///
/// Roots are visible tasks whose parent is absent or not visible, sorted by
/// title; children follow the same visibility + sort, mirroring `next tree`.
pub fn build_items(all_tasks: &[Task], include_all: bool) -> Vec<TreeItem<'static, Uuid>> {
    let visible_ids: HashSet<Uuid> = all_tasks
        .iter()
        .filter(|t| include_all || t.is_active())
        .map(|t| t.id)
        .collect();

    // parent_id -> visible children
    let mut children: HashMap<Uuid, Vec<&Task>> = HashMap::new();
    for task in all_tasks.iter().filter(|t| visible_ids.contains(&t.id)) {
        if let Some(pid) = task.parent_id {
            if visible_ids.contains(&pid) {
                children.entry(pid).or_default().push(task);
            }
        }
    }

    let mut roots: Vec<&Task> = all_tasks
        .iter()
        .filter(|t| {
            visible_ids.contains(&t.id)
                && t.parent_id
                    .map(|pid| !visible_ids.contains(&pid))
                    .unwrap_or(true)
        })
        .collect();
    roots.sort_by(|a, b| a.title.cmp(&b.title));

    roots
        .into_iter()
        .map(|root| build_node(root, &children))
        .collect()
}

/// Recursively builds one [`TreeItem`] (and its visible subtree).
fn build_node(task: &Task, children: &HashMap<Uuid, Vec<&Task>>) -> TreeItem<'static, Uuid> {
    let kids = children.get(&task.id);
    let has_children = kids.is_some_and(|k| !k.is_empty());
    let text = node_line(task, has_children);

    match kids {
        Some(kids) if !kids.is_empty() => {
            let mut sorted: Vec<&Task> = kids.clone();
            sorted.sort_by(|a, b| a.title.cmp(&b.title));
            let child_items: Vec<TreeItem<'static, Uuid>> =
                sorted.into_iter().map(|c| build_node(c, children)).collect();
            // Ids are unique task UUIDs, so `TreeItem::new` cannot fail.
            TreeItem::new(task.id, text, child_items)
                .expect("duplicate task id in tree children")
        }
        _ => TreeItem::new_leaf(task.id, text),
    }
}

/// The display line for one node: `<glyph> [id] title [project]`.
fn node_line(task: &Task, has_children: bool) -> Line<'static> {
    let (glyph, glyph_style) = match task.status {
        Status::Open => ("○", Style::default()),
        Status::Started => ("▶", Style::default().fg(Color::Green)),
        Status::Done => ("✓", Style::default().fg(Color::Green).add_modifier(Modifier::DIM)),
        Status::Cancelled => (
            "✗",
            Style::default().add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
        ),
    };
    let short = task.id.to_string().replace('-', "")[..8].to_owned();

    let mut spans = vec![
        Span::styled(glyph, glyph_style),
        Span::raw(" "),
        Span::styled(format!("[{short}] "), Style::default().add_modifier(Modifier::DIM)),
        Span::raw(task.title.clone()),
    ];

    // A task tagged "project" with children is flagged so projects stand out.
    if has_children && is_project_tagged(task) {
        spans.push(Span::styled(
            "  [project]",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    }

    Line::from(spans)
}

/// Whether the task carries a `project` tag (bare, `#project`, or `@project`).
fn is_project_tagged(task: &Task) -> bool {
    task.tags.iter().any(|t| tag::bare_name(t) == "project")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child_of(title: &str, parent: Uuid) -> Task {
        let mut t = Task::new(title.to_owned());
        t.parent_id = Some(parent);
        t
    }

    #[test]
    fn roots_are_top_level_visible_tasks() {
        let root = Task::new("root".to_owned());
        let child = child_of("child", root.id);
        let other = Task::new("other".to_owned());
        let items = build_items(&[root, child, other], false);
        // Two roots: "root" and "other" (child is nested under root).
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn child_nests_under_parent() {
        let root = Task::new("root".to_owned());
        let child = child_of("child", root.id);
        let items = build_items(&[root.clone(), child], false);
        let root_item = items.iter().find(|i| *i.identifier() == root.id).unwrap();
        assert_eq!(root_item.children().len(), 1);
    }

    #[test]
    fn all_toggle_reveals_done_task() {
        let root = Task::new("root".to_owned());
        let mut done = child_of("done child", root.id);
        done.mark_done();
        let tasks = vec![root.clone(), done];

        // Without --all the done child is hidden, so root has no children.
        let items = build_items(&tasks, false);
        let root_item = items.iter().find(|i| *i.identifier() == root.id).unwrap();
        assert_eq!(root_item.children().len(), 0);

        // With --all the done child appears.
        let items = build_items(&tasks, true);
        let root_item = items.iter().find(|i| *i.identifier() == root.id).unwrap();
        assert_eq!(root_item.children().len(), 1);
    }

    #[test]
    fn hidden_parent_promotes_child_to_root() {
        // A done parent (hidden without --all) means its active child becomes a
        // root in the default view.
        let mut parent = Task::new("done parent".to_owned());
        parent.mark_done();
        let child = child_of("active child", parent.id);
        let items = build_items(&[parent, child.clone()], false);
        // Only the child is visible, promoted to root.
        assert_eq!(items.len(), 1);
        assert_eq!(*items[0].identifier(), child.id);
    }

    #[test]
    fn project_marker_only_with_children_and_tag() {
        // Tagged "project" WITH children → marked.
        let mut proj = Task::new("proj".to_owned());
        proj.tags = vec!["project".to_owned()];
        let child = child_of("c", proj.id);
        assert!(is_project_tagged(&proj));
        let items = build_items(&[proj.clone(), child], false);
        let line = node_line(&proj, true);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[project]"), "{text}");
        assert_eq!(items.len(), 1);

        // Tagged "project" WITHOUT children → not marked.
        let lonely = node_line(&proj, false);
        let text: String = lonely.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.contains("[project]"), "{text}");

        // Untagged with children → not marked.
        let plain = Task::new("plain".to_owned());
        assert!(!is_project_tagged(&plain));
    }

    #[test]
    fn hash_prefixed_project_tag_is_recognised() {
        let mut t = Task::new("p".to_owned());
        t.tags = vec!["#project".to_owned()];
        assert!(is_project_tagged(&t));
    }
}
