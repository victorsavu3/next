//! Renaming a tag across the whole repository.
//!
//! A tag lives in four places, and a rename that misses any of them leaves
//! the repository inconsistent:
//!
//! - the `tags` list of every active task (`tasks/*.toml`);
//! - the `tags` list of every archived task — warm segments under
//!   `archive/`, and cold (pruned) segments that live only as git blobs;
//! - the tag metadata file (`tags/<tag>.toml`);
//! - machine-local state (active/excluded contexts, resource availability).
//!
//! Renaming is hierarchical: renaming `@work` also moves `@work/frontend` to
//! `<new>/frontend`, because a filter on a parent segment matches every
//! descendant and leaving the children behind would silently split the tree.
//!
//! Everything that is committed happens inside a single repository
//! transaction and produces one commit; machine-local state is updated
//! afterwards under the state lock (it is never committed).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::core::domain::tag::{self, tag_matches, TagMeta};
use crate::core::progress::ProgressSink;
use crate::core::storage::archive::{
    parse_segment, read_manifest, read_segment, segment_paths, write_segment,
};
use crate::core::store::{Store, VcsBackend};
use crate::core::task_repository::TaskRepository;

/// What a rename touched.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RenameOutcome {
    /// Active-tier tasks whose tag list changed.
    pub active_tasks: usize,
    /// Archived tasks whose tag list changed (warm and cold tiers).
    pub archived_tasks: usize,
    /// Repo-relative paths of the archive segments rewritten.
    pub segments: Vec<String>,
    /// Repo-relative paths of pruned (cold) segments brought back into the
    /// checkout to be rewritten. They re-prune on a later archive pass.
    pub restored_segments: Vec<String>,
    /// Tag metadata files moved, as `(from, to)`.
    pub metas_moved: Vec<(String, String)>,
    /// Source tags whose metadata was discarded because the destination
    /// already had its own (`--merge` only).
    pub metas_dropped: Vec<String>,
    /// Machine-local state fields that referenced the tag and were updated.
    pub state_fields: Vec<&'static str>,
}

