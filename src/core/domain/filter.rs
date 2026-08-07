use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use uuid::Uuid;

use crate::core::domain::{
    filter_eval::{eval, EvalCtx, EvalIndexes},
    filter_expr::Expr,
    state::{GlobalState, TagState},
    task::{Status, Task},
};
use crate::core::scoring::TaskDates;

/// Controls which tasks are returned by `apply`.
#[derive(Debug, Clone, Default)]
pub struct FilterSet {
    /// The user's explicit query, evaluated per task by
    /// [`filter_eval::eval`](crate::core::domain::filter_eval::eval).
    ///
    /// The default — `And(vec![])` — matches everything, so a caller with no
    /// query leaves it alone.
    pub expr: Expr,

    /// Override the *included* tags for this query.
    /// `None` → use whatever the state includes.
    /// `Some(v)` → include exactly these instead (an empty vec includes
    /// nothing, which is how a caller asks to see every tag's tasks).
    /// Exclusions always come from the state.
    pub include_override: Option<Vec<String>>,

    /// Override active users for this query.
    /// `None` → use `state.active_users`.
    /// `Some(v)` → use `v` (pass an empty vec to disable user filtering entirely).
    pub user_override: Option<Vec<String>>,

    /// Include tasks hidden by `start_date` in the future.
    pub include_future: bool,

    /// Skip the implicit visibility gate entirely (show everything regardless
    /// of status, blocking or user). Tag state still applies — see
    /// [`effective_tag_state`].
    pub disable_implicit: bool,

    /// When set, only tasks with status `Done` or `Cancelled` are returned.
    /// Enforced even when `disable_implicit` is true.
    pub closed_only: bool,

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
}

/// Applies `filter` to `tasks` and returns those that pass.
///
/// `today` is used for start-date and age checks. `state` supplies the tag
/// state and the active users.
///
/// **Implicit gate** (skipped when `filter.disable_implicit` is true):
/// 1. Task must have `status == Open`.
/// 2. Task must not be hidden by `start_date` (unless `include_future`).
/// 3. Task must not be blocked by an open `blocked_by` task.
/// 4. Task must not be a parent with open children (work on the children
///    instead) — unless `include_blocked_parents` is set, which keeps projects visible.
/// 5. Task must be assigned to an active user, or unassigned.
///
/// **Tag state** ([`GlobalState::admits`]) is applied outside that gate, so a
/// caller can bypass the status and blocking checks while still respecting
/// which tags are included and excluded. One rule covers every tag kind — a
/// context, a resource and a freeform label are filtered identically.
///
/// **Explicit filter**: `expr`, the user's query, evaluated per task. It is
/// the whole of the explicit half — every `+tag`, field predicate and search
/// term goes through one [`eval`] call.
///
/// `task_dates` supplies the git-derived timestamps behind `created:` and
/// `updated:`; pass an empty map when the caller has none, and those fields
/// read as unset.
pub fn apply(
    tasks: Vec<Task>,
    filter: &FilterSet,
    state: &GlobalState,
    today: NaiveDate,
    task_dates: &HashMap<Uuid, TaskDates>,
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

    // Built once for the whole query, not per task: every index the evaluator
    // consults is a fact about the candidate set, not about one task.
    let indexes = EvalIndexes::build(&tasks);
    let eval_ctx = EvalCtx {
        today,
        indexes: &indexes,
        task_dates,
        archived: false,
    };

    // Tag state is independent of the implicit gate; see `effective_tag_state`.
    let effective_state = effective_tag_state(filter, state);

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
            // ── Status gate — always enforced ────────────────────────────────
            if filter.closed_only {
                if !matches!(task.status, Status::Done | Status::Cancelled) {
                    return false;
                }
            } else if !filter.disable_implicit && !task.is_active() {
                return false;
            }

            // ── Remaining implicit checks — skipped when disable_implicit ────
            if !filter.disable_implicit {
                if !filter.include_future && task.is_hidden(today) {
                    return false;
                }
                if !filter.closed_only && task.blocked_by.iter().any(|id| open_ids.contains(id)) {
                    return false;
                }
                if !filter.closed_only
                    && !filter.include_blocked_parents
                    && parents_with_open_children.contains(&task.id)
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

            // ── Tag state ────────────────────────────────────────────────────
            // One rule for every kind of tag; see `GlobalState::admits`.
            if !effective_state.admits(&task.tags) {
                return false;
            }

            // ── Explicit filters ─────────────────────────────────────────────
            if let Some(ref desc) = project_descendants {
                if !desc.contains(&task.id) {
                    return false;
                }
            }

            eval(&filter.expr, task, &eval_ctx)
        })
        .collect()
}

