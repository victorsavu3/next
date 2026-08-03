//! Integration tests for the archive pass (`next archive`).

mod common;

use chrono::NaiveDate;
use next::core::archiver::run_archive_pass;
use next::core::domain::task::Task;
use next::core::store::TaskQuery;

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

/// Saves a task and commits it, mimicking a normal mutation.
fn add_committed(env: &mut common::TestEnv, task: &Task) {
    env.ctx
        .repo
        .transaction(|store, vcs, root| {
            store.save_task(task)?;
            vcs.commit(
                &[next::core::storage::task_path(root, task)],
                &format!("next: add {:?}", task.title),
            )?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn archive_pass_moves_old_closed_tasks() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);

    let mut old_done = Task::new("Old done");
    old_done.slug = Some("old-done".into());
    old_done.mark_done(d(2025, 11, 20));
    let mut old_done2 = Task::new("Old done 2");
    old_done2.mark_done(d(2025, 12, 2));
    let mut fresh_done = Task::new("Fresh done");
    fresh_done.mark_done(d(2026, 7, 1));
    let open = Task::new("Still open");

    for t in [&old_done, &old_done2, &fresh_done, &open] {
        add_committed(&mut env, t);
    }

    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.archived, 2);
    assert_eq!(
        outcome.segments,
        vec!["archive/2025/11-001.toml", "archive/2025/12-001.toml"],
        "one segment per completion month"
    );

    // Files moved on disk.
    let root = env.ctx.repo.repo_root.clone();
    assert!(!root.join("tasks/old-done.toml").exists());
    assert!(root.join("archive/2025/11-001.toml").exists());

    // Active views exclude them; archived query and direct lookups see them.
    let store = env.ctx.repo.store();
    assert_eq!(store.list_tasks().unwrap().len(), 2);
    let archived = store
        .query_tasks(&TaskQuery {
            archived: true,
            ..TaskQuery::unpaginated()
        })
        .unwrap();
    assert_eq!(archived.total, 2);
    assert_eq!(store.get_task(old_done.id).unwrap().title, "Old done");
    assert!(store.get_task_by_slug("old-done").unwrap().is_some());

    // The pass committed exactly once and left the tree clean: a second
    // pass finds nothing.
    let outcome2 = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome2.archived, 0);

    // A rebuilt cache (fresh clone situation) agrees.
    use next::core::store::Store as _;
    let (store2, vcs2) = next::core::storage::open(root.clone()).unwrap();
    drop(vcs2);
    assert_eq!(
        store2
            .query_tasks(&TaskQuery {
                archived: true,
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total,
        2
    );
}

#[test]
fn archive_pass_seals_segments_at_cap() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);

    // Lower the cap so the test stays small.
    let root = env.ctx.repo.repo_root.clone();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nsegment_max_tasks = 2\n",
    )
    .unwrap();

    for i in 0..5 {
        let mut t = Task::new(format!("Old {i}"));
        t.mark_done(d(2025, 10, 1 + i));
        add_committed(&mut env, &t);
    }

    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.archived, 5);
    assert_eq!(
        outcome.segments,
        vec![
            "archive/2025/10-001.toml",
            "archive/2025/10-002.toml",
            "archive/2025/10-003.toml",
        ],
        "5 tasks at cap 2 → three segments"
    );

    // A later pass appends to the month's open segment (003 has room).
    let mut late = Task::new("Old straggler");
    late.mark_done(d(2025, 10, 20));
    add_committed(&mut env, &late);
    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.archived, 1);
    assert_eq!(outcome.segments, vec!["archive/2025/10-003.toml"]);

    let entries =
        next::core::storage::archive::read_segment(&root.join("archive/2025/10-003.toml")).unwrap();
    assert_eq!(entries.len(), 2);
}

