use std::{fs, path::Path, path::PathBuf, process::Command};

use next::{
    core::domain::task::Task,
    core::store::{PullResult, Store, VcsBackend},
};
use next::core::storage::{CachedStore, GitBackend};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    // Strip the repo-scoping vars `git commit` exports to hook subprocesses
    // (`GIT_DIR`, …) so a suite run by a pre-commit hook stays in `dir`.
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_OBJECT_DIRECTORY"] {
        cmd.env_remove(var);
    }
    let status = cmd.status().unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// Initialises a bare repository that acts as the shared remote.
fn bare_remote() -> TempDir {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init", "--bare", "-q", "."]);
    dir
}

/// Initialises a working repository with `origin` pointing to `remote_path`.
/// Returns the TempDir (must stay in scope) and the opened backend pair.
fn local_with_remote(remote_path: &Path) -> (TempDir, CachedStore, GitBackend) {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init", "-q"]);
    run_git(dir.path(), &["config", "user.email", "test@example.com"]);
    run_git(dir.path(), &["config", "user.name", "Test"]);
    run_git(
        dir.path(),
        &["remote", "add", "origin", remote_path.to_str().unwrap()],
    );
    let (store, vcs) = next::core::storage::open(dir.path().to_path_buf()).unwrap();
    (dir, store, vcs)
}

/// Clones `remote_path` into a subdirectory of a fresh TempDir.
/// Returns the base TempDir (must stay in scope), the clone path, and the backend pair.
fn clone_of(remote_path: &Path) -> (TempDir, PathBuf, CachedStore, GitBackend) {
    let base = TempDir::new().unwrap();
    let clone_path = base.path().join("repo");
    run_git(
        base.path(),
        &["clone", "-q", remote_path.to_str().unwrap(), clone_path.to_str().unwrap()],
    );
    run_git(&clone_path, &["config", "user.email", "test@example.com"]);
    run_git(&clone_path, &["config", "user.name", "Test"]);
    let (store, vcs) = next::core::storage::open(clone_path.clone()).unwrap();
    (base, clone_path, store, vcs)
}

/// Saves a task to store and commits via vcs. Returns the task.
fn commit_task(
    store: &mut CachedStore,
    vcs: &GitBackend,
    repo_root: &Path,
    title: &str,
) -> Task {
    let task = Task::new(title);
    store.save_task(&task).unwrap();
    let path = next::core::storage::task_path(repo_root, &task);
    vcs.commit(&[path], &format!("next: add \"{title}\"")).unwrap();
    task
}

/// Opens a fresh store from disk (bypasses any in-memory cache state).
fn fresh_store(root: &Path) -> CachedStore {
    next::core::storage::open(root.to_path_buf()).unwrap().0
}

// ---------------------------------------------------------------------------
// Push tests
// ---------------------------------------------------------------------------

#[test]
fn push_sends_local_commit_to_bare_remote() {
    let remote = bare_remote();
    let (dir, mut store, vcs) = local_with_remote(remote.path());

    commit_task(&mut store, &vcs, dir.path(), "My task");
    vcs.push().unwrap();

    // Verify remote now has at least one commit.
    let remote_repo = git2::Repository::open_bare(remote.path()).unwrap();
    assert!(remote_repo.head().is_ok(), "remote HEAD should exist after push");
}

#[test]
fn push_sends_multiple_commits() {
    let remote = bare_remote();
    let (dir, mut store, vcs) = local_with_remote(remote.path());

    commit_task(&mut store, &vcs, dir.path(), "Task 1");
    commit_task(&mut store, &vcs, dir.path(), "Task 2");
    vcs.push().unwrap();

    // Count commits reachable from remote HEAD.
    let remote_repo = git2::Repository::open_bare(remote.path()).unwrap();
    let head = remote_repo.head().unwrap().peel_to_commit().unwrap();
    let mut count = 0usize;
    let mut walk = remote_repo.revwalk().unwrap();
    walk.push(head.id()).unwrap();
    for _ in walk {
        count += 1;
    }
    assert!(count >= 2, "expected at least 2 commits on remote, got {count}");
}

