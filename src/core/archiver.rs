//! The archive pass: moves eligible closed tasks from `tasks/*.toml` into
//! warm-tier segment files (`archive/<YYYY>/<MM>-<NNN>.toml`).
//!
//! Eligibility (see `config/archive.toml` for the threshold):
//! - status is done or cancelled;
//! - the reference date — `completed_at`, else the git-derived update time —
//!   is older than `archive_after_days` (tasks with no date stay put);
//! - no live recurrence: a task still carrying a `recurrence` rule is the
//!   head of its series unless a newer active-tier task with the same
//!   series id exists;
//! - no child in the active tier is left behind: children must already be
//!   archived or eligible in the same pass (subtrees archive bottom-up).
//!
//! The pass runs as one repository transaction and produces a single commit,
//! so two machines archiving concurrently converge on identical segment
//! bytes (entries are packed in `(completed_at, id)` order).

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use uuid::Uuid;

use crate::core::domain::task::{Status, Task};
use crate::core::error::Result;
use crate::core::storage::archive::{
    load_archive_config, read_segment, segment_paths, segment_rel_path, write_segment,
    ArchiveConfig, ArchivedTask,
};
use crate::core::store::Store;
use crate::core::task_repository::TaskRepository;

/// What an archive pass did.
#[derive(Debug, Default)]
pub struct ArchiveOutcome {
    /// Number of tasks moved into the archive.
    pub archived: usize,
    /// Repo-relative paths of the segments written.
    pub segments: Vec<String>,
}

/// Computes the set of archivable task ids among the active tier.
///
/// `today` fixes the threshold; `updated` supplies the fallback reference
/// date for tasks without `completed_at` (cancelled tasks).
fn eligible_ids(
    active: &[Task],
    updated: &HashMap<Uuid, NaiveDate>,
    config: &ArchiveConfig,
    today: NaiveDate,
) -> HashSet<Uuid> {
    let cutoff = today - chrono::Duration::days(config.archive_after_days as i64);

    // Live-series heads: recurrence_ids of active (open/started) tasks.
    let live_series: HashSet<Uuid> = active
        .iter()
        .filter(|t| t.is_active())
        .filter_map(|t| t.recurrence_id.or(Some(t.id)).filter(|_| t.recurrence.is_some()))
        .collect();

    let mut eligible: HashSet<Uuid> = active
        .iter()
        .filter(|t| matches!(t.status, Status::Done | Status::Cancelled))
        .filter(|t| {
            let reference = t.completed_at.or_else(|| updated.get(&t.id).copied());
            reference.is_some_and(|d| d < cutoff)
        })
        .filter(|t| {
            // A closed task still carrying its rule must not archive while it
            // is the newest instance of its series.
            t.recurrence.is_none() || {
                let series = t.recurrence_id.unwrap_or(t.id);
                live_series.contains(&series)
            }
        })
        .map(|t| t.id)
        .collect();

    // Never leave an active-tier child under an archived parent: a parent is
    // only eligible once all its active-tier children are eligible too.
    // Ineligibility propagates upwards, so iterate to a fixpoint.
    let mut children_of: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for t in active {
        if let Some(pid) = t.parent_id {
            children_of.entry(pid).or_default().push(t.id);
        }
    }
    loop {
        let blocked: Vec<Uuid> = eligible
            .iter()
            .filter(|id| {
                children_of
                    .get(id)
                    .is_some_and(|kids| kids.iter().any(|kid| !eligible.contains(kid)))
            })
            .copied()
            .collect();
        if blocked.is_empty() {
            break;
        }
        for id in blocked {
            eligible.remove(&id);
        }
    }
    eligible
}