/// The tag state this query runs under.
///
/// Tag state is independent of the implicit gate: `--all` means "every status",
/// not "every tag". A tag excluded on purpose stays excluded until it is
/// un-excluded, which is what makes exclusion worth setting; the tree view
/// relies on the same rule to keep its filtering while bypassing the status
/// checks.
///
/// `include_override` replaces the *included* set for one query without
/// touching the stored state — how `context:@x` and the MCP context parameter
/// pin a view. Exclusions always come from the state.
fn effective_tag_state(filter: &FilterSet, state: &GlobalState) -> GlobalState {
    let Some(included) = filter.include_override.as_ref() else {
        return state.clone();
    };
    let mut effective = state.clone();
    effective.tags.retain(|_, v| *v != TagState::Included);
    for tag_name in included {
        effective.set_state(tag_name, Some(TagState::Included));
    }
    effective
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
    use crate::core::domain::filter_expr::parse;
    use crate::core::domain::task::Task;
    use chrono::NaiveDate;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()
    }

    fn empty_state() -> GlobalState {
        GlobalState::default()
    }

    /// `apply` with no git dates — every test here filters on stored fields.
    fn apply(
        tasks: Vec<Task>,
        filter: &FilterSet,
        state: &GlobalState,
        today: NaiveDate,
    ) -> Vec<Task> {
        super::apply(tasks, filter, state, today, &HashMap::new())
    }

    fn run(tasks: Vec<Task>, filter: FilterSet) -> Vec<Task> {
        apply(tasks, &filter, &empty_state(), today())
    }

    /// A `FilterSet` carrying nothing but the query `q`.
    fn query(q: &str) -> FilterSet {
        FilterSet {
            expr: parse(q).unwrap_or_else(|e| panic!("{q}: {e}")),
            ..Default::default()
        }
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

    // ── Tag state ────────────────────────────────────────────────────────────
    //
    // One model for every kind of tag: `@`, `#` and freeform are filtered by
    // the same two rules, so these tests deliberately mix the sigils.

    fn state_with(pairs: &[(&str, TagState)]) -> GlobalState {
        GlobalState {
            tags: pairs.iter().map(|(t, s)| (t.to_string(), *s)).collect(),
            ..Default::default()
        }
    }

    fn tagged(title: &str, tags: &[&str]) -> Task {
        let mut t = Task::new(title);
        t.tags = tags.iter().map(|s| s.to_string()).collect();
        t
    }

    #[test]
    fn no_tag_state_shows_everything() {
        let result = run(
            vec![tagged("Work", &["@work"]), Task::new("Untagged")],
            FilterSet::default(),
        );
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn excluding_hides_the_tag_whatever_its_kind() {
        for tag_name in ["@home", "#printer", "errand"] {
            let state = state_with(&[(tag_name, TagState::Excluded)]);
            let result = apply(
                vec![tagged("Hidden", &[tag_name]), Task::new("Untagged")],
                &FilterSet::default(),
                &state,
                today(),
            );
            assert_eq!(result.len(), 1, "{tag_name}");
            assert_eq!(result[0].title, "Untagged", "{tag_name}");
        }
    }

    #[test]
    fn including_restricts_to_that_tag_whatever_its_kind() {
        // A resource can be included, not just excluded — the old model had no
        // way to say "only the things I need the printer for".
        for tag_name in ["@work", "#printer", "errand"] {
            let state = state_with(&[(tag_name, TagState::Included)]);
            let result = apply(
                vec![
                    tagged("Kept", &[tag_name]),
                    tagged("Other", &["@elsewhere"]),
                ],
                &FilterSet::default(),
                &state,
                today(),
            );
            assert_eq!(result.len(), 1, "{tag_name}");
            assert_eq!(result[0].title, "Kept", "{tag_name}");
        }
    }

    #[test]
    fn an_included_tag_hides_untagged_tasks() {
        // The context-neutral exemption is gone: including a tag means the
        // list is that tag's tasks, and a task carrying nothing is not one.
        let state = state_with(&[("@work", TagState::Included)]);
        let result = apply(
            vec![tagged("Work", &["@work"]), Task::new("Untagged")],
            &FilterSet::default(),
            &state,
            today(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Work");
    }

    #[test]
    fn state_matches_descendants_but_not_ancestors() {
        // Downward only, uniformly. The old bidirectional context match — where
        // an active `@work/frontend` also showed plain `@work` tasks — is gone.
        let state = state_with(&[("@work", TagState::Included)]);
        let result = apply(
            vec![tagged("Frontend", &["@work/frontend"])],
            &FilterSet::default(),
            &state,
            today(),
        );
        assert_eq!(result.len(), 1, "a parent include covers its descendants");

        let state = state_with(&[("@work/frontend", TagState::Included)]);
        let result = apply(
            vec![tagged("General work", &["@work"])],
            &FilterSet::default(),
            &state,
            today(),
        );
        assert!(
            result.is_empty(),
            "a child include does not cover its parent"
        );
    }

    #[test]
    fn a_pinned_default_opts_a_child_out_of_its_parents_exclusion() {
        let state = state_with(&[
            ("@home", TagState::Excluded),
            ("@home/kitchen", TagState::Default),
        ]);
        let result = apply(
            vec![
                tagged("Kitchen", &["@home/kitchen"]),
                tagged("Garden", &["@home/garden"]),
            ],
            &FilterSet::default(),
            &state,
            today(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Kitchen");
    }

    #[test]
    fn exclusion_wins_over_inclusion_on_the_same_task() {
        let state = state_with(&[
            ("@work", TagState::Included),
            ("#printer", TagState::Excluded),
        ]);
        let result = apply(
            vec![
                tagged("Needs printer", &["@work", "#printer"]),
                tagged("Plain work", &["@work"]),
            ],
            &FilterSet::default(),
            &state,
            today(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Plain work");
    }

    #[test]
    fn include_override_replaces_the_stored_inclusions() {
        let state = state_with(&[("@work", TagState::Included)]);
        let filter = FilterSet {
            include_override: Some(vec!["@home".into()]),
            ..Default::default()
        };
        let result = apply(
            vec![tagged("Home task", &["@home"]), tagged("Work", &["@work"])],
            &filter,
            &state,
            today(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Home task");
    }

    #[test]
    fn include_override_keeps_the_stored_exclusions() {
        // Overriding what you are working on should not resurrect what you
        // deliberately hid.
        let state = state_with(&[
            ("@work", TagState::Included),
            ("#printer", TagState::Excluded),
        ]);
        let filter = FilterSet {
            include_override: Some(vec![]),
            ..Default::default()
        };
        let result = apply(
            vec![
                tagged("Printing", &["#printer"]),
                tagged("Anything", &["@home"]),
            ],
            &filter,
            &state,
            today(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Anything");
    }

    #[test]
    fn tag_state_survives_disable_implicit() {
        // `--all` means every status, not every tag: the tree view depends on
        // this to keep filtering while showing closed tasks.
        let state = state_with(&[("#printer", TagState::Excluded)]);
        let filter = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        let result = apply(
            vec![tagged("Printing", &["#printer"]), Task::new("Other")],
            &filter,
            &state,
            today(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Other");
    }

    // ── The explicit query ───────────────────────────────────────────────────

    #[test]
    fn required_tag_exact_match() {
        let mut tagged = Task::new("Tagged");
        tagged.tags = vec!["#rust".into()];
        let untagged = Task::new("Untagged");

        let result = run(vec![tagged, untagged], query("+#rust"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Tagged");
    }

    #[test]
    fn required_tag_ancestor_matches_descendant() {
        let mut task = Task::new("Rust task");
        task.tags = vec!["#lang/rust".into()];

        let result = run(vec![task], query("+#lang"));
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn required_tag_all_must_match() {
        let mut both = Task::new("Both tags");
        both.tags = vec!["#a".into(), "#b".into()];
        let mut one = Task::new("One tag");
        one.tags = vec!["#a".into()];

        let result = run(vec![both, one], query("+#a +#b"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Both tags");
    }

    #[test]
    fn excluded_tag_removes_task() {
        let mut tagged = Task::new("Tagged");
        tagged.tags = vec!["#work".into()];
        let clean = Task::new("Clean");

        let result = run(vec![tagged, clean], query("-#work"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Clean");
    }

    #[test]
    fn excluded_tag_ancestor_excludes_descendant_tagged_tasks() {
        let mut task = Task::new("Rust task");
        task.tags = vec!["#lang/rust".into()];

        let result = run(vec![task], query("-#lang"));
        assert!(result.is_empty());
    }

    #[test]
    fn the_empty_query_gates_but_does_not_filter() {
        // The identity expression must leave the implicit gate as the only
        // thing deciding, which is what makes `next list` work with no query.
        let mut done = Task::new("Done");
        done.mark_done(today());
        let open = Task::new("Open");

        let result = run(vec![done, open], query(""));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Open");
    }

    #[test]
    fn a_bare_token_searches_text_instead_of_tags() {
        // The break: `next list bug` used to mean "tagged bug".
        let mut tagged = Task::new("Something else");
        tagged.tags = vec!["bug".into()];
        let mentioned = Task::new("A bug in the parser");

        let result = run(vec![tagged, mentioned], query("bug"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "A bug in the parser");

        // And the sigil form still means the tag, not the prose.
        let mut tagged = Task::new("Something else");
        tagged.tags = vec!["bug".into()];
        let mentioned = Task::new("A bug in the parser");
        let result = run(vec![tagged, mentioned], query("+bug"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Something else");
    }

    #[test]
    fn a_query_can_use_or_and_grouping() {
        let mut a = Task::new("A");
        a.tags = vec!["#a".into()];
        let mut b = Task::new("B");
        b.tags = vec!["#b".into()];
        let c = Task::new("C");

        let result = run(vec![a, b, c], query("+#a or +#b"));
        let titles: Vec<&str> = result.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "B"]);
    }

    #[test]
    fn a_query_sees_the_relational_indexes() {
        // `is:blocked` needs the whole candidate set, so it only works because
        // the indexes are built from the tasks being filtered.
        let blocker = Task::new("Blocker");
        let mut blocked = Task::new("Blocked");
        blocked.blocked_by = vec![blocker.id];

        let filter = FilterSet {
            disable_implicit: true, // the gate would hide a blocked task
            ..query("is:blocked")
        };
        let result = run(vec![blocker, blocked], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Blocked");
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
    fn disable_implicit_still_honours_inclusions() {
        // Changed with tag-state unification: `--all` widens the statuses, not
        // the tags. To see other tags too, override the inclusions.
        let state = state_with(&[("@work", TagState::Included)]);
        let task = Task::new("No tags");
        let filter = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        assert!(apply(vec![task.clone()], &filter, &state, today()).is_empty());

        let filter = FilterSet {
            disable_implicit: true,
            include_override: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(apply(vec![task], &filter, &state, today()).len(), 1);
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
        let state = GlobalState {
            active_users: vec!["alice".into()],
            ..Default::default()
        };

        let mut alice_task = Task::new("Alice task");
        alice_task.assignee = Some("alice".into());
        let mut bob_task = Task::new("Bob task");
        bob_task.assignee = Some("bob".into());

        let result = apply(
            vec![alice_task, bob_task],
            &FilterSet::default(),
            &state,
            today(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Alice task");
    }

    #[test]
    fn unassigned_tasks_visible_when_user_filter_active() {
        let state = GlobalState {
            active_users: vec!["alice".into()],
            ..Default::default()
        };

        let unassigned = Task::new("Shared task");
        let result = apply(vec![unassigned], &FilterSet::default(), &state, today());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn multiple_active_users_shows_all_their_tasks() {
        let state = GlobalState {
            active_users: vec!["alice".into(), "bob".into()],
            ..Default::default()
        };

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
        let state = GlobalState {
            active_users: vec!["alice".into()],
            ..Default::default()
        };

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
        let state = GlobalState {
            active_users: vec!["alice".into()],
            ..Default::default()
        };

        let mut bob_task = Task::new("Bob task");
        bob_task.assignee = Some("bob".into());

        let filter = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        let result = apply(vec![bob_task], &filter, &state, today());
        assert_eq!(result.len(), 1);
    }

    // ── closed_only ──────────────────────────────────────────────────────────

    #[test]
    fn closed_only_shows_done_tasks() {
        let mut done = Task::new("Done task");
        done.mark_done(today());
        let open = Task::new("Open task");
        let filter = FilterSet {
            closed_only: true,
            ..Default::default()
        };
        let result = run(vec![done, open], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Done task");
    }

    #[test]
    fn closed_only_shows_cancelled_tasks() {
        let mut cancelled = Task::new("Cancelled task");
        cancelled.mark_cancelled();
        let open = Task::new("Open task");
        let filter = FilterSet {
            closed_only: true,
            ..Default::default()
        };
        let result = run(vec![cancelled, open], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Cancelled task");
    }

    #[test]
    fn closed_only_respects_tag_state() {
        let state = state_with(&[("@work", TagState::Included)]);

        let mut done_work = Task::new("Done work task");
        done_work.mark_done(today());
        done_work.tags = vec!["@work".into()];

        let mut done_home = Task::new("Done home task");
        done_home.mark_done(today());
        done_home.tags = vec!["@home".into()];

        let filter = FilterSet {
            closed_only: true,
            ..Default::default()
        };
        let result = apply(vec![done_work, done_home], &filter, &state, today());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Done work task");
    }

    #[test]
    fn closed_only_hides_future_tasks_by_default() {
        let mut done_future = Task::new("Done future task");
        done_future.mark_done(today());
        done_future.start = Some(today() + chrono::Duration::days(7));

        let filter = FilterSet {
            closed_only: true,
            ..Default::default()
        };
        let result = run(vec![done_future], filter);
        assert!(result.is_empty());
    }

    #[test]
    fn disable_implicit_shows_future_tasks() {
        let mut future = Task::new("Future task");
        future.start = Some(today() + chrono::Duration::days(7));
        let filter = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        let result = run(vec![future], filter);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn closed_only_shows_done_task_with_open_blocker() {
        let blocker = Task::new("blocker");
        let mut done = Task::new("done but blocked");
        done.blocked_by = vec![blocker.id];
        done.mark_done(today());

        let filter = FilterSet {
            closed_only: true,
            ..Default::default()
        };
        let result = run(vec![blocker, done], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "done but blocked");
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
            ..query("+#keep")
        };
        let result = run(vec![done, done_no_tag], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "Done with tag");
    }

    #[test]
    fn all_and_closed_together_returns_only_closed() {
        let open = Task::new("open task");
        let mut done = Task::new("done task");
        done.mark_done(today());
        let filter = FilterSet {
            disable_implicit: true,
            closed_only: true,
            ..Default::default()
        };
        let result = run(vec![open, done], filter);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "done task");
    }
}
