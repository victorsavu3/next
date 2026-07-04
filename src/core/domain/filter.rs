use std::collections::HashSet;

use chrono::NaiveDate;
use uuid::Uuid;

use crate::core::domain::{
    state::GlobalState,
    tag,
    task::{Status, Task},
};

/// Controls which tasks are returned by `apply`.
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    /// Tags that must ALL appear on a task (hierarchical: `#lang` matches `#lang/rust`).
    pub required_tags: Vec<String>,

    /// Tags that must NOT appear on a task.
    pub excluded_tags: Vec<String>,

    /// Override active contexts for this query.
    /// `None` → use `state.active_contexts`.
    /// `Some(v)` → use `v` (pass an empty vec to disable context filtering entirely).
    pub context_override: Option<Vec<String>>,

    /// Override active users for this query.
    /// `None` → use `state.active_users`.
    /// `Some(v)` → use `v` (pass an empty vec to disable user filtering entirely).
    pub user_override: Option<Vec<String>>,

    /// Include tasks hidden by `start_date` in the future.
    pub include_future: bool,

    /// Skip the implicit visibility gate entirely (show everything regardless
    /// of status, blocking, resources, context, or user).
    pub disable_implicit: bool,

    /// Keep parent tasks that have open children (projects) visible, instead of
    /// hiding them under the "work on the children instead" rule.  All other
    /// implicit filters still apply.  The scored CLI list leaves this `false`
    /// (a blocked parent isn't actionable); the TUI sets it `true` so projects
    /// show alongside their subtasks.
    pub include_blocked_parents: bool,

    /// When set, only tasks that are descendants (or the root itself) of the
    /// task with this slug are returned.  Resolved against the full task list
    /// inside `apply`; silently returns nothing if the slug is not found.
    pub parent_slug: Option<String>,

    /// Show only done and cancelled tasks instead of active ones.
    /// Context, user, and explicit tag filters still apply; start-date,
    /// blocking, parent, and resource gates are skipped.
    pub closed_only: bool,
}

