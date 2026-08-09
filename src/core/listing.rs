//! Shared loading for filtered task listings.
//!
//! Every scored listing (CLI `list`/`next`/`forecast`, the MCP views, the
//! TUI) runs the same pipeline: load candidates → `domain::filter::apply` →
//! `scoring::score_and_sort`. These helpers push the status gate down into
//! the store so the common case loads only active tasks, while preserving
//! `filter::apply`'s exact semantics:
//!
//! - its relational checks (open blockers, parents with open children) only
//!   ever look for *open* tasks, and every open task is in the active set;
//! - `--closed` needs only closed tasks, which is its own pushdown;
//! - `--all` (`disable_implicit`) and `parent:` project scoping genuinely
//!   need the full set (a project root or intermediate node may be closed),
//!   so they fall back to loading everything;
//! - so do the relational predicates `is:blocked` and `is:project`, which are
//!   answered from an index over the loaded set: narrow the load and the index
//!   silently acquires holes.

use std::collections::HashSet;

use crate::core::domain::filter::FilterSet;
use crate::core::domain::task::{Status, Task};
use crate::core::error::Result;
use crate::core::store::{Store, TaskQuery};

/// Loads the candidate set for `filter`, status-reduced when possible.
///
/// The result is what `filter::apply` should run on; it is *not* yet
/// filtered.
pub fn load_candidates(store: &dyn Store, filter: &FilterSet) -> Result<Vec<Task>> {
    if filter.disable_implicit
        || filter.parent_slug.is_some()
        || crate::core::domain::filter_expr::needs_full_corpus(&filter.expr)
    {
        return store.list_tasks();
    }
    let statuses = if filter.closed_only {
        vec![Status::Done, Status::Cancelled]
    } else {
        vec![Status::Open, Status::Started]
    };
    Ok(store
        .query_tasks(&TaskQuery {
            statuses: Some(statuses),
            ..TaskQuery::unpaginated()
        })?
        .items)
}

/// Extends `tasks` with any referenced parents that are not in the list, so
/// the result can serve as the parent-lookup pool for
/// [`crate::core::scoring::score_and_sort`] (a closed parent still
/// contributes its priority and tags to a child's score).
pub fn extend_with_parents(store: &dyn Store, mut tasks: Vec<Task>) -> Result<Vec<Task>> {
    let present: HashSet<_> = tasks.iter().map(|t| t.id).collect();
    let missing: Vec<_> = tasks
        .iter()
        .filter_map(|t| t.parent_id)
        .filter(|pid| !present.contains(pid))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if !missing.is_empty() {
        tasks.extend(store.get_tasks(&missing)?);
    }
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::filter::FilterSet;
    use crate::core::storage::TomlStore;

    fn store_with(tasks: &[Task]) -> (tempfile::TempDir, TomlStore) {
        let dir = tempfile::TempDir::new().unwrap();
        let mut store =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        for t in tasks {
            store.save_task(t).unwrap();
        }
        (dir, store)
    }

    #[test]
    fn default_filter_loads_only_active() {
        let mut done = Task::new("Done");
        done.mark_done(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        let open = Task::new("Open");
        let (_dir, store) = store_with(&[done, open]);

        let got = load_candidates(&store, &FilterSet::default()).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "Open");
    }

    #[test]
    fn closed_only_loads_only_closed() {
        let mut done = Task::new("Done");
        done.mark_done(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        let open = Task::new("Open");
        let (_dir, store) = store_with(&[done, open]);

        let filter = FilterSet {
            closed_only: true,
            ..Default::default()
        };
        let got = load_candidates(&store, &filter).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "Done");
    }

    #[test]
    fn disable_implicit_and_parent_slug_load_everything() {
        let mut done = Task::new("Done");
        done.mark_done(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        let open = Task::new("Open");
        let (_dir, store) = store_with(&[done, open]);

        let all = FilterSet {
            disable_implicit: true,
            ..Default::default()
        };
        assert_eq!(load_candidates(&store, &all).unwrap().len(), 2);

        let scoped = FilterSet {
            parent_slug: Some("proj".into()),
            ..Default::default()
        };
        assert_eq!(load_candidates(&store, &scoped).unwrap().len(), 2);
    }

    #[test]
    fn relational_predicates_load_everything() {
        // These are answered from an index over the loaded set, so a status
        // pushdown would put holes in it — a parent whose children are all
        // closed would look childless.
        let mut done = Task::new("Done");
        done.mark_done(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        let open = Task::new("Open");
        let (_dir, store) = store_with(&[done, open]);

        for query in ["is:project", "is:blocked", "+@work and is:project"] {
            let filter = FilterSet {
                expr: crate::core::domain::filter_expr::parse(query).unwrap(),
                ..Default::default()
            };
            assert_eq!(
                load_candidates(&store, &filter).unwrap().len(),
                2,
                "{query} must see closed tasks too"
            );
        }

        // A query that asks nothing relational keeps the status pushdown.
        let filter = FilterSet {
            expr: crate::core::domain::filter_expr::parse("+@work due<+7d").unwrap(),
            ..Default::default()
        };
        assert_eq!(load_candidates(&store, &filter).unwrap().len(), 1);
    }

    #[test]
    fn extend_with_parents_fetches_out_of_set_parent() {
        let mut parent = Task::new("Project");
        parent.mark_done(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        let mut child = Task::new("Child");
        child.parent_id = Some(parent.id);
        let (_dir, store) = store_with(&[parent.clone(), child.clone()]);

        let pool = extend_with_parents(&store, vec![child]).unwrap();
        assert_eq!(pool.len(), 2);
        assert!(pool.iter().any(|t| t.id == parent.id));
    }
}
