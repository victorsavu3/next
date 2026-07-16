//! Larger-scale archiving integration tests: a repository with thousands of
//! tasks driven through the full archive → prune lifecycle, and repeated
//! resurrection / re-archive cycles.
//!
//! The corpus is written directly to disk and committed once (the same way
//! a long-lived repo would arrive in a fresh checkout), then everything else
//! goes through the real store, pass, and reconcile machinery.

mod common;

use chrono::NaiveDate;
use next::core::archiver::run_archive_pass;
use next::core::domain::task::Task;
use next::core::store::{Store, TaskQuery};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

const TODAY: fn() -> NaiveDate = || d(2026, 7, 15);

/// Writes `task` straight to `tasks/` without touching a store.
fn write_task_file(root: &std::path::Path, task: &Task) {
    std::fs::write(
        next::core::storage::task_path(root, task),
        toml::to_string_pretty(task).unwrap(),
    )
    .unwrap();
}

#[test]
fn large_repository_lifecycle() {
    // `env` owns the TempDir; keep it alive. Fresh repo handles are opened
    // below over the prepared corpus instead of using env's own.
    let env = common::setup();
    let root = env.ctx.repo.repo_root.clone();

    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();

    // 2 100 tasks: 1 500 closed across five months — three past the archive
    // threshold (one of them past the prune threshold, one month big enough
    // to seal a segment), two recent — plus 600 open.
    let months: [(NaiveDate, usize); 5] = [
        (d(2025, 3, 1), 200),  // ancient → archives, then prunes
        (d(2025, 10, 1), 1100), // old → archives into 2 segments (cap 1000)
        (d(2025, 12, 1), 100), // old → archives
        (d(2026, 6, 1), 60),   // recent → stays active tier
        (d(2026, 7, 1), 40),   // recent → stays active tier
    ];
    let mut done_old = 0usize;
    let mut done_recent = 0usize;
    let mut sample_old = None;
    for (when, count) in months {
        for i in 0..count {
            let mut t = Task::new(format!("Done {when} #{i}"));
            t.mark_done(when);
            if when < d(2026, 1, 1) {
                done_old += 1;
                sample_old.get_or_insert(t.clone());
            } else {
                done_recent += 1;
            }
            write_task_file(&root, &t);
        }
    }
    for i in 0..600 {
        let mut t = Task::new(format!("Open #{i}"));
        if i % 20 == 0 {
            t.tags = vec!["@scale/hot".into()];
        }
        write_task_file(&root, &t);
    }
    common::git(&root, &["add", "-A"]);
    common::git(&root, &["commit", "-q", "-m", "corpus"]);

    // A fresh open rebuilds the cache over the corpus.
    let mut repo = next::core::TaskRepository::open(root.clone()).unwrap();
    assert_eq!(repo.store().list_tasks().unwrap().len(), 2100);

    // A second store opened now simulates another machine that will learn
    // about the pass purely through the incremental reconcile.
    let inner = next::core::storage::TomlStore::open(root.clone(), root.join("state.toml")).unwrap();
    let vcs = next::core::storage::GitBackend::open(&root).unwrap();
    let head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
    let mut other =
        next::core::storage::CachedStore::open(inner, root.join(".next-other.db"), &head).unwrap();

    let outcome = run_archive_pass(&mut repo, TODAY()).unwrap();
    assert_eq!(outcome.archived, done_old);
    assert_eq!(
        outcome.segments,
        vec![
            "archive/2025/03-001.toml",
            "archive/2025/10-001.toml",
            "archive/2025/10-002.toml",
            "archive/2025/12-001.toml",
        ],
        "1100-task month seals one segment at the 1000 cap"
    );
    assert_eq!(outcome.pruned, vec!["archive/2025/03-001.toml"]);

    // Tier accounting: active view, archived view, and pagination windows.
    let store = repo.store();
    assert_eq!(store.list_tasks().unwrap().len(), 600 + done_recent);
    let archived = store
        .query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() })
        .unwrap();
    assert_eq!(archived.total as usize, done_old);

    let mut seen = std::collections::HashSet::new();
    let mut page_no = 1;
    loop {
        let page = store
            .query_tasks(&TaskQuery {
                archived: true,
                page: page_no,
                page_size: 500,
                ..Default::default()
            })
            .unwrap();
        if page.items.is_empty() {
            break;
        }
        for t in &page.items {
            assert!(seen.insert(t.id), "pages must not overlap");
        }
        page_no += 1;
    }
    assert_eq!(seen.len(), done_old, "pages must cover the whole archive");

    // Random access across tiers (the sampled task is in the pruned month or
    // a warm segment; both must resolve).
    let sample = sample_old.unwrap();
    assert_eq!(store.get_task(sample.id).unwrap().title, sample.title);

    // The other machine catches up incrementally — and agrees on every count.
    let new_head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
    other.after_pull(&new_head).unwrap();
    assert_eq!(other.list_tasks().unwrap().len(), 600 + done_recent);
    assert_eq!(
        other
            .query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() })
            .unwrap()
            .total as usize,
        done_old
    );
    assert!(other.get_task(sample.id).is_ok());
}