/// Applies `filter` to `tasks` and returns those that pass.
///
/// `today` is used for start-date and age checks. `state` supplies the global
/// context and resource availability when the implicit gate is active.
///
/// **Implicit gate** (skipped when `filter.disable_implicit` is true):
/// 1. Task must have `status == Open`.
/// 2. Task must not be hidden by `start_date` (unless `include_future`).
/// 3. Task must not be blocked by an open `blocked_by` task.
/// 4. Task must not be a parent with open children (work on the children
///    instead) — unless `include_blocked_parents` is set, which keeps projects visible.
/// 5. Task must not carry any unavailable `#resource` tag.
/// 6. If contexts are active, task must have at least one matching `@context` tag.
///
/// **Explicit filters** (always applied):
/// - `required_tags`: task must contain ALL (hierarchical match).
/// - `excluded_tags`: task must contain NONE.
pub fn apply(
    tasks: Vec<Task>,
    filter: &FilterSet,
    state: &GlobalState,
    today: NaiveDate,
) -> Vec<Task> {
    let (open_ids, parents_with_open_children) = if filter.disable_implicit {
        (HashSet::new(), HashSet::new())
    } else {
        build_implicit_indexes(&tasks)
    };

    // Pre-compute descendant set for parent_slug filter (empty = no filter).
    let project_descendants: Option<HashSet<Uuid>> = filter.parent_slug.as_ref().map(|slug| {
        tasks
            .iter()
            .find(|t| t.slug.as_deref() == Some(slug.as_str()))
            .map(|root| descendants_of(root.id, &tasks))
            .unwrap_or_default()
    });

    let active_contexts: &[String] = if filter.disable_implicit {
        &[]
    } else {
        filter
            .context_override
            .as_deref()
            .unwrap_or(&state.active_contexts)
    };

    let active_users: &[String] = if filter.disable_implicit {
        &[]
    } else {
        filter
            .user_override
            .as_deref()
            .unwrap_or(&state.active_users)
    };

    tasks
        .into_iter()
        .filter(|task| {
            // ── Implicit gate ────────────────────────────────────────────────
            if !filter.disable_implicit {
                if filter.closed_only {
                    if !matches!(task.status, Status::Done | Status::Cancelled) {
                        return false;
                    }
                } else {
                    if !task.is_active() {
                        return false;
                    }
                    if !filter.include_future && task.is_hidden(today) {
                        return false;
                    }
                    if task.blocked_by.iter().any(|id| open_ids.contains(id)) {
                        return false;
                    }
                    if !filter.include_blocked_parents
                        && parents_with_open_children.contains(&task.id)
                    {
                        return false;
                    }
                    if task
                        .tags
                        .iter()
                        .any(|t| tag::is_resource(t) && !state.is_resource_available(t))
                    {
                        return false;
                    }
                }
                if !active_contexts.is_empty() && !task_matches_contexts(task, active_contexts) {
                    return false;
                }
                if !state.excluded_contexts.is_empty()
                    && task_excluded_by_contexts(task, &state.excluded_contexts)
                {
                    return false;
                }
                // User filter: unassigned tasks are always visible; assigned tasks
                // must match one of the active users.
                if !active_users.is_empty() {
                    if let Some(ref assignee) = task.assignee {
                        if !active_users.iter().any(|u| u == assignee) {
                            return false;
                        }
                    }
                }
            }

            // ── Explicit filters ─────────────────────────────────────────────
            if let Some(ref desc) = project_descendants {
                if !desc.contains(&task.id) {
                    return false;
                }
            }
            for req in &filter.required_tags {
                if !task.tags.iter().any(|t| tag::tag_matches(req, t)) {
                    return false;
                }
            }
            for exc in &filter.excluded_tags {
                if task.tags.iter().any(|t| tag::tag_matches(exc, t)) {
                    return false;
                }
            }

            true
        })
        .collect()
}

/// Returns `true` if `task` has at least one `@context` tag that is compatible
/// with one of the `active_contexts`.
///
/// Compatibility is bidirectional: `@work` active matches a task tagged
/// `@work/frontend` (ancestor of task context), and `@work/frontend` active
/// matches a task tagged `@work` (task context is ancestor of active context).
/// This ensures general work tasks remain visible when a sub-context is active.
fn task_matches_contexts(task: &Task, active_contexts: &[String]) -> bool {
    let task_contexts: Vec<&str> = task
        .tags
        .iter()
        .filter(|t| tag::is_context(t))
        .map(|t| t.as_str())
        .collect();

    if task_contexts.is_empty() {
        return true;
    }

    active_contexts.iter().any(|active| {
        task_contexts.iter().any(|&tc| {
            tag::tag_matches(active, tc) || tag::tag_matches(tc, active)
        })
    })
}

/// Returns `true` if `task` should be hidden because one of its `@context`
/// tags matches an excluded context.
///
/// Matching is one-directional: `excluded` is an ancestor of (or equal to)
/// the task's context tag.  This means excluding `@home` also hides
/// `@home/kitchen`, but excluding `@home/kitchen` does NOT hide tasks tagged
/// only with `@home`.  Tasks with no `@` tags are never excluded.
fn task_excluded_by_contexts(task: &Task, excluded_contexts: &[String]) -> bool {
    let task_contexts: Vec<&str> = task
        .tags
        .iter()
        .filter(|t| tag::is_context(t))
        .map(|t| t.as_str())
        .collect();

    if task_contexts.is_empty() {
        return false;
    }

    excluded_contexts.iter().any(|exc| {
        task_contexts.iter().any(|&tc| tag::tag_matches(exc, tc))
    })
}