#[test]
fn push_without_remote_errors() {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init", "-q"]);
    run_git(dir.path(), &["config", "user.email", "test@example.com"]);
    run_git(dir.path(), &["config", "user.name", "Test"]);
    let (mut store, vcs) = next::core::storage::open(dir.path().to_path_buf()).unwrap();

    // Commit a task so HEAD exists; then push should fail because there's no remote.
    commit_task(&mut store, &vcs, dir.path(), "Orphan task");
    let err = vcs.push().unwrap_err();
    assert!(
        err.to_string().contains("origin") || err.to_string().contains("remote"),
        "unexpected error: {err}"
    );
}

// ---------------------------------------------------------------------------
// Pull tests
// ---------------------------------------------------------------------------

#[test]
fn pull_fast_forwards_to_remote_commits() {
    let remote = bare_remote();

    // Machine A commits a task and pushes.
    let (a_dir, mut a_store, a_vcs) = local_with_remote(remote.path());
    commit_task(&mut a_store, &a_vcs, a_dir.path(), "Task from A");
    a_vcs.push().unwrap();

    // Machine B clones (one commit in history).
    let (_b_base, b_path, _, b_vcs) = clone_of(remote.path());
    let b_head_before = b_vcs.head_hash().unwrap();

    // A adds another task and pushes.
    commit_task(&mut a_store, &a_vcs, a_dir.path(), "Task from A v2");
    a_vcs.push().unwrap();

    // B pulls — should fast-forward.
    let result = b_vcs.pull().unwrap();
    assert!(
        matches!(result, PullResult::Clean),
        "expected Clean pull result"
    );

    // B's HEAD should have advanced.
    let b_head_after = b_vcs.head_hash().unwrap();
    assert_ne!(b_head_before, b_head_after);

    // Both tasks should be visible on disk via a fresh store.
    let store_b = fresh_store(&b_path);
    let tasks = store_b.list_tasks().unwrap();
    assert_eq!(tasks.len(), 2, "expected 2 tasks in B after pull");
}

#[test]
fn pull_returns_clean_when_already_up_to_date() {
    let remote = bare_remote();
    let (a_dir, mut a_store, a_vcs) = local_with_remote(remote.path());
    commit_task(&mut a_store, &a_vcs, a_dir.path(), "Initial task");
    a_vcs.push().unwrap();

    let (_b_base, _b_path, _, b_vcs) = clone_of(remote.path());

    // No new commits — pull should be up-to-date.
    let result = b_vcs.pull().unwrap();
    assert!(matches!(result, PullResult::Clean));
}

#[test]
fn pull_initial_checkout_into_fresh_repo() {
    // A fresh repo with no commits can pull from a remote that has commits.
    let remote = bare_remote();

    // A commits and pushes to populate the remote.
    let (a_dir, mut a_store, a_vcs) = local_with_remote(remote.path());
    commit_task(&mut a_store, &a_vcs, a_dir.path(), "Initial task");
    a_vcs.push().unwrap();

    // B is a fresh repo (no commits, unborn branch) with the same remote.
    let (b_dir, _, b_vcs) = local_with_remote(remote.path());
    assert_eq!(b_vcs.head_hash().unwrap(), "unborn");

    let result = b_vcs.pull().unwrap();
    assert!(matches!(result, PullResult::Clean));

    // B should now have the task file on disk.
    let entries: Vec<_> = fs::read_dir(b_dir.path().join("tasks"))
        .unwrap()
        .collect();
    assert_eq!(entries.len(), 1, "expected 1 task file after initial pull");
}

#[test]
fn pull_non_ff_merge_clean() {
    let remote = bare_remote();

    // A commits initial task and pushes.
    let (a_dir, mut a_store, a_vcs) = local_with_remote(remote.path());
    commit_task(&mut a_store, &a_vcs, a_dir.path(), "Task from A");
    a_vcs.push().unwrap();

    // B clones, then adds its own task (diverging history).
    let (_b_base, b_path, mut b_store, b_vcs) = clone_of(remote.path());
    commit_task(&mut b_store, &b_vcs, &b_path, "Task from B");

    // A adds another task and pushes — now A and B have diverged.
    commit_task(&mut a_store, &a_vcs, a_dir.path(), "Task from A v2");
    a_vcs.push().unwrap();

    // B pulls — should merge cleanly (different files = no conflicts).
    let result = b_vcs.pull().unwrap();
    assert!(matches!(result, PullResult::Clean));

    // All three tasks should be present in B.
    let store_b = fresh_store(&b_path);
    let tasks = store_b.list_tasks().unwrap();
    assert_eq!(tasks.len(), 3, "expected 3 tasks after non-FF merge");
}

