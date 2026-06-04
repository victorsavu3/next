/// Concurrent-access tests for file locking.
///
/// Each thread opens a fresh `TomlStore` or `GitBackend` instance to simulate
/// a separate process.  `flock(2)` locks are per open-file-description, so
/// two `open()` calls in the same process contend exactly like two separate
/// processes would.
use std::{path::Path, process::Command, sync::Arc};

use next::core::{
    domain::{state::GlobalState, task::Task},
    service::{apply_edits, EditTaskParams},
    store::{Store as _, VcsBackend as _},
};
use next::core::storage::{FileLock, GitBackend, TomlStore};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn init_git(dir: &Path) {
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
}

fn fresh_store(root: &Path) -> TomlStore {
    TomlStore::open(root.to_path_buf(), root.join("state.toml")).unwrap()
}

fn fresh_vcs(root: &Path) -> GitBackend {
    GitBackend::open(root).unwrap()
}

// ---------------------------------------------------------------------------
// Task-file locking tests
// ---------------------------------------------------------------------------

/// N threads each save one unique task concurrently.
/// Without the repo lock, concurrent writes can produce corrupt/truncated TOML
/// files (e.g. two writers both rename a .tmp file to the same destination, or
/// the directory-scan for slug-uniqueness returns stale data).
/// With the lock, all N tasks must be present and valid afterwards.
#[test]
fn concurrent_task_saves_all_persisted() {
    let dir = TempDir::new().unwrap();
    TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap(); // create tasks/

    let root = Arc::new(dir.path().to_path_buf());

    const N: usize = 20;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let root = Arc::clone(&root);
            std::thread::spawn(move || {
                let mut store = fresh_store(&root);
                let task = Task::new(format!("Concurrent task {i}"));
                store.save_task(&task).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let store = fresh_store(dir.path());
    let tasks = store.list_tasks().unwrap();
    assert_eq!(tasks.len(), N, "all {N} tasks must be persisted without corruption");
}

/// Concurrent saves of tasks that all have the same slug prefix must not cause
/// silent data loss: slug-conflict detection and the repo lock together ensure
/// that at most one task can claim a given slug.
#[test]
fn concurrent_unique_slugs_no_conflict() {
    let dir = TempDir::new().unwrap();
    TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

    let root = Arc::new(dir.path().to_path_buf());

    const N: usize = 10;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let root = Arc::clone(&root);
            std::thread::spawn(move || {
                let mut store = fresh_store(&root);
                let mut task = Task::new(format!("Task {i}"));
                task.slug = Some(format!("task-{i}"));
                store.save_task(&task).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let store = fresh_store(dir.path());
    let tasks = store.list_tasks().unwrap();
    assert_eq!(tasks.len(), N);
}

/// The same slug claimed by two concurrent writers: exactly one must win and
/// the other must get a `SlugConflict` error.  Without the lock both writers
/// can pass the uniqueness check before either has committed its file.
#[test]
fn concurrent_slug_conflict_detected() {
    let dir = TempDir::new().unwrap();
    TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

    let root = Arc::new(dir.path().to_path_buf());

    const THREADS: usize = 8;
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let root = Arc::clone(&root);
            std::thread::spawn(move || {
                let mut store = fresh_store(&root);
                let mut task = Task::new("Shared title");
                task.slug = Some("shared-slug".into());
                store.save_task(&task)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let successes = results.iter().filter(|r| r.is_ok()).count();
    let conflicts = results
        .iter()
        .filter(|r| {
            matches!(r, Err(next::core::error::TaskError::SlugConflict(_)))
        })
        .count();

    assert_eq!(successes, 1, "exactly one thread must win the slug");
    assert_eq!(conflicts, THREADS - 1, "all others must get SlugConflict");

    // The winning task is intact on disk.
    let store = fresh_store(dir.path());
    let tasks = store.list_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
}

// ---------------------------------------------------------------------------
// State-file locking tests
// ---------------------------------------------------------------------------

/// N threads concurrently write different states.  Without the exclusive state
/// lock + atomic rename, two writers can interleave their fs::write calls and
/// produce a torn (half-old, half-new) TOML file that fails to parse.
/// With the lock, the final state must always be valid TOML.
#[test]
fn concurrent_state_saves_no_corruption() {
    let dir = TempDir::new().unwrap();
    TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

    let root = Arc::new(dir.path().to_path_buf());

    const N: usize = 30;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let root = Arc::clone(&root);
            std::thread::spawn(move || {
                let mut store = fresh_store(&root);
                let state = GlobalState {
                    active_contexts: vec![format!("@context{i}")],
                    ..Default::default()
                };
                store.save_state(&state).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // state.toml must parse cleanly — the last writer's data.
    let store = fresh_store(dir.path());
    let state = store.get_state().unwrap();
    assert_eq!(state.active_contexts.len(), 1, "state must not be corrupt");
    assert!(
        state.active_contexts[0].starts_with("@context"),
        "unexpected context value: {:?}",
        state.active_contexts[0]
    );
}

/// Concurrent task and state writes must not corrupt either file.
#[test]
fn concurrent_task_and_state_saves_no_corruption() {
    let dir = TempDir::new().unwrap();
    TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

    let root = Arc::new(dir.path().to_path_buf());

    const N: usize = 10;
    let mut handles = Vec::new();

    for i in 0..N {
        let root = Arc::clone(&root);
        handles.push(std::thread::spawn(move || {
            let mut store = fresh_store(&root);
            store.save_task(&Task::new(format!("Task {i}"))).unwrap();
        }));
    }

    for i in 0..N {
        let root = Arc::clone(&root);
        handles.push(std::thread::spawn(move || {
            let mut store = fresh_store(&root);
            let state = GlobalState {
                active_contexts: vec![format!("@ctx{i}")],
                ..Default::default()
            };
            store.save_state(&state).unwrap();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let store = fresh_store(dir.path());
    assert_eq!(store.list_tasks().unwrap().len(), N, "all task files intact");
    store.get_state().unwrap(); // must not error
}

// ---------------------------------------------------------------------------
// Lost-update prevention (transactional read-modify-write)
// ---------------------------------------------------------------------------

/// N concurrent "processes" each add a *distinct* tag to the *same* task.
///
/// Each thread opens its own `CachedStore` + `GitBackend` (as a separate
/// process would) and calls `apply_edits`, which reads the task, appends one
/// tag, writes, and commits.  Without a transaction that holds the repo lock
/// across the whole read-modify-write — and reconciles the cache with the
/// on-disk HEAD before reading — concurrent edits clobber each other and tags
/// are silently lost.  With it, every tag must survive.
#[test]
fn concurrent_tag_edits_do_not_lose_updates() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    // Seed a single task and an initial commit so HEAD exists.
    let task_id = {
        let (mut store, vcs) = next::core::storage::open(dir.path().to_path_buf()).unwrap();
        let task = Task::new("Shared task");
        let id = task.id;
        store.save_task(&task).unwrap();
        let path = next::core::storage::task_path(dir.path(), &task);
        vcs.commit(&[path], "seed task").unwrap();
        id
    };

    let root = Arc::new(dir.path().to_path_buf());
    const N: usize = 12;
    let barrier = Arc::new(std::sync::Barrier::new(N));

    let handles: Vec<_> = (0..N)
        .map(|i| {
            let root = Arc::clone(&root);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let (mut store, vcs) = next::core::storage::open((*root).clone()).unwrap();
                let edits = EditTaskParams {
                    add_tags: vec![format!("tag{i}")],
                    ..Default::default()
                };
                // Release all threads at once to maximise contention.
                barrier.wait();
                apply_edits(
                    task_id,
                    edits,
                    chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                    &root,
                    &mut store,
                    &vcs,
                )
                .unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // Re-open fresh and assert every tag survived.
    let (store, _vcs) = next::core::storage::open(dir.path().to_path_buf()).unwrap();
    let task = store.get_task(task_id).unwrap();
    let mut tags: Vec<String> = task.tags.iter().filter(|t| t.starts_with("tag")).cloned().collect();
    tags.sort();
    let expected: Vec<String> = (0..N).map(|i| format!("tag{i}")).collect();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    assert_eq!(
        tags, expected_sorted,
        "all {N} concurrent tag additions must survive; lost updates indicate a broken transaction"
    );
}

/// N concurrent "processes" each append a distinct active user to the shared
/// machine-local state, using the same read-modify-write transaction the
/// `next user`/`context`/`resource` handlers use: hold the (re-entrant)
/// state lock across `get_state` → modify → `save_state`.  Without the
/// transaction the per-call locks let two writers read the same state and
/// clobber each other; with it, every appended user must survive.
#[test]
fn concurrent_state_edits_do_not_lose_updates() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    // The state lock lives next to the state file; mirror what `TomlStore`
    // derives so the test transaction and `save_state` share one lock.
    let state_path = dir.path().join("state.toml");
    let state_lock_path = next::core::storage::state_lock_path(&state_path);

    let root = Arc::new(dir.path().to_path_buf());
    let state_lock_path = Arc::new(state_lock_path);
    const N: usize = 12;
    let barrier = Arc::new(std::sync::Barrier::new(N));

    let handles: Vec<_> = (0..N)
        .map(|i| {
            let root = Arc::clone(&root);
            let state_lock_path = Arc::clone(&state_lock_path);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut store = TomlStore::open((*root).clone(), root.join("state.toml")).unwrap();
                barrier.wait();
                // Transaction: hold the re-entrant state lock across the whole
                // read-modify-write. get_state / save_state re-acquire it.
                let _lock = FileLock::acquire(&state_lock_path).unwrap();
                let mut state = store.get_state().unwrap();
                state.active_users.push(format!("user{i}"));
                store.save_state(&state).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let store = TomlStore::open(dir.path().to_path_buf(), state_path).unwrap();
    let mut users = store.get_state().unwrap().active_users;
    users.sort();
    let mut expected: Vec<String> = (0..N).map(|i| format!("user{i}")).collect();
    expected.sort();
    assert_eq!(
        users, expected,
        "all {N} concurrent state edits must survive; lost updates indicate a broken state transaction"
    );
}

/// N concurrent "processes" each subscribe one plugin to a distinct task via
/// `registry::watch`, which holds the re-entrant plugins lock across its
/// load → modify → save. Without that lock the concurrent writers would clobber
/// each other; with it, every subscription must survive.
#[test]
fn concurrent_plugin_watches_do_not_lose_updates() {
    use next::core::plugin::registry;

    let dir = TempDir::new().unwrap();
    let root = Arc::new(dir.path().to_path_buf());
    registry::register(&root, "p", vec!["cmd".to_string()]).unwrap();

    const N: usize = 12;
    let barrier = Arc::new(std::sync::Barrier::new(N));
    let ids: Vec<uuid::Uuid> = (0..N).map(|_| uuid::Uuid::new_v4()).collect();

    let handles: Vec<_> = ids
        .iter()
        .map(|id| {
            let root = Arc::clone(&root);
            let barrier = Arc::clone(&barrier);
            let id = *id;
            std::thread::spawn(move || {
                barrier.wait();
                registry::watch(&root, "p", id).unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let reg = registry::load(&root).unwrap();
    let mut watched = reg.plugins[0].tasks.clone();
    watched.sort();
    let mut expected = ids;
    expected.sort();
    assert_eq!(
        watched, expected,
        "all {N} concurrent subscriptions must survive; lost updates indicate a broken plugins lock"
    );
}

// ---------------------------------------------------------------------------
// Repo-lock coordination between TomlStore and GitBackend
// ---------------------------------------------------------------------------

/// Concurrent task saves and git commits use the same `.next.lock`.
/// Without it, two threads writing the git index simultaneously can corrupt
/// the repository.  With the lock they serialize and all commits succeed.
#[test]
fn concurrent_save_and_commit_no_errors() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

    let root = Arc::new(dir.path().to_path_buf());

    const N: usize = 6;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let root = Arc::clone(&root);
            std::thread::spawn(move || {
                // Each thread gets its own store + vcs to simulate a separate process.
                let mut store = fresh_store(&root);
                let vcs = fresh_vcs(&root);

                let task = Task::new(format!("Task {i}"));
                store.save_task(&task).unwrap();

                let task_path = next::core::storage::task_path(&root, &task);
                vcs.commit(&[task_path], &format!("add task {i}")).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // All N tasks must be readable from disk.
    let store = fresh_store(dir.path());
    assert_eq!(store.list_tasks().unwrap().len(), N);
}

/// A git pull and concurrent task saves must not trample each other.
/// The repo lock guarantees that pull completes atomically relative to writes.
#[test]
fn pull_and_task_save_do_not_trample() {
    // Set up a bare remote with one initial commit.
    let remote_dir = TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "--bare", "-q"])
        .current_dir(remote_dir.path())
        .status()
        .unwrap();

    // Working repo A: add a task and push.
    let repo_a = TempDir::new().unwrap();
    init_git(repo_a.path());
    Command::new("git")
        .args(["remote", "add", "origin", remote_dir.path().to_str().unwrap()])
        .current_dir(repo_a.path())
        .status()
        .unwrap();

    let (mut store_a, vcs_a) = next::core::storage::open(repo_a.path().to_path_buf()).unwrap();
    let task_a = Task::new("Remote task");
    store_a.save_task(&task_a).unwrap();
    let path_a = next::core::storage::task_path(repo_a.path(), &task_a);
    vcs_a.commit(&[path_a], "add remote task").unwrap();
    vcs_a.push().unwrap();

    // Working repo B: pull from remote while also saving local tasks concurrently.
    let repo_b = TempDir::new().unwrap();
    init_git(repo_b.path());
    Command::new("git")
        .args(["remote", "add", "origin", remote_dir.path().to_str().unwrap()])
        .current_dir(repo_b.path())
        .status()
        .unwrap();
    next::core::storage::open(repo_b.path().to_path_buf()).unwrap(); // creates tasks/

    let root_b = Arc::new(repo_b.path().to_path_buf());

    // Thread 1: pull.
    let root_b1 = Arc::clone(&root_b);
    let pull_thread = std::thread::spawn(move || {
        let vcs = fresh_vcs(&root_b1);
        vcs.pull().unwrap()
    });

    // Thread 2: save a local task at the same time.
    let root_b2 = Arc::clone(&root_b);
    let save_thread = std::thread::spawn(move || {
        let mut store = fresh_store(&root_b2);
        let task = Task::new("Local task");
        store.save_task(&task).unwrap();
    });

    pull_thread.join().unwrap();
    save_thread.join().unwrap();

    // Both tasks must be readable; state must not be corrupt.
    let store = fresh_store(&root_b);
    let tasks = store.list_tasks().unwrap();
    // We expect at least the local task; the remote task may or may not be
    // visible depending on whether the pull finished first, but there must be
    // no corruption (no parse error, no missing files).
    assert!(!tasks.is_empty(), "tasks must not be lost due to concurrent pull");
}