#[test]
fn archived_tasks_keep_frozen_dates() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);

    let mut old = Task::new("Dated");
    old.mark_done(d(2025, 9, 1));
    add_committed(&mut env, &old);

    let before = env.ctx.repo.store().task_dates().unwrap()[&old.id].clone();

    run_archive_pass(&mut env.ctx.repo, today).unwrap();

    // Frozen dates come from git history (whole seconds); the live cache
    // stamp has sub-second precision, so compare at second granularity.
    let after = env.ctx.repo.store().task_dates().unwrap()[&old.id].clone();
    assert_eq!(after.created_at.timestamp(), before.created_at.timestamp());

    // The frozen dates live in the segment file itself.
    let entries = next::core::storage::archive::read_segment(
        &env.ctx.repo.repo_root.join("archive/2025/09-001.toml"),
    )
    .unwrap();
    assert_eq!(
        entries[0].created_at.unwrap().timestamp(),
        before.created_at.timestamp()
    );
}

#[test]
fn editing_archived_task_resurrects_it() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);

    let mut old = Task::new("Buried");
    old.slug = Some("buried".into());
    old.mark_done(d(2025, 8, 10));
    let mut old2 = Task::new("Buried too");
    old2.mark_done(d(2025, 8, 12));
    add_committed(&mut env, &old);
    add_committed(&mut env, &old2);

    run_archive_pass(&mut env.ctx.repo, today).unwrap();
    let root = env.ctx.repo.repo_root.clone();
    assert!(!root.join("tasks/buried.toml").exists());

    let created_before = env.ctx.repo.store().task_dates().unwrap()[&old.id].created_at;

    // Edit the archived task → it must come back to the active tier.
    use next::core::service::{apply_edits, EditTaskParams};
    let edited = apply_edits(
        old.id,
        EditTaskParams {
            title: Some("Buried, revised".into()),
            ..Default::default()
        },
        today,
        &root,
        &mut *env.ctx.repo.store,
        &*env.ctx.repo.vcs,
    )
    .unwrap();
    assert_eq!(edited.title, "Buried, revised");

    // Back on disk as an individual file; segment shrunk but intact.
    assert!(root.join("tasks/buried.toml").exists());
    let entries =
        next::core::storage::archive::read_segment(&root.join("archive/2025/08-001.toml")).unwrap();
    assert_eq!(entries.len(), 1, "only the untouched task stays archived");
    assert_eq!(entries[0].task.id, old2.id);

    // Cache agrees: active again, still done-status, frozen created_at kept.
    let store = env.ctx.repo.store();
    assert_eq!(
        store
            .query_tasks(&TaskQuery {
                archived: true,
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        store.task_dates().unwrap()[&old.id].created_at.timestamp(),
        created_before.timestamp()
    );

    // The working tree is clean for tracked files (the segment change was
    // committed with the edit); untracked cache artifacts (.next.db etc.)
    // are gitignored in real repos but the test env writes no .gitignore.
    let mut status_cmd = std::process::Command::new("git");
    status_cmd
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(&root);
    // Strip hook-exported repo scoping so this inspects the temp repo even
    // when the suite runs under the pre-commit hook.
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
    ] {
        status_cmd.env_remove(var);
    }
    let dirty = status_cmd.output().unwrap();
    assert!(
        String::from_utf8_lossy(&dirty.stdout).trim().is_empty(),
        "resurrection must leave no uncommitted changes; dirty: {}",
        String::from_utf8_lossy(&dirty.stdout)
    );

    // Still closed and old → the next pass re-archives it.
    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.archived, 1);
    assert!(!root.join("tasks/buried.toml").exists());
    let entries =
        next::core::storage::archive::read_segment(&root.join("archive/2025/08-001.toml")).unwrap();
    assert_eq!(entries.len(), 2, "re-archived into the same month segment");
}

#[test]
fn resurrecting_last_entry_removes_segment_file() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);

    let mut only = Task::new("Loner");
    only.mark_done(d(2025, 7, 1));
    add_committed(&mut env, &only);
    run_archive_pass(&mut env.ctx.repo, today).unwrap();

    let root = env.ctx.repo.repo_root.clone();
    let seg = root.join("archive/2025/07-001.toml");
    assert!(seg.exists());

    use next::core::service::{apply_edits, EditTaskParams};
    apply_edits(
        only.id,
        EditTaskParams {
            notes: Some("back".into()),
            ..Default::default()
        },
        today,
        &root,
        &mut *env.ctx.repo.store,
        &*env.ctx.repo.vcs,
    )
    .unwrap();
    assert!(!seg.exists(), "an emptied segment file is removed");
}

