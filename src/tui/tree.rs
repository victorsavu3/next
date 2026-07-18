//! Tree-view state for the TUI.
//!
//! [`TreeView`] owns the `tui-tree-widget` selection/expansion state plus the
//! tree-local `--all` toggle (include done/cancelled). The parent/child
//! structure is rebuilt from the app's cached task list on every reload (and
//! whenever the `--all` toggle flips), grouping root tasks by their deepest
//! context tag — the same grouping `next tree` applies on the CLI.
//!
//! Node identifiers are task [`Uuid`]s; section-header nodes use deterministic
//! v5 UUIDs derived from the section name, which never collide with random v4
//! task UUIDs. `selected_id()` always returns the *last* element of the path,
//! so it resolves to a task UUID regardless of nesting depth.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_tree_widget::{TreeItem, TreeState};
use uuid::Uuid;

use crate::core::domain::filter::{self, FilterSet};
use crate::core::domain::state::GlobalState;
use crate::core::domain::tag;
use crate::core::domain::task::{Status, Task};

/// Namespace UUID for deriving deterministic section-header identifiers.
/// Any fixed, well-known UUID works; we use the OID namespace from RFC 4122.
const SECTION_NS: Uuid = Uuid::NAMESPACE_OID;

/// Derives a stable UUID for a context-section header from its display name.
fn section_uuid(name: &str) -> Uuid {
    Uuid::new_v5(&SECTION_NS, name.as_bytes())
}

/// Output of [`build_items`].
pub struct TreeBuild {
    /// The widget items (sections at the root level, tasks nested inside).
    pub items: Vec<TreeItem<'static, Uuid>>,
    /// UUID of every section header, in display order. Used to auto-open them.
    pub section_ids: Vec<Uuid>,
    /// Maps each root task UUID to the section UUID it was placed in (primary
    /// placement only). Used by the view-switch to seed the selection path.
    pub task_section: HashMap<Uuid, Uuid>,
}

/// Live state for the tree view.
#[derive(Default)]
pub struct TreeView {
    /// Widget selection + expansion state, keyed by task id.
    state: TreeState<Uuid>,
    /// Tree-local `--all`: include done/cancelled tasks (mirrors `next tree --all`).
    include_all: bool,
    /// Section UUIDs that have already been auto-opened. Once opened, a section
    /// stays open or closed based on user input — we never re-force it open.
    known_sections: HashSet<Uuid>,
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
    /// the last element is the node itself (task UUID even when nested in a section).
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

    /// Jump the highlight to the first visible node.
    pub fn key_first(&mut self) {
        self.state.select_first();
    }