#[test]
fn pull_reports_conflicts() {
    let remote = bare_remote();

    // A commits initial task with known slug and pushes.
    let (a_dir, mut a_store, a_vcs) = local_with_remote(remote.path());
    let mut task = Task::new("Shared task");
    task.slug = Some("shared".into());
    a_store.save_task(&task).unwrap();
    let task_path_a = next::core::storage::task_path(a_dir.path(), &task);
    a_vcs.commit(std::slice::from_ref(&task_path_a), "next: add \"Shared task\"").unwrap();
    a_vcs.push().unwrap();

    // B clones.
    let (_b_base, b_path, _, b_vcs) = clone_of(remote.path());
    let task_path_b = b_path.join("tasks").join("shared.toml");

    // Both A and B edit the same file with conflicting content.
    fs::write(&task_path_a, "title = \"A version\"\nid = \"00000000-0000-0000-0000-000000000001\"\nstatus = \"open\"\npriority = \"high\"\ncreated_at = \"2026-01-01T00:00:00Z\"\nupdated_at = \"2026-01-01T00:00:00Z\"\n").unwrap();
    a_vcs.commit(&[task_path_a], "next: edit A").unwrap();
    a_vcs.push().unwrap();

    fs::write(&task_path_b, "title = \"B version\"\nid = \"00000000-0000-0000-0000-000000000001\"\nstatus = \"open\"\npriority = \"low\"\ncreated_at = \"2026-01-01T00:00:00Z\"\nupdated_at = \"2026-01-01T00:00:00Z\"\n").unwrap();
    b_vcs.commit(std::slice::from_ref(&task_path_b), "next: edit B").unwrap();

    // B pulls — should report a conflict.
    let result = b_vcs.pull().unwrap();
    assert!(
        matches!(result, PullResult::Conflicts(_)),
        "expected Conflicts, got {result:?}"
    );

    if let PullResult::Conflicts(paths) = result {
        assert!(!paths.is_empty(), "conflict paths should be non-empty");
    }
}

// ---------------------------------------------------------------------------
// Round-trip: push then pull
// ---------------------------------------------------------------------------

#[test]
fn push_then_pull_preserves_all_task_data() {
    let remote = bare_remote();

    // A adds a fully-populated task.
    let (a_dir, mut a_store, a_vcs) = local_with_remote(remote.path());
    let mut task = Task::new("Detailed task");
    task.slug = Some("detailed".into());
    task.description = Some("A multi-line\ndescription".into());
    task.url = Some("https://example.com/ticket".into());
    task.tags = vec!["@work".into(), "#laptop".into()];
    a_store.save_task(&task).unwrap();
    let path = next::core::storage::task_path(a_dir.path(), &task);
    a_vcs.commit(&[path], "next: add \"Detailed task\"").unwrap();
    a_vcs.push().unwrap();

    // B clones and reads the task.
    let (_b_base, b_path, _, _b_vcs) = clone_of(remote.path());
    let store_b = fresh_store(&b_path);
    let tasks = store_b.list_tasks().unwrap();

    assert_eq!(tasks.len(), 1);
    let loaded = &tasks[0];
    assert_eq!(loaded.title, "Detailed task");
    assert_eq!(loaded.slug.as_deref(), Some("detailed"));
    assert_eq!(loaded.description.as_deref(), Some("A multi-line\ndescription"));
    assert_eq!(loaded.url.as_deref(), Some("https://example.com/ticket"));
    assert!(loaded.tags.contains(&"@work".to_string()));
}

// ---------------------------------------------------------------------------
// Bidirectional sync
// ---------------------------------------------------------------------------