#[test]
fn direct_save_and_delete_of_archived_task_are_rejected() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);

    let mut old = Task::new("Locked away");
    old.mark_done(d(2025, 6, 15));
    add_committed(&mut env, &old);
    run_archive_pass(&mut env.ctx.repo, today).unwrap();

    // A blind save_task must not create a duplicate individual file.
    let mut edited = old.clone();
    edited.title = "Sneaky edit".into();
    let err = env.ctx.repo.store.save_task(&edited).unwrap_err();
    assert!(
        err.to_string().contains("archived"),
        "unexpected error: {err}"
    );

    // A blind delete must not remove the whole segment.
    let err = env.ctx.repo.store.delete_task(old.id).unwrap_err();
    assert!(
        err.to_string().contains("archived"),
        "unexpected error: {err}"
    );
    assert!(env
        .ctx
        .repo
        .repo_root
        .join("archive/2025/06-001.toml")
        .exists());
}

#[test]
fn auto_archive_runs_once_per_day_during_sync() {
    // sync() needs a remote; reuse the pattern from tests/sync.rs with a
    // bare repo as origin.
    let remote = tempfile::TempDir::new().unwrap();
    let mut init_bare = std::process::Command::new("git");
    init_bare
        .args(["init", "--bare", "-q"])
        .current_dir(remote.path());
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
    ] {
        init_bare.env_remove(var);
    }
    assert!(init_bare.status().unwrap().success());

    let mut env = common::setup();
    let root = env.ctx.repo.repo_root.clone();
    common::git(
        &root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );

    let mut old = Task::new("Auto-archived");
    old.mark_done(d(2025, 5, 1));
    add_committed(&mut env, &old);
    common::git(&root, &["push", "-q", "-u", "origin", "HEAD"]);

    // First sync runs the pass (auto defaults on, no last_archive yet).
    next::core::sync(&mut env.ctx.repo, false, false).unwrap();
    assert!(root.join("archive/2025/05-001.toml").exists());
    let state = next::core::sync_state::load(&root).unwrap();
    let first_stamp = state.last_archive.expect("last_archive recorded");

    // Second sync within the day: throttled, stamp unchanged.
    let mut old2 = Task::new("Waits a day");
    old2.mark_done(d(2025, 5, 2));
    add_committed(&mut env, &old2);
    next::core::sync(&mut env.ctx.repo, false, false).unwrap();
    let state = next::core::sync_state::load(&root).unwrap();
    assert_eq!(
        state.last_archive,
        Some(first_stamp),
        "throttled sync must not re-stamp"
    );
    assert_eq!(
        env.ctx
            .repo
            .store()
            .query_tasks(&TaskQuery {
                archived: true,
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total,
        1,
        "second task waits for the next day's pass"
    );

    // Backdate the stamp a day → the pass runs again.
    next::core::sync_state::record_archive(&root, first_stamp - chrono::Duration::days(2)).unwrap();
    next::core::sync(&mut env.ctx.repo, false, false).unwrap();
    assert_eq!(
        env.ctx
            .repo
            .store()
            .query_tasks(&TaskQuery {
                archived: true,
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total,
        2
    );

    // auto = false disables the pass entirely.
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/archive.toml"), "auto = false\n").unwrap();
    let mut old3 = Task::new("Stays put");
    old3.mark_done(d(2025, 5, 3));
    add_committed(&mut env, &old3);
    next::core::sync_state::record_archive(&root, first_stamp - chrono::Duration::days(2)).unwrap();
    next::core::sync(&mut env.ctx.repo, false, false).unwrap();
    assert_eq!(
        env.ctx
            .repo
            .store()
            .query_tasks(&TaskQuery {
                archived: true,
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total,
        2,
        "auto=false must not archive"
    );
}

#[test]
fn prune_moves_segments_to_cold_tier() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);
    let root = env.ctx.repo.repo_root.clone();

    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();

    // One ancient month (past the prune cutoff) and one recent-old month.
    let mut ancient = Task::new("Ancient");
    ancient.slug = Some("ancient".into());
    ancient.mark_done(d(2025, 3, 1));
    let mut warm = Task::new("Warm");
    warm.mark_done(d(2025, 12, 1));
    add_committed(&mut env, &ancient);
    add_committed(&mut env, &warm);

    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.archived, 2);
    assert_eq!(outcome.pruned, vec!["archive/2025/03-001.toml"]);

    // The pruned segment left the checkout; the manifest records it.
    assert!(!root.join("archive/2025/03-001.toml").exists());
    assert!(root.join("archive/2025/12-001.toml").exists());
    let manifest = next::core::storage::archive::read_manifest(&root).unwrap();
    assert_eq!(manifest.len(), 1);
    assert_eq!(manifest[0].tasks, 1);
    let attrs = std::fs::read_to_string(root.join(".gitattributes")).unwrap();
    assert!(attrs.contains("archive/pruned.jsonl merge=union"));

    // Cold tasks stay fully readable: archived query, id and slug lookups.
    let store = env.ctx.repo.store();
    let archived = store
        .query_tasks(&TaskQuery {
            archived: true,
            ..TaskQuery::unpaginated()
        })
        .unwrap();
    assert_eq!(archived.total, 2, "warm + cold both served");
    assert_eq!(store.get_task(ancient.id).unwrap().title, "Ancient");
    assert!(store.get_task_by_slug("ancient").unwrap().is_some());

    // A fresh cache (fresh-clone situation) recovers cold rows from blobs.
    use next::core::store::Store as _;
    let inner =
        next::core::storage::TomlStore::open(root.clone(), root.join("state.toml")).unwrap();
    let vcs = next::core::storage::GitBackend::open(&root).unwrap();
    let head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
    let store2 =
        next::core::storage::CachedStore::open(inner, root.join(".next-cold.db"), &head).unwrap();
    assert_eq!(
        store2
            .query_tasks(&TaskQuery {
                archived: true,
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total,
        2
    );
    assert_eq!(store2.get_task(ancient.id).unwrap().title, "Ancient");

    // A second pass does nothing further.
    let outcome2 = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome2.archived, 0);
    assert!(outcome2.pruned.is_empty());
}

#[test]
fn editing_cold_task_resurrects_it_and_restores_segment() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);
    let root = env.ctx.repo.repo_root.clone();

    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();

    let mut frozen = Task::new("Frozen");
    frozen.mark_done(d(2025, 2, 10));
    let mut neighbour = Task::new("Neighbour");
    neighbour.mark_done(d(2025, 2, 20));
    add_committed(&mut env, &frozen);
    add_committed(&mut env, &neighbour);
    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.pruned, vec!["archive/2025/02-001.toml"]);
    assert!(!root.join("archive/2025/02-001.toml").exists());

    use next::core::service::{apply_edits, EditTaskParams};
    let edited = apply_edits(
        frozen.id,
        EditTaskParams {
            notes: Some("thawed".into()),
            ..Default::default()
        },
        today,
        &root,
        &mut *env.ctx.repo.store,
        &*env.ctx.repo.vcs,
    )
    .unwrap();
    assert_eq!(edited.notes.as_deref(), Some("thawed"));

    // The task is active again; the rest of the segment returned to the
    // checkout (warm) and re-prunes on the next pass.
    assert!(env
        .ctx
        .repo
        .store()
        .list_tasks()
        .unwrap()
        .iter()
        .any(|t| t.id == frozen.id));
    let entries =
        next::core::storage::archive::read_segment(&root.join("archive/2025/02-001.toml")).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].task.id, neighbour.id);

    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.archived, 1, "the thawed task re-archives");
    assert_eq!(
        outcome.pruned,
        vec!["archive/2025/02-001.toml"],
        "the restored segment re-prunes"
    );
    let manifest = next::core::storage::archive::read_manifest(&root).unwrap();
    assert_eq!(manifest.len(), 1, "last manifest line per path wins");
}

