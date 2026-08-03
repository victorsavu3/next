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
    append_manifest, ensure_manifest_gitattributes, load_archive_config, read_manifest,
    read_segment, segment_paths, segment_rel_path, write_segment, ArchiveConfig, ArchivedTask,
    PrunedSegment, MANIFEST_REL_PATH,
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
    /// Repo-relative paths of segments pruned to the cold tier.
    pub pruned: Vec<String>,
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
        .filter_map(|t| {
            t.recurrence_id
                .or(Some(t.id))
                .filter(|_| t.recurrence.is_some())
        })
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
        // Freeze dates from git history, not from the cache: incremental
        // cache stamps are per-machine approximations (a task's author
        // stamps its save time; a machine that received it via pull stamps
        // the pulled head's commit time). Segment bytes must be identical
        // when two converged machines archive concurrently, and the
        // history-derived times are the only ones both sides agree on.
        // One `git log --name-only` walk per pass; the cache stays the
        // fallback for files history cannot date (never committed).
        let walk = crate::core::storage::git_backend::task_git_dates(&repo_root.join("tasks"))
            .unwrap_or_default();
        let cache_dates = store.task_dates()?;
        let dates: HashMap<Uuid, crate::core::scoring::TaskDates> = active
            .iter()
            .filter_map(|t| {
                let hex = t.id.simple().to_string();
                let filename = crate::core::storage::filenames::generate_filename(t);
                let filename = filename.as_str();
                walk.get(&hex[..8])
                    .or_else(|| walk.get(filename))
                    .or_else(|| cache_dates.get(&t.id))
                    .map(|d| (t.id, d.clone()))
            })
            .collect();
        let updated: HashMap<Uuid, NaiveDate> = dates
            .iter()
            .map(|(id, d)| (*id, d.updated_at.date_naive()))
            .collect();

        let eligible = eligible_ids(&active, &updated, &config, today);
        if eligible.is_empty() {
            let mut outcome = ArchiveOutcome::default();
            prune_phase(store, vcs, repo_root, &config, today, &mut outcome)?;
            return Ok(outcome);
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
            by_month
                .entry((reference.year(), reference.month()))
                .or_default()
                .push(task);
        }

        let existing = segment_paths(repo_root)?;
        // Pruned paths that are gone from the checkout must never be
        // reused: a fresh segment under a recycled name would shadow the
        // manifest entry. A manifest path *restored* by a resurrection is in
        // the checkout again and safe to append to (re-pruning appends a
        // fresh manifest line, and the last line per path wins).
        let pruned_paths: Vec<String> = read_manifest(repo_root)?
            .into_iter()
            .map(|p| p.path)
            .filter(|p| !repo_root.join(p).exists())
            .collect();
        for ((year, month), tasks) in by_month {
            let month_prefix = format!("archive/{year:04}/{month:02}-");
            let highest = |paths: &mut dyn Iterator<Item = &str>| -> Option<u32> {
                paths
                    .filter_map(|rel| {
                        rel.strip_prefix(&month_prefix)?
                            .strip_suffix(".toml")?
                            .parse()
                            .ok()
                    })
                    .max()
            };
            // Continue the month's highest-numbered checkout segment if it
            // has room; never step back onto a pruned number.
            let in_checkout = highest(&mut existing.iter().map(|(rel, _)| rel.as_str()));
            let in_manifest = highest(&mut pruned_paths.iter().map(String::as_str));
            let mut seg_no: u32 = match (in_checkout, in_manifest) {
                (Some(c), Some(p)) if p >= c => p + 1,
                (Some(c), _) => c,
                (None, Some(p)) => p + 1,
                (None, None) => 1,
            };
            let month_anchor =
                NaiveDate::from_ymd_opt(year, month, 1).expect("valid month from date");
            let mut entries =
                read_segment(&repo_root.join(segment_rel_path(month_anchor, seg_no)))?;

            for task in tasks {
                if entries.len() >= config.segment_max_tasks {
                    // Seal the current segment and start the next one.
                    flush_segment(
                        store,
                        repo_root,
                        month_anchor,
                        seg_no,
                        &entries,
                        &mut commit_paths,
                        &mut outcome,
                    )?;
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
            flush_segment(
                store,
                repo_root,
                month_anchor,
                seg_no,
                &entries,
                &mut commit_paths,
                &mut outcome,
            )?;
        }

        outcome.segments.sort();
        vcs.commit(
            &commit_paths,
            &format!("next: archive {} task(s)", outcome.archived),
        )?;
        // With the warm commit in HEAD, segment blobs are addressable —
        // prune whatever crossed the cold threshold.
        prune_phase(store, vcs, repo_root, &config, today, &mut outcome)?;
        Ok(outcome)
    })
    .map_err(|e| {
        crate::core::error::TaskError::Other(format!("archive pass in {}: {e}", root.display()))
    })
}