#[test]
fn resurrection_and_rearchive_cycles() {
    let mut env = common::setup();
    let root = env.ctx.repo.repo_root.clone();

    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();

    // The cycling task shares its segment with a bystander, so segment
    // integrity is visible on every lap.
    let mut cycler = Task::new("Cycler");
    cycler.slug = Some("cycler".into());
    cycler.mark_done(d(2025, 2, 5)); // past both thresholds → archives AND prunes
    let mut bystander = Task::new("Bystander");
    bystander.mark_done(d(2025, 2, 25));
    for t in [&cycler, &bystander] {
        env.ctx
            .repo
            .transaction(|store, vcs, root| {
                store.save_task(t)?;
                vcs.commit(&[next::core::storage::task_path(root, t)], "add")?;
                Ok(())
            })
            .unwrap();
    }

    let created_at = env.ctx.repo.store().task_dates().unwrap()[&cycler.id].created_at;
    let seg = "archive/2025/02-001.toml";

    use next::core::service::{apply_edits, EditTaskParams};
    for cycle in 1..=3 {
        // Archive (and prune — the month is past the cold threshold).
        let outcome = run_archive_pass(&mut env.ctx.repo, TODAY()).unwrap();
        assert_eq!(outcome.archived, if cycle == 1 { 2 } else { 1 }, "cycle {cycle}");
        assert!(outcome.pruned.contains(&seg.to_string()), "cycle {cycle} prunes");
        assert!(!root.join(seg).exists(), "cycle {cycle}: segment left the checkout");

        // Resurrect from the cold tier by editing.
        let edited = apply_edits(
            cycler.id,
            EditTaskParams { notes: Some(format!("cycle {cycle}")), ..Default::default() },
            TODAY(),
            &root,
            &mut *env.ctx.repo.store,
            &*env.ctx.repo.vcs,
        )
        .unwrap();
        assert_eq!(edited.notes.as_deref(), Some(format!("cycle {cycle}").as_str()));

        // Exactly one copy anywhere: active tier has it, the restored
        // segment holds only the bystander.
        let store = env.ctx.repo.store();
        assert_eq!(
            store.query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() }).unwrap().total,
            1,
            "cycle {cycle}: only the bystander stays archived"
        );
        let entries = next::core::storage::archive::read_segment(&root.join(seg)).unwrap();
        assert_eq!(entries.len(), 1, "cycle {cycle}");
        assert_eq!(entries[0].task.id, bystander.id, "cycle {cycle}");
        assert!(store.get_task_by_slug("cycler").unwrap().is_some(), "cycle {cycle}");
        assert_eq!(
            store.task_dates().unwrap()[&cycler.id].created_at,
            created_at,
            "cycle {cycle}: creation date survives the round trip"
        );
    }

    // A final pass re-archives the last resurrection; across every cycle
    // the corpus holds exactly two tasks — no duplicates in any tier.
    let outcome = run_archive_pass(&mut env.ctx.repo, TODAY()).unwrap();
    let store = env.ctx.repo.store();
    let archived_total = store
        .query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() })
        .unwrap()
        .total;
    let active_total = store.list_tasks().unwrap().len() as u64;
    assert_eq!(active_total + archived_total, 2, "no duplicates after cycling: {outcome:?}");

    // The manifest never accumulated stale state: one line per path at most
    // is live, and the cache can be rebuilt from scratch identically.
    let (fresh, _vcs) = {
        let inner =
            next::core::storage::TomlStore::open(root.clone(), root.join("state.toml")).unwrap();
        let vcs = next::core::storage::GitBackend::open(&root).unwrap();
        let head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
        (
            next::core::storage::CachedStore::open(inner, root.join(".next-fresh.db"), &head)
                .unwrap(),
            vcs,
        )
    };
    assert_eq!(
        fresh.list_tasks().unwrap().len() as u64 + fresh
            .query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() })
            .unwrap()
            .total,
        2,
        "fresh rebuild agrees with the live cache"
    );
}
