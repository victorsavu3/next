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
use crate::core::domain::filter_expr::{Atom, Expr, Field, Named};
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

/// The implicit status gate a query's own status predicate contradicts.
///
/// Nothing here changes what matches — the caller uses it to explain an empty
/// result. See [`contradicted_status_gate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContradictedGate {
    /// The default listing, which shows only open and started tasks.
    Active,
    /// `--closed`, which shows only done and cancelled ones.
    Closed,
}

/// Whether `filter`'s status gate can only ever contradict the status
/// predicate in its own query.
///
/// `next list status:done` is unconditionally empty: the gate admits open and
/// started tasks, the query asks for done ones, and no task can be both. That
/// is correct — the gate is what `--all` exists to lift — but on its own it
/// reads as "there are no such tasks", which is a different statement. The
/// answer is a hint, not a widened query: auto-lifting the gate from a query
/// term would change which tasks a command returns, and that is a decision of
/// its own.
///
/// `None` means the query and the gate can agree on at least one status, which
/// includes every query that says nothing about status at all. A `--all`
/// listing has no gate to contradict, so it is always `None`.
///
/// The caller should also check that the result really was empty: a query can
/// contradict the gate on one branch and still match on another.
pub fn contradicted_status_gate(filter: &FilterSet) -> Option<ContradictedGate> {
    if filter.disable_implicit && !filter.closed_only {
        return None;
    }
    let (gate, admitted) = if filter.closed_only {
        (ContradictedGate::Closed, CLOSED)
    } else {
        (ContradictedGate::Active, ACTIVE)
    };
    let satisfiable = satisfiable_statuses(&filter.expr, false);
    // A query satisfiable at *no* status — `status:done and status:open` —
    // matches nothing on its own merits, exactly as `status:nonesuch` does.
    // The gate is not why it came back empty and `--all` would not help, so
    // blaming the gate would be a promise the hint cannot keep.
    (satisfiable != 0 && satisfiable & admitted == 0).then_some(gate)
}

// A set of statuses, one bit each, in `Status` declaration order.
const OPEN: u8 = 1;
const STARTED: u8 = 1 << 1;
const DONE: u8 = 1 << 2;
const CANCELLED: u8 = 1 << 3;
const ACTIVE: u8 = OPEN | STARTED;
const CLOSED: u8 = DONE | CANCELLED;
const ANY: u8 = ACTIVE | CLOSED;

/// The statuses a task could have and still satisfy `expr` (or fail it, when
/// `negated`).
///
/// An over-approximation, deliberately: every non-status atom is treated as
/// freely satisfiable at any status, so the result is never smaller than the
/// true set. An empty result therefore proves the expression is unsatisfiable
/// at those statuses, which is what the hint claims. Status atoms are pure
/// functions of the status, so negating one is exact — that is what makes
/// `not status:done` come out as the other three rather than as "anything".
fn satisfiable_statuses(expr: &Expr, negated: bool) -> u8 {
    match expr {
        // De Morgan: a negated conjunction fails as soon as one part does.
        Expr::And(parts) | Expr::Or(parts) => {
            let disjunction = matches!(expr, Expr::Or(_)) != negated;
            if disjunction {
                parts
                    .iter()
                    .fold(0, |acc, p| acc | satisfiable_statuses(p, negated))
            } else {
                parts
                    .iter()
                    .fold(ANY, |acc, p| acc & satisfiable_statuses(p, negated))
            }
        }
        Expr::Not(inner) => satisfiable_statuses(inner, !negated),
        Expr::Atom(atom) => {
            let matched = match atom {
                Atom::Equals {
                    field: Field::Status,
                    values,
                } => {
                    let mut set = 0;
                    for value in values {
                        match crate::core::domain::filter_eval::status_from(&value.raw) {
                            Some(Status::Open) => set |= OPEN,
                            Some(Status::Started) => set |= STARTED,
                            Some(Status::Done) => set |= DONE,
                            Some(Status::Cancelled) => set |= CANCELLED,
                            // An unspellable status matches nothing at any
                            // status, so the gate is not why the result is
                            // empty. Bow out rather than blame it.
                            None => return ANY,
                        }
                    }
                    set
                }
                Atom::Is(Named::Closed) => CLOSED,
                // Everything else — including `status<x`, which the evaluator
                // does not answer — says nothing about status.
                _ => return ANY,
            };
            if negated {
                ANY & !matched
            } else {
                matched
            }
        }
    }
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

    fn gate(query: &str, closed: bool, all: bool) -> Option<ContradictedGate> {
        contradicted_status_gate(&FilterSet {
            expr: crate::core::domain::filter_expr::parse(query).unwrap(),
            closed_only: closed,
            disable_implicit: all,
            ..Default::default()
        })
    }

    #[test]
    fn a_status_the_gate_excludes_is_reported() {
        for query in ["status:done", "status:cancelled", "is:closed"] {
            assert_eq!(
                gate(query, false, false),
                Some(ContradictedGate::Active),
                "{query} cannot match an open-only listing"
            );
        }
        for query in ["status:open", "status:started", "not is:closed"] {
            assert_eq!(
                gate(query, true, false),
                Some(ContradictedGate::Closed),
                "{query} cannot match a --closed listing"
            );
        }
    }

    #[test]
    fn a_query_the_gate_agrees_with_is_not_reported() {
        // Nothing about status at all, and status terms the gate admits.
        for query in ["", "+@work", "status:open", "is:closed or status:open"] {
            assert_eq!(gate(query, false, false), None, "{query}");
        }
        // `--all` has no gate left to contradict.
        assert_eq!(gate("status:done", false, true), None);
        // An unknown status name matches nothing on its own merits; the gate
        // is not the reason, so it must not be blamed.
        assert_eq!(gate("status:nonesuch", false, false), None);
    }

    /// A query no task can satisfy at any status is empty on its own merits,
    /// so the gate must not take the blame — `--all` would return nothing
    /// either, and the hint promises that it helps.
    #[test]
    fn a_self_contradictory_query_does_not_blame_the_gate() {
        for query in [
            "status:done and status:open",
            "is:closed and status:started",
            "status:open and not status:open",
        ] {
            assert_eq!(gate(query, false, false), None, "{query}");
            assert_eq!(gate(query, true, false), None, "{query} under --closed");
        }
    }

    #[test]
    fn a_contradiction_needs_every_branch_to_contradict() {
        // One satisfiable branch is enough to keep quiet …
        assert_eq!(gate("status:done or +@work", false, false), None);
        // … but an `and` has to satisfy both.
        assert_eq!(
            gate("status:done and +@work", false, false),
            Some(ContradictedGate::Active)
        );
        // Negation is exact for a status atom, so `-status:done` still leaves
        // the open ones — and negating the whole disjunction removes them.
        assert_eq!(gate("not status:done", false, false), None);
        assert_eq!(
            gate("not (status:open or status:started)", false, false),
            Some(ContradictedGate::Active)
        );
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