/// The cold-tier prune: checkout segments whose newest completion is older
/// than `prune_after_days` are recorded in the append-only manifest (path +
/// blob SHA, union-merged) and removed from the working tree, as one commit.
/// Their cache rows flip to the cold tier; recovery is a single blob read.
fn prune_phase(
    store: &mut dyn Store,
    vcs: &dyn crate::core::store::VcsBackend,
    repo_root: &std::path::Path,
    config: &ArchiveConfig,
    today: NaiveDate,
    outcome: &mut ArchiveOutcome,
) -> Result<()> {
    let Some(prune_days) = config.prune_after_days else {
        return Ok(());
    };
    let cutoff = today - chrono::Duration::days(prune_days as i64);

    let mut manifest_lines = Vec::new();
    let mut commit_paths = Vec::new();
    for (rel, abs) in segment_paths(repo_root)? {
        let entries = read_segment(&abs)?;
        let newest = entries.iter().filter_map(|e| e.task.completed_at).max();
        // A segment with no completion dates at all never prunes — without a
        // reference date it can not be judged old.
        if newest.is_none_or(|d| d >= cutoff) {
            continue;
        }
        let Some(blob) = crate::core::storage::git_backend::blob_id_at_head(repo_root, &rel) else {
            // Not in HEAD (dirty or never committed) — skip; a later pass
            // prunes it once committed.
            continue;
        };
        manifest_lines.push(PrunedSegment {
            path: rel.clone(),
            blob,
            tasks: entries.len(),
        });
        std::fs::remove_file(&abs)
            .map_err(|e| crate::core::error::TaskError::Other(format!("prune {rel}: {e}")))?;
        store.note_cold_segment(&rel)?;
        commit_paths.push(abs);
        outcome.pruned.push(rel);
    }
    if manifest_lines.is_empty() {
        return Ok(());
    }

    append_manifest(repo_root, &manifest_lines)?;
    commit_paths.push(repo_root.join(MANIFEST_REL_PATH));
    if let Some(attrs) = ensure_manifest_gitattributes(repo_root)? {
        commit_paths.push(attrs);
    }
    outcome.pruned.sort();
    vcs.commit(
        &commit_paths,
        &format!(
            "next: prune {} segment(s) to cold tier",
            outcome.pruned.len()
        ),
    )?;
    Ok(())
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
    // Warm segments are read from the checkout; a pruned (cold) segment is
    // recovered from its manifest blob — one object read. The remaining
    // entries are written back as a checkout file, so the whole segment
    // returns to the warm tier (and re-prunes on a later pass).
    let mut entries = if abs.exists() {
        read_segment(&abs)?
    } else {
        let blob = read_manifest(repo_root)?
            .into_iter()
            .find(|p| p.path == rel)
            .map(|p| p.blob)
            .ok_or_else(|| {
                crate::core::error::TaskError::Other(format!(
                    "cache row for {id} points at pruned segment {rel}, which the manifest does not list"
                ))
            })?;
        let content = crate::core::storage::git_backend::blob_content(repo_root, &blob)
            .ok_or_else(|| {
                crate::core::error::TaskError::Other(format!(
                    "pruned segment {rel} (blob {blob}) is unreachable in the object store"
                ))
            })?;
        crate::core::storage::archive::parse_segment(&content)?
    };
    let pos = entries
        .iter()
        .position(|e| e.task.id == id)
        .ok_or_else(|| {
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
        head.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
        });

        let got = eligible_ids(
            &[head.clone()],
            &HashMap::new(),
            &ArchiveConfig::default(),
            TODAY(),
        );
        assert!(!got.contains(&head.id));

        // With an active successor in the same series, the old instance goes.
        let mut successor = Task::new("Next instance");
        successor.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
        });
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
