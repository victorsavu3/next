/// Concurrent-access tests for file locking.
///
/// Each thread opens a fresh `TomlStore` or `GitBackend` instance to simulate
/// a separate process.  `flock(2)` locks are per open-file-description, so
/// two `open()` calls in the same process contend exactly like two separate
/// processes would.
use std::{path::Path, process::Command, sync::Arc};

use next::{
    domain::{state::GlobalState, task::Task},
    store::{Store as _, VcsBackend as _},
};
use next::storage::{GitBackend, TomlStore};
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
                let task = Task::new(&format!("Concurrent task {i}"));
                store.save_task(&task).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let store = fresh_store(&dir.path().to_path_buf());
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
                let mut task = Task::new(&format!("Task {i}"));
                task.slug = Some(format!("task-{i}"));
                store.save_task(&task).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let store = fresh_store(&dir.path().to_path_buf());
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
            matches!(r, Err(next::error::AppError::SlugConflict(_)))
        })
        .count();

    assert_eq!(successes, 1, "exactly one thread must win the slug");
    assert_eq!(conflicts, THREADS - 1, "all others must get SlugConflict");

    // The winning task is intact on disk.
    let store = fresh_store(&dir.path().to_path_buf());
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
    let store = fresh_store(&dir.path().to_path_buf());
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
            store.save_task(&Task::new(&format!("Task {i}"))).unwrap();
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

    let store = fresh_store(&dir.path().to_path_buf());
    assert_eq!(store.list_tasks().unwrap().len(), N, "all task files intact");
    store.get_state().unwrap(); // must not error
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

                let task = Task::new(&format!("Task {i}"));
                store.save_task(&task).unwrap();

                let task_path = next::storage::task_path(&root, &task);
                vcs.commit(&[task_path], &format!("add task {i}")).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // All N tasks must be readable from disk.
    let store = fresh_store(&dir.path().to_path_buf());
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

    let (mut store_a, vcs_a) = next::storage::open(repo_a.path().to_path_buf()).unwrap();
    let task_a = Task::new("Remote task");
    store_a.save_task(&task_a).unwrap();
    let path_a = next::storage::task_path(repo_a.path(), &task_a);
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
    next::storage::open(repo_b.path().to_path_buf()).unwrap(); // creates tasks/

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