    /// Jump the highlight to the last visible node.
    pub fn key_last(&mut self) {
        self.state.select_last();
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

    /// Opens any section in `section_ids` that hasn't been opened before.
    /// Sections the user has closed via keyboard are already in `known_sections`
    /// and are left alone, so user collapses survive rebuilds.
    pub fn sync_sections(&mut self, section_ids: &[Uuid]) {
        for &id in section_ids {
            if self.known_sections.insert(id) {
                self.state.open(vec![id]);
            }
        }
    }
}

/// Builds the displayable tree (context sections → roots → nested children)
/// from `all_tasks`, honouring `filter_set` and the `include_all` toggle.
///
/// Filtering is applied in two stages:
/// 1. `filter::apply` runs first, restricting to tasks that pass the user's
///    `FilterSet` (tags, contexts, users, etc.). The `FilterSet`'s own
///    `disable_implicit` field (set by `filter_all`) determines whether the
///    implicit status/blocking/resource gate is skipped.
/// 2. If `include_all` is false, done/cancelled tasks that survived stage 1
///    are additionally excluded — the tree-local `.` toggle controls this.
///
/// Root tasks (visible tasks whose parent is absent or not visible) are then
/// sorted alphabetically and grouped into context sections by their deepest
/// `@context` tag, mirroring `next tree`. Tasks with no context tag fall into
/// a trailing "No context" section.
pub fn build_items(
    all_tasks: &[Task],
    filter_set: &FilterSet,
    state: &GlobalState,
    today: NaiveDate,
    include_all: bool,
) -> TreeBuild {
    // Stage 1: apply the user's FilterSet.
    //
    // When the tree-local `include_all` toggle is on we want done/cancelled
    // tasks to pass through the implicit gate (status/blocking/resource
    // checks), so we force `disable_implicit = true` in that case.  We
    // preserve the active/excluded contexts from state via the override fields
    // so that context filtering still applies even with the implicit gate off.
    let effective_filter = if include_all && !filter_set.disable_implicit {
        FilterSet {
            disable_implicit: true,
            context_override: Some(
                filter_set
                    .context_override
                    .clone()
                    .unwrap_or_else(|| state.active_contexts.clone()),
            ),
            excluded_context_override: Some(
                filter_set
                    .excluded_context_override
                    .clone()
                    .unwrap_or_else(|| state.excluded_contexts.clone()),
            ),
            ..filter_set.clone()
        }
    } else {
        filter_set.clone()
    };
    let filtered = filter::apply(all_tasks.to_vec(), &effective_filter, state, today);

    // Stage 2: if the tree-local include_all toggle is off, additionally
    // exclude done/cancelled tasks that survived the filter.
    let visible_ids: HashSet<Uuid> = filtered
        .iter()
        .filter(|t| include_all || t.is_active())
        .map(|t| t.id)
        .collect();

    // parent_id -> visible children
    let mut children_map: HashMap<Uuid, Vec<&Task>> = HashMap::new();
    for task in all_tasks.iter().filter(|t| visible_ids.contains(&t.id)) {
        if let Some(pid) = task.parent_id {
            if visible_ids.contains(&pid) {
                children_map.entry(pid).or_default().push(task);
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

    if roots.is_empty() {
        return TreeBuild { items: Vec::new(), section_ids: Vec::new(), task_section: HashMap::new() };
    }

    // ── Context grouping ──────────────────────────────────────────────────────
    //
    // Each root task is placed in exactly one section: the alphabetically first
    // of its deepest context tag(s). Tasks with no context tag go into "No
    // context". Children always follow their parent's primary section.
    //
    // Unlike the CLI, duplicate entries in secondary context sections are not
    // shown in the TUI — the widget doesn't have inline per-row notes.

    // BTreeMap gives sorted iteration; "No context" is extracted separately.
    let mut sections: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
    let mut task_section: HashMap<Uuid, Uuid> = HashMap::new();

    for root in &roots {
        let ctx_tags = tag::deepest_context_tags(&root.tags);
        // Pick the primary section: alphabetically first deepest tag, or "No context".
        let section_name = ctx_tags.into_iter().next().unwrap_or_else(|| "No context".to_string());
        sections.entry(section_name).or_default().push(root);
    }

    // Move "No context" to the end.
    let no_ctx = sections.remove("No context");

    // Fill task_section map (root_task_id → section UUID).
    for (name, tasks) in &sections {
        let sid = section_uuid(name);
        for task in tasks {
            task_section.insert(task.id, sid);
        }
    }
    if let Some(ref tasks) = no_ctx {
        let sid = section_uuid("No context");
        for task in tasks {
            task_section.insert(task.id, sid);
        }
    }

    // ── Build TreeItem nodes ──────────────────────────────────────────────────

    let mut items: Vec<TreeItem<'static, Uuid>> = Vec::new();
    let mut section_ids: Vec<Uuid> = Vec::new();

    for (name, tasks) in &sections {
        let sid = section_uuid(name);
        section_ids.push(sid);
        let child_items: Vec<TreeItem<'static, Uuid>> = tasks
            .iter()
            .map(|t| build_node(t, &children_map))
            .collect();
        items.push(
            TreeItem::new(sid, section_line(name), child_items)
                .expect("section UUID collision"),
        );
    }

    if let Some(tasks) = no_ctx {
        let sid = section_uuid("No context");
        section_ids.push(sid);
        let child_items: Vec<TreeItem<'static, Uuid>> = tasks
            .iter()
            .map(|t| build_node(t, &children_map))
            .collect();
        items.push(
            TreeItem::new(sid, section_line("No context"), child_items)
                .expect("No context UUID collision"),
        );
    }

    TreeBuild { items, section_ids, task_section }
}

/// Given the visible tree `items` and the id of the node about to be deleted,
/// returns the full selection path (root → node) the highlight should move to
/// afterwards, following the rule: next sibling, else previous sibling, else
/// the parent. Returns `None` when `target` isn't in the tree, or when it is a
/// top-level node with no siblings and no parent.
///
/// Deleting a node never removes its ancestors, so the returned path stays
/// valid across the post-delete rebuild and can be handed straight to
/// `TreeState::select`.
pub fn neighbor_after_delete(items: &[TreeItem<'_, Uuid>], target: Uuid) -> Option<Vec<Uuid>> {
    /// `ancestors` is the path down to (and including) the parent of `siblings`;
    /// it is empty at the top level, where `has_parent` is false.
    fn walk(
        siblings: &[TreeItem<'_, Uuid>],
        ancestors: &[Uuid],
        has_parent: bool,
        target: Uuid,
    ) -> Option<Vec<Uuid>> {
        for (i, item) in siblings.iter().enumerate() {
            if *item.identifier() == target {
                // Next sibling, else previous sibling.
                if let Some(sib) =
                    siblings.get(i + 1).or_else(|| i.checked_sub(1).map(|p| &siblings[p]))
                {
                    let mut path = ancestors.to_vec();
                    path.push(*sib.identifier());
                    return Some(path);
                }
                // Only child: fall back to the parent (its full path is `ancestors`).
                return has_parent.then(|| ancestors.to_vec());
            }
            let mut child_ancestors = ancestors.to_vec();
            child_ancestors.push(*item.identifier());
            if let Some(found) = walk(item.children(), &child_ancestors, true, target) {
                return Some(found);
            }
        }
        None
    }
    walk(items, &[], false, target)
}

/// Recursively builds one [`TreeItem`] (and its visible subtree).
fn build_node(task: &Task, children: &HashMap<Uuid, Vec<&Task>>) -> TreeItem<'static, Uuid> {
    let kids = children.get(&task.id);
    let text = node_line(task);

    match kids {
        Some(kids) if !kids.is_empty() => {
            let mut sorted: Vec<&Task> = kids.clone();
            sorted.sort_by(|a, b| a.title.cmp(&b.title));
            let child_items: Vec<TreeItem<'static, Uuid>> =
                sorted.into_iter().map(|c| build_node(c, children)).collect();
            TreeItem::new(task.id, text, child_items)
                .expect("duplicate task id in tree children")
        }
        _ => TreeItem::new_leaf(task.id, text),
    }
}

/// Styled separator line for a context section header: `── @work ──`.
fn section_line(name: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("── {name} ──"),
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )])
}

/// The display line for one node: `<glyph> [id] title`.
fn node_line(task: &Task) -> Line<'static> {
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

    let title_style = match task.status {
        Status::Done => Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        Status::Cancelled => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
        _ => Style::default(),
    };

    Line::from(vec![
        Span::styled(glyph, glyph_style),
        Span::raw(" "),
        Span::styled(format!("[{short}] "), Style::default().add_modifier(Modifier::DIM)),
        Span::styled(task.title.clone(), title_style),
    ])
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::core::domain::state::GlobalState;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, 6).unwrap()
    }

    fn no_filter() -> FilterSet {
        FilterSet::default()
    }

    fn no_state() -> GlobalState {
        GlobalState::default()
    }

    fn child_of(title: &str, parent: Uuid) -> Task {
        let mut t = Task::new(title.to_owned());
        t.parent_id = Some(parent);
        t
    }

    fn all_filter() -> FilterSet {
        FilterSet { disable_implicit: true, ..FilterSet::default() }
    }

    /// Find a task item anywhere in the tree (searches all sections' subtrees).
    fn find_task<'a>(items: &'a [TreeItem<'a, Uuid>], task_id: Uuid) -> Option<&'a TreeItem<'a, Uuid>> {
        for item in items {
            if *item.identifier() == task_id {
                return Some(item);
            }
            if let Some(found) = find_task(item.children(), task_id) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn roots_are_top_level_visible_tasks() {
        let root = Task::new("root".to_owned());
        let child = child_of("child", root.id);
        let other = Task::new("other".to_owned());
        let build = build_items(&[root, child, other], &all_filter(), &no_state(), today(), false);
        // Both root tasks land in a single "No context" section.
        assert_eq!(build.items.len(), 1, "expected one section");
        let section = &build.items[0];
        assert_eq!(section.children().len(), 2, "two roots in the section");
    }

    #[test]
    fn child_nests_under_parent() {
        let root = Task::new("root".to_owned());
        let child = child_of("child", root.id);
        let build = build_items(
            &[root.clone(), child],
            &all_filter(),
            &no_state(),
            today(),
            false,
        );
        let root_item = find_task(&build.items, root.id).unwrap();
        assert_eq!(root_item.children().len(), 1);
    }

    #[test]
    fn all_toggle_reveals_done_task() {
        let root = Task::new("root".to_owned());
        let mut done = child_of("done child", root.id);
        done.mark_done(today());
        let tasks = vec![root.clone(), done];

        // Without --all the done child is hidden.
        let build = build_items(&tasks, &no_filter(), &no_state(), today(), false);
        let root_item = find_task(&build.items, root.id).unwrap();
        assert_eq!(root_item.children().len(), 0);

        // With --all the done child appears.
        let mut af = no_filter();
        af.disable_implicit = true;
        let build = build_items(&tasks, &af, &no_state(), today(), true);
        let root_item = find_task(&build.items, root.id).unwrap();
        assert_eq!(root_item.children().len(), 1);
    }

    #[test]
    fn hidden_parent_promotes_child_to_root() {
        let mut parent = Task::new("done parent".to_owned());
        parent.mark_done(today());
        let child = child_of("active child", parent.id);
        let build = build_items(&[parent, child.clone()], &no_filter(), &no_state(), today(), false);
        // Only the child is visible, promoted to root inside "No context".
        assert_eq!(build.items.len(), 1, "one section");
        let section = &build.items[0];
        assert_eq!(section.children().len(), 1, "child promoted to section root");
        assert_eq!(*section.children()[0].identifier(), child.id);
    }

    #[test]
    fn filter_set_restricts_tree_to_matching_tasks() {
        let mut tagged = Task::new("tagged task".to_owned());
        tagged.tags = vec!["#work".to_owned()];
        let untagged = Task::new("untagged task".to_owned());

        let filter_set = FilterSet {
            required_tags: vec!["#work".to_owned()],
            disable_implicit: true,
            ..FilterSet::default()
        };

        let build = build_items(
            &[tagged.clone(), untagged.clone()],
            &filter_set,
            &no_state(),
            today(),
            true,
        );

        // One section, one task.
        assert_eq!(build.items.len(), 1);
        assert_eq!(build.items[0].children().len(), 1);
        assert_eq!(*build.items[0].children()[0].identifier(), tagged.id);
    }

    #[test]
    fn filter_applies_on_top_of_include_all() {
        let mut done_matching = Task::new("done + tagged".to_owned());
        done_matching.tags = vec!["#work".to_owned()];
        done_matching.mark_done(today());

        let mut done_not_matching = Task::new("done + untagged".to_owned());
        done_not_matching.mark_done(today());

        let filter_set = FilterSet {
            required_tags: vec!["#work".to_owned()],
            disable_implicit: true,
            ..FilterSet::default()
        };

        let build = build_items(
            &[done_matching.clone(), done_not_matching.clone()],
            &filter_set,
            &no_state(),
            today(),
            true,
        );

        assert_eq!(build.items.len(), 1);
        assert_eq!(build.items[0].children().len(), 1);
        assert_eq!(*build.items[0].children()[0].identifier(), done_matching.id);
    }

    #[test]
    fn active_context_respected_with_include_all() {
        let mut work_task = Task::new("work task".to_owned());
        work_task.tags = vec!["@work".to_owned()];

        let mut home_task = Task::new("home task".to_owned());
        home_task.tags = vec!["@home".to_owned()];

        let state = GlobalState {
            active_contexts: vec!["@work".to_owned()],
            ..Default::default()
        };

        let build = build_items(&[work_task.clone(), home_task.clone()], &no_filter(), &state, today(), true);

        // Only @work is visible → one "@work" section, one task inside.
        assert_eq!(build.items.len(), 1, "only @work task should be visible");
        assert_eq!(build.items[0].children().len(), 1);
        assert_eq!(*build.items[0].children()[0].identifier(), work_task.id);
    }

    #[test]
    fn excluded_context_respected_with_include_all() {
        let mut home_task = Task::new("home task".to_owned());
        home_task.tags = vec!["@home".to_owned()];

        let neutral_task = Task::new("neutral task".to_owned());

        let state = GlobalState {
            excluded_contexts: vec!["@home".to_owned()],
            ..Default::default()
        };

        let build = build_items(
            &[home_task.clone(), neutral_task.clone()],
            &no_filter(),
            &state,
            today(),
            true,
        );

        // Only the neutral task survives → "No context" section with one task.
        assert_eq!(build.items.len(), 1, "excluded @home task must not appear");
        assert_eq!(build.items[0].children().len(), 1);
        assert_eq!(*build.items[0].children()[0].identifier(), neutral_task.id);
    }

    #[test]
    fn include_all_shows_done_tasks_in_active_context_only() {
        let mut done_work = Task::new("done work".to_owned());
        done_work.tags = vec!["@work".to_owned()];
        done_work.mark_done(today());

        let mut done_home = Task::new("done home".to_owned());
        done_home.tags = vec!["@home".to_owned()];
        done_home.mark_done(today());

        let state = GlobalState {
            active_contexts: vec!["@work".to_owned()],
            ..Default::default()
        };

        let build = build_items(
            &[done_work.clone(), done_home.clone()],
            &no_filter(),
            &state,
            today(),
            true,
        );

        assert_eq!(build.items.len(), 1, "only done @work task should be visible");
        assert_eq!(build.items[0].children().len(), 1);
        assert_eq!(*build.items[0].children()[0].identifier(), done_work.id);
    }

    // ── Context section grouping ─────────────────────────────────────────────

    #[test]
    fn tasks_grouped_by_deepest_context_tag() {
        let mut work_task = Task::new("Work task".to_owned());
        work_task.tags = vec!["@work".to_owned()];

        let mut home_task = Task::new("Home task".to_owned());
        home_task.tags = vec!["@home".to_owned()];

        let plain = Task::new("Plain task".to_owned());

        let build = build_items(
            &[work_task.clone(), home_task.clone(), plain.clone()],
            &all_filter(),
            &no_state(),
            today(),
            false,
        );

        // Three sections: @home, @work (sorted), No context (last).
        assert_eq!(build.items.len(), 3, "expected 3 sections");
        assert_eq!(*build.items[0].identifier(), section_uuid("@home"));
        assert_eq!(*build.items[1].identifier(), section_uuid("@work"));
        assert_eq!(*build.items[2].identifier(), section_uuid("No context"));

        // Each section has exactly one task.
        assert_eq!(build.items[0].children().len(), 1);
        assert_eq!(build.items[1].children().len(), 1);
        assert_eq!(build.items[2].children().len(), 1);
    }

    #[test]
    fn nested_context_tag_creates_its_own_section() {
        let mut task = Task::new("Frontend task".to_owned());
        task.tags = vec!["@work/frontend".to_owned()];

        let build = build_items(&[task.clone()], &all_filter(), &no_state(), today(), false);

        assert_eq!(build.items.len(), 1);
        assert_eq!(*build.items[0].identifier(), section_uuid("@work/frontend"));
    }

    #[test]
    fn no_context_section_appears_last() {
        let mut work_task = Task::new("Work".to_owned());
        work_task.tags = vec!["@work".to_owned()];
        let plain = Task::new("Plain".to_owned());

        let build = build_items(
            &[work_task.clone(), plain.clone()],
            &all_filter(),
            &no_state(),
            today(),
            false,
        );

        // @work first, No context last.
        assert_eq!(build.items.len(), 2);
        assert_eq!(*build.items[0].identifier(), section_uuid("@work"));
        assert_eq!(*build.items[1].identifier(), section_uuid("No context"));
    }

    #[test]
    fn task_section_map_covers_all_roots() {
        let mut work_task = Task::new("Work task".to_owned());
        work_task.tags = vec!["@work".to_owned()];
        let plain = Task::new("Plain task".to_owned());

        let build = build_items(
            &[work_task.clone(), plain.clone()],
            &all_filter(),
            &no_state(),
            today(),
            false,
        );

        assert_eq!(build.task_section.get(&work_task.id), Some(&section_uuid("@work")));
        assert_eq!(build.task_section.get(&plain.id), Some(&section_uuid("No context")));
    }

    // ── neighbor_after_delete ─────────────────────────────────────────────────

    #[test]
    fn neighbor_prefers_next_sibling() {
        // Three roots sort to a, b, c inside "No context".
        let a = Task::new("a".to_owned());
        let b = Task::new("b".to_owned());
        let c = Task::new("c".to_owned());
        let build = build_items(
            &[a.clone(), b.clone(), c.clone()],
            &all_filter(),
            &no_state(),
            today(),
            false,
        );
        let path = neighbor_after_delete(&build.items, b.id).unwrap();
        assert_eq!(path.last(), Some(&c.id), "deleting b should target next sibling c");
    }

    #[test]
    fn neighbor_falls_back_to_previous_sibling_when_last() {
        let a = Task::new("a".to_owned());
        let b = Task::new("b".to_owned());
        let c = Task::new("c".to_owned());
        let build = build_items(
            &[a.clone(), b.clone(), c.clone()],
            &all_filter(),
            &no_state(),
            today(),
            false,
        );
        let path = neighbor_after_delete(&build.items, c.id).unwrap();
        assert_eq!(path.last(), Some(&b.id), "deleting last sibling targets previous");
    }

    #[test]
    fn neighbor_of_only_child_is_parent() {
        let parent = Task::new("parent".to_owned());
        let child = child_of("child", parent.id);
        let build = build_items(
            &[parent.clone(), child.clone()],
            &all_filter(),
            &no_state(),
            today(),
            false,
        );
        let path = neighbor_after_delete(&build.items, child.id).unwrap();
        assert_eq!(path.last(), Some(&parent.id), "only child targets its parent");
    }

    #[test]
    fn neighbor_of_missing_target_is_none() {
        let a = Task::new("a".to_owned());
        let build = build_items(&[a], &all_filter(), &no_state(), today(), false);
        assert!(neighbor_after_delete(&build.items, Uuid::new_v4()).is_none());
    }

    #[test]
    fn sync_sections_opens_new_sections_once() {
        let mut view = TreeView::default();
        let ids = vec![section_uuid("@work"), section_uuid("@home")];

        view.sync_sections(&ids);
        assert!(view.known_sections.contains(&section_uuid("@work")));
        assert!(view.known_sections.contains(&section_uuid("@home")));

        // Calling again with the same ids is a no-op (already in known_sections).
        let pre_len = view.known_sections.len();
        view.sync_sections(&ids);
        assert_eq!(view.known_sections.len(), pre_len);
    }
}