#[test]
fn external_prune_reconciles_incrementally() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);
    let root = env.ctx.repo.repo_root.clone();

    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();

    let mut old = Task::new("Elsewhere-pruned");
    old.mark_done(d(2025, 1, 5));
    add_committed(&mut env, &old);
    // Archive (warm) first, without pruning, on "this" machine's view.
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\n",
    )
    .unwrap();
    run_archive_pass(&mut env.ctx.repo, today).unwrap();

    // A second store over the same repo — the "other machine" whose cache
    // must pick the prune up via the incremental reconcile.
    use next::core::store::Store as _;
    let inner =
        next::core::storage::TomlStore::open(root.clone(), root.join("state.toml")).unwrap();
    let vcs = next::core::storage::GitBackend::open(&root).unwrap();
    let head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
    let mut other =
        next::core::storage::CachedStore::open(inner, root.join(".next-other.db"), &head).unwrap();
    assert_eq!(other.get_task(old.id).unwrap().title, "Elsewhere-pruned");

    // Now prune on the first machine.
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();
    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.pruned.len(), 1);

    // The other machine reconciles from the diff: segment delete + manifest
    // append → the task survives as a cold row.
    let new_head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
    other.after_pull(&new_head).unwrap();
    assert_eq!(other.get_task(old.id).unwrap().title, "Elsewhere-pruned");
    assert_eq!(
        other
            .query_tasks(&TaskQuery {
                archived: true,
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total,
        1
    );
}

#[test]
fn pruned_segment_numbers_are_never_reused() {
    let mut env = common::setup();
    let today = d(2026, 7, 15);
    let root = env.ctx.repo.repo_root.clone();

    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();

    let mut first = Task::new("First of month");
    first.mark_done(d(2025, 4, 2));
    add_committed(&mut env, &first);
    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.pruned, vec!["archive/2025/04-001.toml"]);

    // A straggler completed in the same ancient month arrives later
    // (e.g. via resurrection elsewhere): it must open segment 002, not
    // shadow the pruned 001.
    let mut straggler = Task::new("Straggler");
    straggler.mark_done(d(2025, 4, 20));
    add_committed(&mut env, &straggler);
    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.archived, 1);
    assert_eq!(outcome.segments, vec!["archive/2025/04-002.toml"]);
    assert_eq!(
        outcome.pruned,
        vec!["archive/2025/04-002.toml"],
        "and it prunes in turn"
    );

    let manifest = next::core::storage::archive::read_manifest(&root).unwrap();
    let paths: Vec<_> = manifest.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["archive/2025/04-001.toml", "archive/2025/04-002.toml"]
    );

    // Both cold segments' tasks remain reachable.
    let store = env.ctx.repo.store();
    assert!(store.get_task(first.id).is_ok());
    assert!(store.get_task(straggler.id).is_ok());
}

