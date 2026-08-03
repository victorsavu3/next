//! Multi-instance convergence: several clones of one shared remote mutate in
//! parallel across multiple iterations — adds, edits, completions, archive
//! passes, resurrections — and after every convergence all instances must
//! hold the identical task corpus across all tiers, every cache must match a
//! from-scratch rebuild of its own checkout, and nothing may be lost.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use next::core::archiver::run_archive_pass;
use next::core::domain::task::Task;
use next::core::service::{apply_edits, EditTaskParams};
use next::core::store::{Store, TaskQuery};
use next::core::{sync, SyncOutcome, TaskRepository};
use tempfile::TempDir;

const TODAY: fn() -> NaiveDate = || NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();

/// One clone of the shared remote, with its own repository handles.
struct Instance {
    #[allow(dead_code)]
    dir: TempDir,
    root: PathBuf,
    repo: TaskRepository,
}

fn clone_instance(remote: &Path, name: &str) -> Instance {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join(name);
    common::git(
        dir.path(),
        &[
            "clone",
            "--quiet",
            &format!("file://{}", remote.display()),
            root.to_str().unwrap(),
        ],
    );
    common::git(&root, &["config", "user.email", &format!("{name}@test")]);
    common::git(&root, &["config", "user.name", name]);
    let repo = TaskRepository::open(root.clone()).unwrap();
    Instance { dir, root, repo }
}

