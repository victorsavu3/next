//! Integration tests for the archive pass (`next archive`).

mod common;

use chrono::NaiveDate;
use next::core::archiver::run_archive_pass;
use next::core::store::TaskQuery;
use next::core::domain::task::Task;

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
        .query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() })
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
            .query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() })
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
        next::core::storage::archive::read_segment(&root.join("archive/2025/10-003.toml"))
            .unwrap();
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

    let after = env.ctx.repo.store().task_dates().unwrap()[&old.id].clone();
    assert_eq!(after.created_at, before.created_at);

    // The frozen dates live in the segment file itself.
    let entries =
        next::core::storage::archive::read_segment(
            &env.ctx.repo.repo_root.join("archive/2025/09-001.toml"),
        )
        .unwrap();
    assert_eq!(entries[0].created_at, Some(before.created_at));
}