impl RenameOutcome {
    /// Whether the rename found anything at all to change.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Renames `old` (and every tag nested under it) to `new`.
///
/// With `merge` unset, the rename refuses to run when any destination tag is
/// already in use; with `merge` set, destinations may exist — a task ending
/// up with the same tag twice keeps one copy, and where both sides carry tag
/// metadata the destination's is kept and the source's is dropped.
///
/// # Progress
///
/// One phase per tier it rewrites (see [`crate::core::progress`]), each a
/// determinate bar and each begun only when that tier has entries at all — a
/// repository with no archive reports a single bar, not three. The conflict
/// scan that runs without `--merge` gets its own spinner ahead of them; see
/// `rename_in_repo`.
pub fn rename_tag(
    repo: &mut TaskRepository,
    old: &str,
    new: &str,
    merge: bool,
) -> anyhow::Result<RenameOutcome> {
    validate_rename(old, new)?;

    // The transaction closure is not handed the repository, so the sink is
    // cloned out first and moved in.
    let progress = repo.progress_handle();
    let mut outcome = repo.transaction(move |store, vcs, root| {
        rename_in_repo(store, vcs, root, old, new, merge, &*progress)
    })?;
    outcome.state_fields = repo.state_transaction(|store| rename_in_state(store, old, new))?;
    Ok(outcome)
}

/// Rejects renames that cannot have a well-defined result.
fn validate_rename(old: &str, new: &str) -> anyhow::Result<()> {
    tag::validate_tag(old).map_err(|e| anyhow::anyhow!("{e}"))?;
    tag::validate_tag(new).map_err(|e| anyhow::anyhow!("{e}"))?;
    if old == new {
        anyhow::bail!("tag {old:?} is already named that");
    }
    // The leading character decides how a tag filters (context / resource /
    // freeform), so changing it is a reclassification, not a rename.
    if tag::classify(old) != tag::classify(new) {
        anyhow::bail!(
            "cannot rename {old:?} to {new:?}: a tag's leading character decides its kind \
             (@context, #resource, or freeform) and must not change"
        );
    }
    // Renaming into one's own subtree makes the destination of one tag the
    // source of another; there is no sensible single-pass result.
    if tag_matches(old, new) {
        anyhow::bail!("cannot rename {old:?} into its own subtree ({new:?})");
    }
    Ok(())
}

/// Maps a tag through the rename, or `None` when it is unaffected.
fn map_tag(tag: &str, old: &str, new: &str) -> Option<String> {
    tag_matches(old, tag).then(|| format!("{new}{}", &tag[old.len()..]))
}

/// Rewrites a task's tag list in place, de-duplicating tags that collide
/// after the rename. Returns whether anything changed.
fn rewrite_tags(tags: &mut Vec<String>, old: &str, new: &str) -> bool {
    if !tags.iter().any(|t| tag_matches(old, t)) {
        return false;
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(tags.len());
    for t in tags.iter() {
        let mapped = map_tag(t, old, new).unwrap_or_else(|| t.clone());
        if seen.insert(mapped.clone()) {
            out.push(mapped);
        }
    }
    *tags = out;
    true
}

/// The committed half: tasks (both tiers) and tag metadata, as one commit.
///
/// Progress is reported as one determinate bar per tier, because that is how
/// the cost actually divides: a repository may hold ten thousand active tasks
/// and no archive, or a handful of active tasks and hundreds of segments, and
/// a single bar over their sum would move at three different speeds. Each is
/// skipped entirely when its tier is empty. The conflict scan gets a spinner
/// of its own rather than being folded into those bars: it is a *second* full
/// traversal of all three tiers (it has to know every tag in use before the
/// first write), so charging its cost to the rewrite bars would make them
/// stall at zero for half the wall time and claim the rewrite was to blame.
/// It is a spinner rather than a bar because it spans all three tiers at once
/// and they share no unit.
fn rename_in_repo(
    store: &mut dyn Store,
    vcs: &dyn VcsBackend,
    root: &std::path::Path,
    old: &str,
    new: &str,
    merge: bool,
    progress: &dyn ProgressSink,
) -> anyhow::Result<RenameOutcome> {
    let metas = store.list_tag_metas()?;
    if !merge {
        let checking = progress.begin("Checking destination tags", None);
        reject_destination_conflicts(store, root, &metas, old, new)?;
        checking.finish(None);
    }

    let mut outcome = RenameOutcome::default();
    let mut commit_paths: Vec<PathBuf> = Vec::new();

    // ── Active tier ───────────────────────────────────────────────────────
    let tasks = store.list_tasks()?;
    let rewriting =
        (!tasks.is_empty()).then(|| progress.begin("Rewriting tasks", Some(tasks.len() as u64)));
    for mut task in tasks {
        if let Some(rewriting) = &rewriting {
            // Ticked per task examined, not per task changed: the read and the
            // tag scan happen either way, and a bar that only moved on matches
            // would sit still through a rename that touches one task in a
            // thousand.
            rewriting.inc(1);
        }
        if rewrite_tags(&mut task.tags, old, new) {
            commit_paths.push(crate::core::storage::task_path(root, &task));
            store.save_task(&task)?;
            outcome.active_tasks += 1;
        }
    }
    if let Some(rewriting) = &rewriting {
        rewriting.finish(None);
    }
    drop(rewriting);

    // ── Warm tier: segment files in the checkout ──────────────────────────
    let segments = segment_paths(root)?;
    let warm = (!segments.is_empty())
        .then(|| progress.begin("Rewriting archive segments", Some(segments.len() as u64)));
    for (rel, abs) in segments {
        if let Some(warm) = &warm {
            // `rel` is a `String` the loop already owns — nothing is formatted.
            warm.set_message(&rel);
            warm.inc(1);
        }
        let mut entries = read_segment(&abs)?;
        let mut changed = 0;
        for entry in &mut entries {
            if rewrite_tags(&mut entry.task.tags, old, new) {
                changed += 1;
            }
        }
        if changed == 0 {
            continue;
        }
        // Mirror into the cache before the file write, mirroring the order
        // resurrection uses: rows at this path are replaced wholesale.
        store.note_archived_segment(&rel, &entries)?;
        write_segment(&abs, entries)?;
        commit_paths.push(abs);
        outcome.segments.push(rel);
        outcome.archived_tasks += changed;
    }
    if let Some(warm) = &warm {
        warm.finish(None);
    }
    drop(warm);

    // ── Cold tier: pruned segments, recovered from their manifest blob ────
    //
    // A pruned segment is out of the checkout, so it can only be rewritten
    // by restoring it. That un-prunes it — a later archive pass prunes it
    // again and appends a fresh manifest line, which is exactly what a
    // resurrection does — but leaving it alone would keep the old tag alive
    // in tasks the rename claims to have covered.
    //
    // The "already handled as a warm segment above" skip is hoisted into a
    // filter so the bar knows its total before the first blob read; it costs
    // one `exists()` per manifest entry, which the loop paid anyway, and it
    // cannot change what is processed because `read_manifest` already
    // de-duplicates by path.
    let pruned_segments: Vec<_> = read_manifest(root)?
        .into_iter()
        .filter(|pruned| !root.join(&pruned.path).exists())
        .collect();
    let cold = (!pruned_segments.is_empty()).then(|| {
        progress.begin(
            "Rewriting pruned segments",
            Some(pruned_segments.len() as u64),
        )
    });
    for pruned in pruned_segments {
        if let Some(cold) = &cold {
            cold.set_message(&pruned.path);
            // Advanced before the read, so a blob the checks below skip as
            // unreachable or unchanged still counts towards the total.
            cold.inc(1);
        }
        let abs = root.join(&pruned.path);
        let Some(content) = crate::core::storage::git_backend::blob_content(root, &pruned.blob)
        else {
            tracing::warn!(
                "pruned segment {} (blob {}) is unreachable; tasks inside keep the old tag",
                pruned.path,
                pruned.blob
            );
            continue;
        };
        let mut entries = parse_segment(&content)?;
        let mut changed = 0;
        for entry in &mut entries {
            if rewrite_tags(&mut entry.task.tags, old, new) {
                changed += 1;
            }
        }
        if changed == 0 {
            continue;
        }
        store.note_archived_segment(&pruned.path, &entries)?;
        write_segment(&abs, entries)?;
        commit_paths.push(abs);
        outcome.restored_segments.push(pruned.path.clone());
        outcome.segments.push(pruned.path);
        outcome.archived_tasks += changed;
    }
    if let Some(cold) = &cold {
        cold.finish(None);
    }
    drop(cold);

    // ── Tag metadata files ────────────────────────────────────────────────
    let mut affected: Vec<(String, TagMeta)> = metas
        .iter()
        .filter(|(t, _)| tag_matches(old, t))
        .map(|(t, m)| (t.clone(), m.clone()))
        .collect();
    affected.sort_by(|a, b| a.0.cmp(&b.0));
    for (src, meta) in affected {
        let dst = map_tag(&src, old, new).expect("filtered on tag_matches");
        commit_paths.push(crate::core::storage::tag_meta_path(root, &src));
        commit_paths.push(crate::core::storage::tag_meta_path(root, &dst));
        store.delete_tag_meta(&src)?;
        if metas.contains_key(&dst) {
            // Merge: the destination is the surviving identity, so its own
            // metadata wins and the source's is discarded.
            outcome.metas_dropped.push(src);
        } else {
            store.set_tag_meta(&dst, meta)?;
            outcome.metas_moved.push((src, dst));
        }
    }

    outcome.segments.sort();
    outcome.restored_segments.sort();
    if commit_paths.is_empty() {
        return Ok(outcome);
    }
    commit_paths.sort();
    commit_paths.dedup();
    vcs.commit(&commit_paths, &format!("next: tag rename {old} -> {new}"))?;
    Ok(outcome)
}

/// Fails when the rename would land on a tag that is already in use.
fn reject_destination_conflicts(
    store: &dyn Store,
    root: &std::path::Path,
    metas: &HashMap<String, TagMeta>,
    old: &str,
    new: &str,
) -> anyhow::Result<()> {
    let mut in_use: HashSet<String> = metas.keys().cloned().collect();
    for task in store.list_tasks()? {
        in_use.extend(task.tags);
    }
    for (_, abs) in segment_paths(root)? {
        for entry in read_segment(&abs)? {
            in_use.extend(entry.task.tags);
        }
    }
    for pruned in read_manifest(root)? {
        if root.join(&pruned.path).exists() {
            continue;
        }
        if let Some(content) = crate::core::storage::git_backend::blob_content(root, &pruned.blob) {
            for entry in parse_segment(&content)? {
                in_use.extend(entry.task.tags);
            }
        }
    }

    let mut sources: Vec<&String> = in_use.iter().filter(|t| tag_matches(old, t)).collect();
    sources.sort();
    for src in sources {
        let dst = map_tag(src, old, new).expect("filtered on tag_matches");
        if in_use.contains(&dst) {
            anyhow::bail!(
                "tag {dst:?} already exists (renaming {src:?}); \
                 pass --merge to fold {src:?} into it"
            );
        }
    }
    Ok(())
}

/// The machine-local half: the per-tag state in `state.toml`. Returns the
/// names of the fields that changed.
///
/// Unification made this uniform — one map, keyed by the full tag with its
/// sigil, for contexts, resources and freeform labels alike. There is no
/// longer a bare-name special case to get wrong.
fn rename_in_state(
    store: &mut dyn Store,
    old: &str,
    new: &str,
) -> anyhow::Result<Vec<&'static str>> {
    let mut state = store.get_state()?;
    if !state.tags.keys().any(|t| tag_matches(old, t)) {
        return Ok(Vec::new());
    }
    state.tags = state
        .tags
        .iter()
        .map(|(t, v)| (map_tag(t, old, new).unwrap_or_else(|| t.clone()), *v))
        .collect();
    store.save_state(&state)?;
    Ok(vec!["tags"])
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::NaiveDate;
    use tempfile::TempDir;

    use super::*;
    use crate::core::domain::task::Task;
    use crate::core::progress::testing::{ProgressEvent, RecordingSink};

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn maps_tag_and_descendants() {
        assert_eq!(map_tag("@ai", "@ai", "@next").as_deref(), Some("@next"));
        assert_eq!(
            map_tag("@ai/sub", "@ai", "@next").as_deref(),
            Some("@next/sub")
        );
        assert_eq!(map_tag("@aim", "@ai", "@next"), None, "no slash boundary");
        assert_eq!(map_tag("@other", "@ai", "@next"), None);
    }

    #[test]
    fn rewrite_preserves_unrelated_tags_and_order() {
        let mut t = tags(&["python", "@ai/task-manager", "#printer"]);
        assert!(rewrite_tags(&mut t, "@ai/task-manager", "@ai/next"));
        assert_eq!(t, tags(&["python", "@ai/next", "#printer"]));
    }

    #[test]
    fn rewrite_reports_no_change_for_unaffected_tags() {
        let mut t = tags(&["python", "@work"]);
        assert!(!rewrite_tags(&mut t, "@ai", "@next"));
        assert_eq!(t, tags(&["python", "@work"]));
    }

    #[test]
    fn rewrite_dedupes_collisions() {
        // Merging @old into an already-present @new leaves one copy.
        let mut t = tags(&["@new", "@old", "python"]);
        assert!(rewrite_tags(&mut t, "@old", "@new"));
        assert_eq!(t, tags(&["@new", "python"]));
    }

    #[test]
    fn validation_rejects_bad_renames() {
        assert!(validate_rename("@work", "@work").is_err(), "no-op rename");
        assert!(validate_rename("@work", "#work").is_err(), "kind change");
        assert!(validate_rename("@work", "work").is_err(), "kind change");
        assert!(
            validate_rename("@work", "@work/sub").is_err(),
            "into own subtree"
        );
        assert!(
            validate_rename("@work", "@w!").is_err(),
            "invalid destination"
        );
        assert!(
            validate_rename("@work/sub", "@work").is_ok(),
            "promoting is fine"
        );
        assert!(validate_rename("@work", "@office").is_ok());
    }

    // ── Progress reporting ────────────────────────────────────────────────
    //
    // A rename over a real repository, because the point of the assertions is
    // that each bar's total matches the tier it walks. `RecordingSink` is
    // `#[cfg(test)]` and crate-internal, so these cannot live in
    // `tests/test_tag_rename.rs`.

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    const TODAY: fn() -> NaiveDate = || d(2026, 7, 15);

    /// A repository over a fresh temp git repo, with no sink yet.
    fn setup_repo() -> (TempDir, TaskRepository) {
        let dir = TempDir::new().unwrap();
        let git = git2::Repository::init(dir.path()).unwrap();
        {
            let mut cfg = git.config().unwrap();
            cfg.set_str("user.name", "Test").unwrap();
            cfg.set_str("user.email", "test@test.com").unwrap();
        }
        let (store, vcs) = crate::core::storage::open(dir.path().to_path_buf()).unwrap();
        let repo =
            TaskRepository::with_parts(Box::new(store), Box::new(vcs), dir.path().to_path_buf());
        (dir, repo)
    }

    fn add_committed(repo: &mut TaskRepository, task: &Task) {
        repo.transaction(|store, vcs, root| {
            store.save_task(task)?;
            vcs.commit(
                &[crate::core::storage::task_path(root, task)],
                &format!("next: add {:?}", task.title),
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn tagged(title: &str, slug: &str, list: &[&str]) -> Task {
        let mut t = Task::new(title);
        t.slug = Some(slug.to_owned());
        t.tags = tags(list);
        t
    }

    /// Installs a recording sink, *after* the setup commits and archive pass,
    /// so only the rename's own events are recorded.
    fn recording(repo: TaskRepository) -> (TaskRepository, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::new());
        (repo.with_progress(sink.clone()), sink)
    }

    fn determinate_bars(sink: &RecordingSink) -> Vec<String> {
        sink.events()
            .iter()
            .filter_map(|e| match e {
                ProgressEvent::Begin {
                    label,
                    total: Some(_),
                } => Some(label.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_rename_reports_one_bar_per_tier() {
        let (_dir, mut repo) = setup_repo();
        let root = repo.repo_root.clone();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/archive.toml"),
            "archive_after_days = 180\nprune_after_days = 400\n",
        )
        .unwrap();

        // One task per tier: an open one, one that archives warm, and one old
        // enough to be pruned into the cold tier.
        let mut ancient = tagged("Ancient", "ancient", &["@old"]);
        ancient.mark_done(d(2025, 3, 1));
        let mut warm = tagged("Warm", "warm", &["@old", "python"]);
        warm.mark_done(d(2025, 12, 1));
        let open = tagged("Open", "open", &["@old"]);
        for t in [&ancient, &warm, &open] {
            add_committed(&mut repo, t);
        }
        let pass = crate::core::archiver::run_archive_pass(&mut repo, TODAY()).unwrap();
        assert_eq!(pass.archived, 2);
        assert_eq!(pass.pruned, vec!["archive/2025/03-001.toml"]);

        let (mut repo, sink) = recording(repo);
        let outcome = rename_tag(&mut repo, "@old", "@new", false).unwrap();
        assert_eq!(outcome.active_tasks, 1);
        assert_eq!(outcome.archived_tasks, 2);

        // The conflict scan is its own spinner: a second full traversal of all
        // three tiers, with no unit shared between them.
        let checking = sink
            .task("Checking destination tags")
            .expect("a non-merge rename scans first");
        assert_eq!(checking.total, None);
        assert_eq!(checking.finishes, 1);

        let active = sink.task("Rewriting tasks").expect("active tier");
        assert_eq!(active.total, Some(1), "one task left in the active tier");
        assert!(active.completed(), "{active:?}");

        let segments = sink.task("Rewriting archive segments").expect("warm tier");
        assert_eq!(segments.total, Some(1));
        assert!(segments.completed(), "{segments:?}");
        assert_eq!(
            segments.messages,
            vec!["archive/2025/12-001.toml".to_owned()]
        );

        let pruned = sink.task("Rewriting pruned segments").expect("cold tier");
        assert_eq!(
            pruned.total,
            Some(1),
            "only manifest entries missing from the checkout"
        );
        assert!(pruned.completed(), "{pruned:?}");
        assert_eq!(pruned.messages, vec!["archive/2025/03-001.toml".to_owned()]);

        assert_eq!(
            determinate_bars(&sink),
            vec![
                "Rewriting tasks".to_owned(),
                "Rewriting archive segments".to_owned(),
                "Rewriting pruned segments".to_owned(),
            ],
            "one bar per tier, in tier order"
        );
    }

    #[test]
    fn a_rename_omits_the_tiers_with_nothing_in_them() {
        let (_dir, mut repo) = setup_repo();
        add_committed(&mut repo, &tagged("Open", "open", &["@old"]));
        let (mut repo, sink) = recording(repo);

        rename_tag(&mut repo, "@old", "@new", false).unwrap();

        assert_eq!(
            determinate_bars(&sink),
            vec!["Rewriting tasks".to_owned()],
            "a repo with no archive must not flash two empty bars"
        );
    }

    #[test]
    fn a_merge_rename_reports_no_conflict_scan() {
        let (_dir, mut repo) = setup_repo();
        add_committed(&mut repo, &tagged("Open", "open", &["@old", "@new"]));
        let (mut repo, sink) = recording(repo);

        rename_tag(&mut repo, "@old", "@new", true).unwrap();

        assert!(
            sink.task("Checking destination tags").is_none(),
            "--merge skips the scan, so it must not be announced: {:?}",
            sink.labels()
        );
        assert!(sink.task("Rewriting tasks").unwrap().completed());
    }

    #[test]
    fn a_failed_rename_still_finishes_its_bar_once() {
        let (_dir, mut repo) = setup_repo();
        add_committed(&mut repo, &tagged("Open", "open", &["@old"]));
        // An unparsable segment fails the warm-tier loop after its bar has
        // been begun. `--merge` so the conflict scan does not trip on it
        // first. Written after the last commit, so HEAD is unchanged and the
        // cache reconciliation never looks at it.
        let seg = repo.repo_root.join("archive/2025/11-001.toml");
        std::fs::create_dir_all(seg.parent().unwrap()).unwrap();
        std::fs::write(&seg, "this is not toml [[[\n").unwrap();
        let (mut repo, sink) = recording(repo);

        let err = rename_tag(&mut repo, "@old", "@new", true).unwrap_err();
        assert!(err.to_string().contains("parse segment"), "{err}");

        let segments = sink
            .task("Rewriting archive segments")
            .expect("the bar was begun before the failure");
        assert_eq!(
            segments.finishes, 1,
            "the Drop contract cleans up the `?` return exactly once: {segments:?}"
        );
        assert!(
            sink.task("Rewriting pruned segments").is_none(),
            "the cold tier was never reached"
        );
        assert_eq!(
            sink.task("Rewriting tasks").unwrap().finishes,
            1,
            "the earlier phase is not finished twice by the unwind"
        );
    }
}