/// Runs the archive pass on `repo`. Returns what happened; an outcome with
/// `archived == 0` means nothing was eligible (no commit is made).
pub fn run_archive_pass(repo: &mut TaskRepository, today: NaiveDate) -> Result<ArchiveOutcome> {
    let config = load_archive_config(&repo.repo_root);
    let root = repo.repo_root.clone();

    repo.transaction(|store, vcs, repo_root| {
        let active = store.list_tasks()?;
        let dates = store.task_dates()?;
        let updated: HashMap<Uuid, NaiveDate> =
            dates.iter().map(|(id, d)| (*id, d.updated_at.date_naive())).collect();

        let eligible = eligible_ids(&active, &updated, &config, today);
        if eligible.is_empty() {
            return Ok(ArchiveOutcome::default());
        }

        // Deterministic packing order across machines.
        let mut to_archive: Vec<&Task> =
            active.iter().filter(|t| eligible.contains(&t.id)).collect();
        to_archive.sort_by_key(|t| (t.completed_at, t.id));

        // Drop the individual files (and their active cache rows) first;
        // note_archived_segment below re-inserts each task as an archived
        // row pointing at its segment.
        let mut commit_paths = Vec::new();
        for task in &to_archive {
            commit_paths.push(crate::core::storage::task_path(repo_root, task));
            store.delete_task(task.id)?;
        }

        // Group by month of the reference date and append to each month's
        // open segment, sealing at the size cap.
        let mut outcome = ArchiveOutcome::default();
        let mut by_month: HashMap<(i32, u32), Vec<&Task>> = HashMap::new();
        for task in &to_archive {
            use chrono::Datelike as _;
            let reference = task
                .completed_at
                .or_else(|| updated.get(&task.id).copied())
                .expect("eligibility guarantees a reference date");
            by_month.entry((reference.year(), reference.month())).or_default().push(task);
        }

        let existing = segment_paths(repo_root)?;
        for ((year, month), tasks) in by_month {
            let month_prefix = format!("archive/{year:04}/{month:02}-");
            // Continue the month's highest-numbered segment if it has room.
            let mut seg_no: u32 = existing
                .iter()
                .filter_map(|(rel, _)| {
                    rel.strip_prefix(&month_prefix)?.strip_suffix(".toml")?.parse().ok()
                })
                .max()
                .unwrap_or(1);
            let month_anchor =
                NaiveDate::from_ymd_opt(year, month, 1).expect("valid month from date");
            let mut entries = read_segment(&repo_root.join(segment_rel_path(month_anchor, seg_no)))?;

            for task in tasks {
                if entries.len() >= config.segment_max_tasks {
                    // Seal the current segment and start the next one.
                    flush_segment(store, repo_root, month_anchor, seg_no, &entries, &mut commit_paths, &mut outcome)?;
                    seg_no += 1;
                    entries = Vec::new();
                }
                let frozen = dates.get(&task.id);
                entries.push(ArchivedTask {
                    created_at: frozen.map(|d| d.created_at),
                    updated_at: frozen.map(|d| d.updated_at),
                    task: (*task).clone(),
                });
                outcome.archived += 1;
            }
            flush_segment(store, repo_root, month_anchor, seg_no, &entries, &mut commit_paths, &mut outcome)?;
        }

        outcome.segments.sort();
        vcs.commit(
            &commit_paths,
            &format!("next: archive {} task(s)", outcome.archived),
        )?;
        Ok(outcome)
    })
    .map_err(|e| crate::core::error::TaskError::Other(format!("archive pass in {}: {e}", root.display())))
}

/// Brings an archived task back to the active tier so a mutation can apply
/// to it normally: removes its entry from the segment (deleting an emptied
/// segment file), restores `tasks/<file>.toml`, and flips the cache row —
/// the frozen creation date survives, so age scoring is unaffected.
///
/// Returns the absolute segment path to include in the mutation's commit,
/// or `None` when the task was already active. Must run inside the caller's
/// repository transaction. A later archive pass re-archives the task if it
/// becomes eligible again.
pub fn resurrect_if_archived(
    store: &mut dyn Store,
    repo_root: &std::path::Path,
    id: Uuid,
) -> Result<Option<std::path::PathBuf>> {
    use crate::core::store::TaskLocation;

    let TaskLocation::Archived(rel) = store.task_location(id)? else {
        return Ok(None);
    };
    let abs = repo_root.join(&rel);
    let mut entries = read_segment(&abs)?;
    let pos = entries.iter().position(|e| e.task.id == id).ok_or_else(|| {
        crate::core::error::TaskError::Other(format!(
            "cache row for {id} points at {rel}, but the segment has no such entry"
        ))
    })?;
    let removed = entries.remove(pos);

    // Order matters: flip the row first (so it no longer lives at the
    // segment path), then re-mirror the remaining entries, which clears
    // every row still at that path.
    store.resurrect_task(&removed.task)?;
    store.note_archived_segment(&rel, &entries)?;
    write_segment(&abs, entries)?;
    Ok(Some(abs))
}