#[test]
fn cold_tasks_recover_inside_a_partial_clone() {
    // Prune in a source repo, then partial-clone it (blob:none): the pruned
    // segment's blob is not transferred, and the fresh cache must recover it
    // through git's on-demand promisor fetch (the cat-file fallback).
    let mut env = common::setup();
    let today = d(2026, 7, 15);
    let src = env.ctx.repo.repo_root.clone();

    std::fs::create_dir_all(src.join("config")).unwrap();
    std::fs::write(
        src.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();
    common::git(&src, &["config", "uploadpack.allowfilter", "true"]);

    let mut cold = Task::new("Cold in clone");
    cold.mark_done(d(2025, 1, 10));
    let open = Task::new("Open in clone");
    add_committed(&mut env, &cold);
    add_committed(&mut env, &open);
    let outcome = run_archive_pass(&mut env.ctx.repo, today).unwrap();
    assert_eq!(outcome.pruned.len(), 1);

    // Partial clone into a fresh directory.
    let base = tempfile::TempDir::new().unwrap();
    let clone = base.path().join("clone");
    common::git(
        base.path(),
        &[
            "clone",
            "--filter=blob:none",
            "--quiet",
            &format!("file://{}", src.display()),
            clone.to_str().unwrap(),
        ],
    );
    common::git(&clone, &["config", "user.email", "test@test.com"]);
    common::git(&clone, &["config", "user.name", "Test"]);

    // Opening the store rebuilds the cache; the cold segment blob is fetched
    // on demand from the promisor remote.
    use next::core::store::Store as _;
    let (store, _vcs) = next::core::storage::open(clone.clone()).unwrap();
    assert_eq!(store.get_task(cold.id).unwrap().title, "Cold in clone");
    assert_eq!(
        store
            .query_tasks(&TaskQuery {
                archived: true,
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total,
        1
    );
    assert_eq!(store.list_tasks().unwrap().len(), 1, "open task is active");
}