#[test]
fn bidirectional_sync_both_machines_converge() {
    let remote = bare_remote();

    // A adds initial task and pushes.
    let (a_dir, mut a_store, a_vcs) = local_with_remote(remote.path());
    commit_task(&mut a_store, &a_vcs, a_dir.path(), "Task A initial");
    a_vcs.push().unwrap();

    // B clones and adds its own task (diverging).
    let (_b_base, b_path, mut b_store, b_vcs) = clone_of(remote.path());
    commit_task(&mut b_store, &b_vcs, &b_path, "Task B");

    // A adds another task and pushes — A and B have diverged.
    commit_task(&mut a_store, &a_vcs, a_dir.path(), "Task A extra");
    a_vcs.push().unwrap();

    // B syncs: pull (merge A's commit) then push.
    let result = b_vcs.pull().unwrap();
    assert!(matches!(result, PullResult::Clean));
    b_vcs.push().unwrap();

    // Verify: a fresh clone should see all 3 tasks.
    let (_c_base, c_path, _, _) = clone_of(remote.path());
    let store_c = fresh_store(&c_path);
    let tasks = store_c.list_tasks().unwrap();
    assert_eq!(tasks.len(), 3, "remote should have all 3 tasks after B syncs");

    let titles: Vec<&str> = tasks.iter().map(|t| t.title.as_str()).collect();
    assert!(titles.contains(&"Task A initial"));
    assert!(titles.contains(&"Task A extra"));
    assert!(titles.contains(&"Task B"));
}

// ---------------------------------------------------------------------------
// CLI `sync` command: conflicts surface as a typed error (exit code 2)
// ---------------------------------------------------------------------------

/// REQUIREMENTS.md §2.2: `next sync` must exit with code 2 on merge
/// conflicts. The CLI command's `run` signals this by returning a
/// `ConflictsError` (which `main` maps to `std::process::exit(2)`); this
/// test asserts that seam.
#[test]
fn sync_command_returns_conflicts_error() {
    use next::cli::commands::sync as sync_cmd;
    use next::{AppContext, Config, TaskRepository};

    let remote = bare_remote();

    // A commits initial task with known slug and pushes.
    let (a_dir, mut a_store, a_vcs) = local_with_remote(remote.path());
    let mut task = Task::new("Shared task");
    task.slug = Some("shared".into());
    a_store.save_task(&task).unwrap();
    let task_path_a = next::core::storage::task_path(a_dir.path(), &task);
    a_vcs.commit(std::slice::from_ref(&task_path_a), "next: add \"Shared task\"").unwrap();
    a_vcs.push().unwrap();

    // B clones; both sides edit the same file with conflicting content.
    let (_b_base, b_path, b_store, b_vcs) = clone_of(remote.path());
    let task_path_b = b_path.join("tasks").join("shared.toml");

    fs::write(&task_path_a, "title = \"A version\"\nid = \"00000000-0000-0000-0000-000000000001\"\nstatus = \"open\"\npriority = \"high\"\ncreated_at = \"2026-01-01T00:00:00Z\"\nupdated_at = \"2026-01-01T00:00:00Z\"\n").unwrap();
    a_vcs.commit(&[task_path_a], "next: edit A").unwrap();
    a_vcs.push().unwrap();

    fs::write(&task_path_b, "title = \"B version\"\nid = \"00000000-0000-0000-0000-000000000001\"\nstatus = \"open\"\npriority = \"low\"\ncreated_at = \"2026-01-01T00:00:00Z\"\nupdated_at = \"2026-01-01T00:00:00Z\"\n").unwrap();
    b_vcs.commit(std::slice::from_ref(&task_path_b), "next: edit B").unwrap();

    // Run the CLI sync command on B — the pull conflicts.
    let mut ctx = AppContext {
        config: Config::default(),
        repo: TaskRepository::with_parts(Box::new(b_store), Box::new(b_vcs), b_path.clone()),
    };
    let args = sync_cmd::Args { push_only: false, pull_only: false, quiet: false };
    let err = sync_cmd::run(args, &mut ctx).expect_err("sync over conflicting histories must fail");

    let conflicts = err
        .downcast_ref::<sync_cmd::ConflictsError>()
        .expect("error must downcast to ConflictsError so main can exit(2)");
    assert!(!conflicts.0.is_empty(), "conflict paths should be non-empty");
    assert!(
        conflicts.to_string().contains("shared.toml"),
        "message should name the conflicting file: {conflicts}"
    );
}