/// Writes one segment to disk, mirrors it into the cache, and records the
/// path for the commit.
fn flush_segment(
    store: &mut dyn Store,
    repo_root: &std::path::Path,
    month_anchor: NaiveDate,
    seg_no: u32,
    entries: &[ArchivedTask],
    commit_paths: &mut Vec<std::path::PathBuf>,
    outcome: &mut ArchiveOutcome,
) -> Result<()> {
    let rel = segment_rel_path(month_anchor, seg_no);
    let abs = repo_root.join(&rel);
    write_segment(&abs, entries.to_vec())?;
    store.note_archived_segment(&rel, entries)?;
    commit_paths.push(abs);
    outcome.segments.push(rel);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::archive::ArchiveConfig;

    fn done_on(title: &str, date: NaiveDate) -> Task {
        let mut t = Task::new(title);
        t.mark_done(date);
        t
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    const TODAY: fn() -> NaiveDate = || d(2026, 7, 15);

    fn ids(set: &HashSet<Uuid>) -> usize {
        set.len()
    }

    #[test]
    fn old_closed_tasks_are_eligible_fresh_ones_not() {
        let old = done_on("Old", d(2025, 6, 1));
        let fresh = done_on("Fresh", d(2026, 7, 1));
        let open = Task::new("Open");
        let active = vec![old.clone(), fresh, open];

        let got = eligible_ids(&active, &HashMap::new(), &ArchiveConfig::default(), TODAY());
        assert_eq!(ids(&got), 1);
        assert!(got.contains(&old.id));
    }

    #[test]
    fn cancelled_uses_git_updated_fallback() {
        let mut cancelled = Task::new("Cancelled");
        cancelled.mark_cancelled();
        let mut dateless = Task::new("Dateless");
        dateless.mark_cancelled();

        let updated = HashMap::from([(cancelled.id, d(2025, 5, 1))]);
        let active = vec![cancelled.clone(), dateless.clone()];
        let got = eligible_ids(&active, &updated, &ArchiveConfig::default(), TODAY());
        assert!(got.contains(&cancelled.id), "old by git update date");
        assert!(!got.contains(&dateless.id), "no reference date → stays put");
    }

    #[test]
    fn live_recurrence_head_is_protected() {
        use crate::core::domain::task::Recurrence;
        // Closed instance still carrying the rule, with NO active successor:
        // it is the series head and must stay.
        let mut head = done_on("Series head", d(2025, 6, 1));
        head.recurrence = Some(Recurrence::Completion { interval_days: 7, snap: None });

        let got =
            eligible_ids(&[head.clone()], &HashMap::new(), &ArchiveConfig::default(), TODAY());
        assert!(!got.contains(&head.id));

        // With an active successor in the same series, the old instance goes.
        let mut successor = Task::new("Next instance");
        successor.recurrence = Some(Recurrence::Completion { interval_days: 7, snap: None });
        successor.recurrence_id = Some(head.id);
        let mut old = head.clone();
        old.recurrence_id = None; // first instance: series id defaults to its own id
        let got = eligible_ids(
            &[old.clone(), successor],
            &HashMap::new(),
            &ArchiveConfig::default(),
            TODAY(),
        );
        assert!(got.contains(&old.id));
    }

    #[test]
    fn parent_waits_for_children() {
        let parent_old = done_on("Parent", d(2025, 1, 1));
        let mut open_child = Task::new("Open child");
        open_child.parent_id = Some(parent_old.id);

        // An open child pins the parent.
        let got = eligible_ids(
            &[parent_old.clone(), open_child.clone()],
            &HashMap::new(),
            &ArchiveConfig::default(),
            TODAY(),
        );
        assert!(got.is_empty());

        // A recently-closed child pins the parent too (child not yet eligible).
        let mut fresh_child = done_on("Fresh child", d(2026, 7, 1));
        fresh_child.parent_id = Some(parent_old.id);
        let got = eligible_ids(
            &[parent_old.clone(), fresh_child],
            &HashMap::new(),
            &ArchiveConfig::default(),
            TODAY(),
        );
        assert!(got.is_empty());

        // An old-closed child and parent archive together; ineligibility
        // propagates up a deeper chain.
        let mut old_child = done_on("Old child", d(2025, 2, 1));
        old_child.parent_id = Some(parent_old.id);
        let mut open_grandchild = Task::new("Open grandchild");
        open_grandchild.parent_id = Some(old_child.id);
        let got = eligible_ids(
            &[parent_old.clone(), old_child.clone()],
            &HashMap::new(),
            &ArchiveConfig::default(),
            TODAY(),
        );
        assert_eq!(ids(&got), 2);
        let got = eligible_ids(
            &[parent_old, old_child, open_grandchild],
            &HashMap::new(),
            &ArchiveConfig::default(),
            TODAY(),
        );
        assert!(got.is_empty(), "open grandchild pins the whole chain");
    }
}