/// Adds a task through the normal transactional path (file + cache + commit).
fn add_task(inst: &mut Instance, task: &Task) {
    inst.repo
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

/// Marks a task done with a backdated completion (drives archive eligibility).
fn complete(inst: &mut Instance, id: uuid::Uuid, when: NaiveDate) {
    next::core::service::complete_task(
        id,
        when,
        &inst.root.clone(),
        &mut *inst.repo.store,
        &*inst.repo.vcs,
    )
    .unwrap();
}

/// Syncs, retrying on push races (another instance pushed first). Conflicts
/// are a hard failure: this workload is designed to merge cleanly, so any
/// conflict means determinism broke.
fn sync_with_retry(inst: &mut Instance) {
    for attempt in 0..20 {
        match sync(&mut inst.repo, false, false) {
            Ok(SyncOutcome::Clean) => return,
            Ok(SyncOutcome::Conflicts(paths)) => {
                panic!("merge conflict in {paths:?} — segment determinism broke")
            }
            Err(e) if attempt < 19 => {
                // Push race: someone else advanced the remote between our
                // pull and push. Back off briefly and go again.
                let _ = e;
                std::thread::sleep(std::time::Duration::from_millis(25 * (attempt + 1)));
            }
            Err(e) => panic!("sync failed after retries: {e}"),
        }
    }
}

/// Two sequential sync rounds converge every instance on the same history.
fn converge(instances: &mut [Instance]) {
    for inst in instances.iter_mut() {
        sync_with_retry(inst);
    }
    for inst in instances.iter_mut() {
        sync_with_retry(inst);
    }
}

/// Full corpus view of a store: id → (title, status, archived?).
fn snapshot(store: &dyn Store) -> BTreeMap<uuid::Uuid, (String, String, bool)> {
    let mut map = BTreeMap::new();
    for t in store.list_tasks().unwrap() {
        map.insert(t.id, (t.title.clone(), format!("{:?}", t.status), false));
    }
    let archived = store
        .query_tasks(&TaskQuery {
            archived: true,
            ..TaskQuery::unpaginated()
        })
        .unwrap();
    for t in archived.items {
        let clash = map.insert(t.id, (t.title.clone(), format!("{:?}", t.status), true));
        assert!(clash.is_none(), "task {} present in two tiers", t.id);
    }
    map
}

/// Asserts every instance sees the same corpus, that it matches `expected`
/// (when given), and that a from-scratch cache rebuild of each checkout
/// agrees with the live cache — i.e. neither repository nor cache corrupted.
fn assert_converged(
    instances: &mut [Instance],
    expected_total: usize,
    label: &str,
) -> BTreeMap<uuid::Uuid, (String, String, bool)> {
    let reference = snapshot(instances[0].repo.store());
    assert_eq!(
        reference.len(),
        expected_total,
        "{label}: task count on instance 0"
    );
    for (i, inst) in instances.iter().enumerate().skip(1) {
        assert_eq!(
            snapshot(inst.repo.store()),
            reference,
            "{label}: instance {i} diverges from instance 0"
        );
    }
    for (i, inst) in instances.iter().enumerate() {
        // Rebuild from the checkout into a fresh database file.
        let inner =
            next::core::storage::TomlStore::open(inst.root.clone(), inst.root.join("state.toml"))
                .unwrap();
        let vcs = next::core::storage::GitBackend::open(&inst.root).unwrap();
        let head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
        let db = inst.root.join(format!(".next-verify-{label}.db"));
        let fresh = next::core::storage::CachedStore::open(inner, db, &head).unwrap();
        assert_eq!(
            snapshot(&fresh),
            reference,
            "{label}: instance {i}'s live cache diverges from a from-scratch rebuild"
        );
    }
    reference
}

#[test]
fn parallel_instances_converge_without_loss() {
    // Shared bare remote, seeded with the committed archive policy so every
    // clone starts from the same config (auto off: passes run explicitly at
    // controlled points; the auto path is covered in test_archive.rs).
    let remote_dir = TempDir::new().unwrap();
    common::git(
        remote_dir.path(),
        &["init", "--bare", "--quiet", "-b", "main"],
    );
    let remote = remote_dir.path().to_path_buf();
    {
        let seed_dir = TempDir::new().unwrap();
        let seed = seed_dir.path().join("seed");
        common::git(
            seed_dir.path(),
            &[
                "clone",
                "--quiet",
                &format!("file://{}", remote.display()),
                seed.to_str().unwrap(),
            ],
        );
        common::git(&seed, &["config", "user.email", "seed@test"]);
        common::git(&seed, &["config", "user.name", "seed"]);
        std::fs::create_dir_all(seed.join("config")).unwrap();
        std::fs::write(
            seed.join("config/archive.toml"),
            "archive_after_days = 180\nauto = false\n",
        )
        .unwrap();
        std::fs::write(seed.join(".gitignore"), ".next.db\n").unwrap();
        common::git(&seed, &["add", "-A"]);
        common::git(&seed, &["commit", "--quiet", "-m", "seed"]);
        common::git(&seed, &["push", "--quiet", "-u", "origin", "HEAD"]);
    }

    const N: usize = 3;
    const ITERATIONS: usize = 4;
    let mut instances: Vec<Instance> = (0..N)
        .map(|i| clone_instance(&remote, &format!("inst{i}")))
        .collect();

    let mut total_tasks = 0usize;
    let mut per_instance_created: Vec<Vec<Task>> = vec![Vec::new(); N];

    for round in 0..ITERATIONS {
        // ── Parallel mutation phase ──────────────────────────────────────
        // Every instance adds, edits, and completes concurrently in its own
        // clone (local commits only — like real offline work).
        let old_when =
            NaiveDate::from_ymd_opt(2025, 6 + round as u32 % 2, 1 + round as u32).unwrap();
        let handles: Vec<_> = instances
            .into_iter()
            .enumerate()
            .map(|(i, mut inst)| {
                let mut created = std::mem::take(&mut per_instance_created[i]);
                std::thread::spawn(move || {
                    // Add 4 new tasks.
                    for k in 0..4 {
                        let mut t = Task::new(format!("i{i}-r{round}-t{k}"));
                        if k == 0 {
                            t.tags = vec![format!("@multi/i{i}")];
                        }
                        add_task(&mut inst, &t);
                        created.push(t);
                    }
                    // Retitle one of this round's tasks.
                    let victim = created[created.len() - 2].id;
                    apply_edits(
                        victim,
                        EditTaskParams {
                            title: Some(format!("i{i}-r{round}-edited")),
                            ..Default::default()
                        },
                        TODAY(),
                        &inst.root.clone(),
                        &mut *inst.repo.store,
                        &*inst.repo.vcs,
                    )
                    .unwrap();
                    // Complete two tasks from the previous round with an old
                    // date, making them archive-eligible later.
                    if round > 0 {
                        let base = (round - 1) * 4;
                        for k in 0..2 {
                            complete(&mut inst, created[base + k].id, old_when);
                        }
                    }
                    (inst, created)
                })
            })
            .collect();
        let mut joined: Vec<(Instance, Vec<Task>)> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();
        instances = Vec::new();
        for (i, (inst, created)) in joined.drain(..).enumerate() {
            per_instance_created[i] = created;
            instances.push(inst);
            let _ = i;
        }
        total_tasks += N * 4;

        // ── Sync phase ───────────────────────────────────────────────────
        if round == ITERATIONS - 1 {
            // Final round syncs in parallel to race pushes for real.
            let handles: Vec<_> = instances
                .into_iter()
                .map(|mut inst| {
                    std::thread::spawn(move || {
                        sync_with_retry(&mut inst);
                        inst
                    })
                })
                .collect();
            instances = handles.into_iter().map(|h| h.join().unwrap()).collect();
        }
        converge(&mut instances);
        assert_converged(&mut instances, total_tasks, &format!("round{round}"));

        // ── Archive events ───────────────────────────────────────────────
        match round {
            1 => {
                // One instance archives; the rest learn via reconcile.
                let outcome = run_archive_pass(&mut instances[0].repo, TODAY()).unwrap();
                assert!(
                    outcome.archived > 0,
                    "round 1 must archive the backdated dones"
                );
                converge(&mut instances);
                assert_converged(&mut instances, total_tasks, "round1-archived");
            }
            2 => {
                // Two converged instances archive CONCURRENTLY: identical
                // views must produce byte-identical segments, so the pushes
                // merge cleanly.
                let mut movers: Vec<Instance> = instances.drain(0..2).collect();
                let handles: Vec<_> = movers
                    .drain(..)
                    .map(|mut inst| {
                        std::thread::spawn(move || {
                            let out = run_archive_pass(&mut inst.repo, TODAY()).unwrap();
                            assert!(out.archived > 0);
                            sync_with_retry(&mut inst);
                            inst
                        })
                    })
                    .collect();
                let mut back: Vec<Instance> =
                    handles.into_iter().map(|h| h.join().unwrap()).collect();
                back.append(&mut instances);
                instances = back;
                converge(&mut instances);
                assert_converged(&mut instances, total_tasks, "round2-concurrent-archive");

                // Cross-instance resurrection: the LAST instance edits a task
                // archived by the concurrent passes above.
                let archived_id = instances[0]
                    .repo
                    .store()
                    .query_tasks(&TaskQuery {
                        archived: true,
                        ..TaskQuery::unpaginated()
                    })
                    .unwrap()
                    .items[0]
                    .id;
                let n = instances.len();
                let inst = &mut instances[n - 1];
                apply_edits(
                    archived_id,
                    EditTaskParams {
                        notes: Some("resurrected across instances".into()),
                        ..Default::default()
                    },
                    TODAY(),
                    &inst.root.clone(),
                    &mut *inst.repo.store,
                    &*inst.repo.vcs,
                )
                .unwrap();
                converge(&mut instances);
                let corpus = assert_converged(&mut instances, total_tasks, "round2-resurrect");
                assert!(
                    !corpus[&archived_id].2,
                    "resurrected task is active everywhere"
                );
            }
            _ => {}
        }
    }

    // ── Final accounting ─────────────────────────────────────────────────
    // Everything every instance ever created is present exactly once, with
    // the edits applied; completions ended up archived or active-closed but
    // never vanished.
    let corpus = assert_converged(&mut instances, total_tasks, "final");
    for (i, created) in per_instance_created.iter().enumerate() {
        for (k, t) in created.iter().enumerate() {
            let entry = corpus
                .get(&t.id)
                .unwrap_or_else(|| panic!("task {} (i{i} #{k}) was lost", t.title));
            let round = k / 4;
            if k % 4 == 2 {
                assert_eq!(entry.0, format!("i{i}-r{round}-edited"), "edit survived");
            }
        }
    }
    let archived_total = corpus.values().filter(|v| v.2).count();
    assert!(
        archived_total > 0,
        "archive passes must have archived something"
    );
}