/// Pre-computes two indexes needed by the implicit gate.
///
/// Returns:
/// - `open_ids`: IDs of all currently open tasks (for `blocked_by` checks).
/// - `parents_with_open_children`: IDs of tasks that have at least one open child.
fn build_implicit_indexes(tasks: &[Task]) -> (HashSet<Uuid>, HashSet<Uuid>) {
    let open_ids: HashSet<Uuid> = tasks
        .iter()
        .filter(|t| t.is_active())
        .map(|t| t.id)
        .collect();

    let parents_with_open_children: HashSet<Uuid> = tasks
        .iter()
        .filter(|t| t.is_active())
        .filter_map(|t| t.parent_id)
        .collect();

    (open_ids, parents_with_open_children)
}

/// Returns the set of all task IDs that are descendants of `root_id` (inclusive).
fn descendants_of(root_id: Uuid, tasks: &[Task]) -> HashSet<Uuid> {
    let mut result = HashSet::new();
    result.insert(root_id);
    let mut queue = vec![root_id];
    while let Some(current) = queue.pop() {
        for task in tasks {
            if task.parent_id == Some(current) && result.insert(task.id) {
                queue.push(task.id);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::task::Task;
    use chrono::NaiveDate;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()
    }

    fn empty_state() -> GlobalState {
        GlobalState::default()
    }

    fn run(tasks: Vec<Task>, filter: FilterSet) -> Vec<Task> {
        apply(tasks, &filter, &empty_state(), today())
    }

    // ── Implicit gate ────────────────────────────────────────────────────────

    #[test]
    fn excludes_done_tasks() {
        let mut done = Task::new("Done task");
        done.mark_done(today());
        let open = Task::new("Open task");
        let result = run(vec![done, open], FilterSet::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Open task");
    }

    #[test]
    fn excludes_cancelled_tasks() {
        let mut cancelled = Task::new("Cancelled");
        cancelled.mark_cancelled();
        let result = run(vec![cancelled], FilterSet::default());
        assert!(result.is_empty());
    }

    #[test]
    fn excludes_future_tasks_by_default() {
        let mut future = Task::new("Future");
        future.start = Some(today() + chrono::Duration::days(7));
        let result = run(vec![future], FilterSet::default());
        assert!(result.is_empty());
    }

    #[test]
    fn include_future_shows_hidden_tasks() {
        let mut future = Task::new("Future");
        future.start = Some(today() + chrono::Duration::days(7));
        let filter = FilterSet {
            include_future: true,
            ..Default::default()
        };
        let result = run(vec![future], filter);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn start_date_today_is_not_hidden() {
        let mut task = Task::new("Starts today");
        task.start = Some(today());
        let result = run(vec![task], FilterSet::default());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn excludes_task_blocked_by_open_task() {
        let blocker = Task::new("Blocker");
        let mut blocked = Task::new("Blocked");
        blocked.blocked_by = vec![blocker.id];

        let all = vec![blocker, blocked];
        let result = run(all, FilterSet::default());
        // Only the blocker passes (the blocked task is excluded)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Blocker");
    }

    #[test]
    fn task_unblocked_when_blocker_is_done() {
        let mut blocker = Task::new("Blocker");
        blocker.mark_done(today());
        let mut blocked = Task::new("Blocked");
        blocked.blocked_by = vec![blocker.id];

        let result = run(vec![blocker, blocked], FilterSet::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Blocked");
    }

    #[test]
    fn parent_hidden_when_it_has_open_children() {
        let parent = Task::new("Project");
        let mut child = Task::new("Subtask");
        child.parent_id = Some(parent.id);

        let all = vec![parent, child];
        let result = run(all, FilterSet::default());
        // Only the child is shown; parent is hidden (work on the subtask)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Subtask");
    }

    #[test]
    fn include_blocked_parents_keeps_project_visible() {
        let parent = Task::new("Project");
        let mut child = Task::new("Subtask");
        child.parent_id = Some(parent.id);

        let filter = FilterSet {
            include_blocked_parents: true,
            ..Default::default()
        };
        let result = run(vec![parent, child], filter);
        // Both the project and its open subtask are shown.
        let titles: Vec<&str> = result.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(result.len(), 2, "{titles:?}");
        assert!(titles.contains(&"Project"));
        assert!(titles.contains(&"Subtask"));
    }

    #[test]
    fn parent_visible_when_all_children_done() {
        let parent = Task::new("Project");
        let mut child = Task::new("Subtask");
        child.parent_id = Some(parent.id);
        child.mark_done(today());

        let result = run(vec![parent, child], FilterSet::default());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Project");
    }

    #[test]
    fn excludes_tasks_with_unavailable_resource() {
        let mut state = GlobalState::default();
        state.resources.insert("printer".into(), false);

        let mut task = Task::new("Print document");
        task.tags = vec!["#printer".into()];

        let result = apply(vec![task], &FilterSet::default(), &state, today());
        assert!(result.is_empty());
    }

    #[test]
    fn unavailable_parent_resource_blocks_child_resource() {
        let mut state = GlobalState::default();
        state.resources.insert("office".into(), false);

        let mut task = Task::new("Use office printer");
        task.tags = vec!["#office/printer".into()];

        let result = apply(vec![task], &FilterSet::default(), &state, today());
        assert!(result.is_empty());
    }

    #[test]
    fn available_resource_not_excluded() {
        let mut task = Task::new("Print document");
        task.tags = vec!["#printer".into()];
        let result = run(vec![task], FilterSet::default());
        assert_eq!(result.len(), 1);
    }

    // ── Context filtering ────────────────────────────────────────────────────

    #[test]
    fn no_active_context_shows_all_tasks() {
        let mut work = Task::new("Work task");
        work.tags = vec!["@work".into()];
        let bare = Task::new("No context");
        let result = run(vec![work, bare], FilterSet::default());
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn active_context_hides_wrong_context_tasks() {
        let state = GlobalState { active_contexts: vec!["@work".into()], ..Default::default() };

        let mut work_task = Task::new("Work task");
        work_task.tags = vec!["@work".into()];

        let mut home_task = Task::new("Home task");
        home_task.tags = vec!["@home".into()];

        let result = apply(vec![work_task, home_task], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Work task");
    }

    #[test]
    fn active_context_keeps_context_neutral_tasks() {
        let state = GlobalState { active_contexts: vec!["@work".into()], ..Default::default() };

        let mut work_task = Task::new("Work task");
        work_task.tags = vec!["@work".into()];

        let neutral_task = Task::new("No context task");

        let result = apply(vec![work_task, neutral_task], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn parent_context_active_shows_sub_context_task() {
        let state = GlobalState { active_contexts: vec!["@work".into()], ..Default::default() };

        let mut task = Task::new("Frontend work");
        task.tags = vec!["@work/frontend".into()];

        let result = apply(vec![task], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn sub_context_active_shows_parent_context_task() {
        let state = GlobalState { active_contexts: vec!["@work/frontend".into()], ..Default::default() };

        let mut task = Task::new("General work task");
        task.tags = vec!["@work".into()]; // less specific than active context

        let result = apply(vec![task], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn context_override_replaces_state_contexts() {
        let state = GlobalState { active_contexts: vec!["@work".into()], ..Default::default() };

        let mut home_task = Task::new("Home task");
        home_task.tags = vec!["@home".into()];

        let filter = FilterSet {
            context_override: Some(vec!["@home".into()]),
            ..Default::default()
        };
        let result = apply(vec![home_task], &filter, &state, today());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn context_override_empty_disables_context_filter() {
        let state = GlobalState { active_contexts: vec!["@work".into()], ..Default::default() };

        let task = Task::new("No context");
        let filter = FilterSet {
            context_override: Some(vec![]), // override to "no filter"
            ..Default::default()
        };
        let result = apply(vec![task], &filter, &state, today());
        assert_eq!(result.len(), 1);
    }

    // ── Excluded contexts ─────────────────────────────────────────────────────

    #[test]
    fn excluded_context_hides_matching_task() {
        let state = GlobalState { excluded_contexts: vec!["@home".into()], ..Default::default() };

        let mut home_task = Task::new("Home task");
        home_task.tags = vec!["@home".into()];
        let neutral = Task::new("No context");

        let result = apply(vec![home_task, neutral], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "No context");
    }

    #[test]
    fn excluded_context_hides_descendant_contexts() {
        let state = GlobalState { excluded_contexts: vec!["@home".into()], ..Default::default() };

        let mut kitchen = Task::new("Kitchen task");
        kitchen.tags = vec!["@home/kitchen".into()];

        let result = apply(vec![kitchen], &FilterSet::default(), &state, today());
        assert!(result.is_empty(), "@home excluded should hide @home/kitchen");
    }

    #[test]
    fn excluded_sub_context_does_not_hide_parent_context_task() {
        let state = GlobalState { excluded_contexts: vec!["@home/kitchen".into()], ..Default::default() };

        let mut home_task = Task::new("General home task");
        home_task.tags = vec!["@home".into()];

        let result = apply(vec![home_task], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1, "@home/kitchen excluded should not hide @home task");
    }

    #[test]
    fn context_neutral_task_not_hidden_by_exclusion() {
        let state = GlobalState { excluded_contexts: vec!["@home".into()], ..Default::default() };

        let neutral = Task::new("No context task");

        let result = apply(vec![neutral], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1, "context-neutral task must not be excluded");
    }

    #[test]
    fn excluded_context_applies_alongside_active_context() {
        let state = GlobalState {
            active_contexts: vec!["@home".into()],
            excluded_contexts: vec!["@home/kitchen".into()],
            ..Default::default()
        };

        let mut living_room = Task::new("Living room task");
        living_room.tags = vec!["@home/living".into()];
        let mut kitchen = Task::new("Kitchen task");
        kitchen.tags = vec!["@home/kitchen".into()];

        let result = apply(vec![living_room, kitchen], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Living room task");
    }

    // ── Explicit filters ─────────────────────────────────────────────────────

    #[test]
    fn required_tag_exact_match() {
        let mut tagged = Task::new("Tagged");
        tagged.tags = vec!["#rust".into()];
        let untagged = Task::new("Untagged");

        let filter = FilterSet {
            required_tags: vec!["#rust".into()],
            ..Default::default()
        };
        let result = run(vec![tagged, untagged], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Tagged");
    }

    #[test]
    fn required_tag_ancestor_matches_descendant() {
        let mut task = Task::new("Rust task");
        task.tags = vec!["#lang/rust".into()];

        let filter = FilterSet {
            required_tags: vec!["#lang".into()],
            ..Default::default()
        };
        let result = run(vec![task], filter);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn required_tag_all_must_match() {
        let mut both = Task::new("Both tags");
        both.tags = vec!["#a".into(), "#b".into()];
        let mut one = Task::new("One tag");
        one.tags = vec!["#a".into()];

        let filter = FilterSet {
            required_tags: vec!["#a".into(), "#b".into()],
            ..Default::default()
        };
        let result = run(vec![both, one], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Both tags");
    }

    #[test]
    fn excluded_tag_removes_task() {
        let mut tagged = Task::new("Tagged");
        tagged.tags = vec!["#work".into()];
        let clean = Task::new("Clean");

        let filter = FilterSet {
            excluded_tags: vec!["#work".into()],
            ..Default::default()
        };
        let result = run(vec![tagged, clean], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Clean");
    }

    #[test]
    fn excluded_tag_ancestor_excludes_descendant_tagged_tasks() {
        let mut task = Task::new("Rust task");
        task.tags = vec!["#lang/rust".into()];

        let filter = FilterSet {
            excluded_tags: vec!["#lang".into()],
            ..Default::default()
        };
        let result = run(vec![task], filter);
        assert!(result.is_empty());
    }

    // ── disable_implicit ─────────────────────────────────────────────────────

    #[test]
    fn disable_implicit_shows_done_tasks() {
        let mut done = Task::new("Done");
        done.mark_done(today());
        let filter = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        let result = run(vec![done], filter);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn disable_implicit_shows_blocked_tasks() {
        let blocker = Task::new("Blocker");
        let mut blocked = Task::new("Blocked");
        blocked.blocked_by = vec![blocker.id];

        let filter = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        let result = run(vec![blocker, blocked], filter);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn disable_implicit_ignores_contexts() {
        let state = GlobalState { active_contexts: vec!["@work".into()], ..Default::default() };

        let task = Task::new("No context");
        let filter = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        let result = apply(vec![task], &filter, &state, today());
        assert_eq!(result.len(), 1);
    }

    // ── User filtering ───────────────────────────────────────────────────────

    #[test]
    fn no_active_users_shows_all_tasks() {
        let mut assigned = Task::new("Alice task");
        assigned.assignee = Some("alice".into());
        let unassigned = Task::new("Shared task");
        let result = run(vec![assigned, unassigned], FilterSet::default());
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn active_user_hides_other_users_tasks() {
        let state = GlobalState { active_users: vec!["alice".into()], ..Default::default() };

        let mut alice_task = Task::new("Alice task");
        alice_task.assignee = Some("alice".into());
        let mut bob_task = Task::new("Bob task");
        bob_task.assignee = Some("bob".into());

        let result = apply(vec![alice_task, bob_task], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Alice task");
    }

    #[test]
    fn unassigned_tasks_visible_when_user_filter_active() {
        let state = GlobalState { active_users: vec!["alice".into()], ..Default::default() };

        let unassigned = Task::new("Shared task");
        let result = apply(vec![unassigned], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn multiple_active_users_shows_all_their_tasks() {
        let state = GlobalState { active_users: vec!["alice".into(), "bob".into()], ..Default::default() };

        let mut alice_task = Task::new("Alice task");
        alice_task.assignee = Some("alice".into());
        let mut bob_task = Task::new("Bob task");
        bob_task.assignee = Some("bob".into());
        let mut carol_task = Task::new("Carol task");
        carol_task.assignee = Some("carol".into());

        let result = apply(
            vec![alice_task, bob_task, carol_task],
            &FilterSet::default(),
            &state,
            today(),
        );
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn user_override_empty_bypasses_user_filter() {
        let state = GlobalState { active_users: vec!["alice".into()], ..Default::default() };

        let mut bob_task = Task::new("Bob task");
        bob_task.assignee = Some("bob".into());

        let filter = FilterSet {
            user_override: Some(vec![]), // override: no filter
            ..Default::default()
        };
        let result = apply(vec![bob_task], &filter, &state, today());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn disable_implicit_bypasses_user_filter() {
        let state = GlobalState { active_users: vec!["alice".into()], ..Default::default() };

        let mut bob_task = Task::new("Bob task");
        bob_task.assignee = Some("bob".into());

        let filter = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        let result = apply(vec![bob_task], &filter, &state, today());
        assert_eq!(result.len(), 1);
    }

    // ── Combined ─────────────────────────────────────────────────────────────

    #[test]
    fn explicit_filters_apply_even_with_disable_implicit() {
        let mut done = Task::new("Done with tag");
        done.mark_done(today());
        done.tags = vec!["#keep".into()];
        let mut done_no_tag = Task::new("Done without tag");
        done_no_tag.mark_done(today());

        let filter = FilterSet {
            disable_implicit: true,
            required_tags: vec!["#keep".into()],
            ..Default::default()
        };
        let result = run(vec![done, done_no_tag], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Done with tag");
    }
}
