/// Integration tests verifying that the SQLite cache stays in sync with the
/// TOML files and that changes arriving via simulated git pull are picked up
/// correctly on the next store open.
///
/// # Test structure
///
/// Each test uses real TOML files and a real SQLite DB in a temp directory
/// with a real git repo.  Pull scenarios are simulated by writing TOML files
/// directly to the working tree and committing them with `GitBackend::commit`,
/// which advances HEAD exactly as a fast-forward pull would.
use std::{collections::HashSet, fs, path::Path};

use next::core::storage::{CachedStore, GitBackend, TomlStore};
use next::core::{
    domain::{
        state::{GlobalState, TagState},
        task::{Priority, Status, Task},
    },
    store::{Store, VcsBackend},
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn init_git(dir: &Path) {
    let repo = git2::Repository::init(dir).unwrap();
    let mut cfg = repo.config().unwrap();
    cfg.set_str("user.name", "Test").unwrap();
    cfg.set_str("user.email", "test@test.com").unwrap();
}

/// Open a fresh `(CachedStore, GitBackend)` pair rooted at `dir`.
fn open(dir: &Path) -> (CachedStore, GitBackend) {
    let inner = TomlStore::open(dir.to_path_buf(), dir.join("state.toml")).unwrap();
    let vcs = GitBackend::open(dir).unwrap();
    let head = vcs.head_hash().unwrap();
    let db_path = dir.join(".next.db");
    let store = CachedStore::open(inner, db_path, &head).unwrap();
    (store, vcs)
}

/// Open a `CachedStore` using the given explicit `head_hash`.
/// Used in pull-propagation tests to drive whether a rebuild happens.
fn open_with_head(dir: &Path, head_hash: &str) -> CachedStore {
    let inner = TomlStore::open(dir.to_path_buf(), dir.join("state.toml")).unwrap();
    let db_path = dir.join(".next.db");
    CachedStore::open(inner, db_path, head_hash).unwrap()
}

/// Write a single task directly to the TOML file system, bypassing the store.
/// This simulates a file that arrived via `git pull` without going through the
/// write-through path.
fn write_toml_task(dir: &Path, task: &Task) {
    let tasks_dir = dir.join("tasks");
    fs::create_dir_all(&tasks_dir).unwrap();
    let filename = if let Some(ref slug) = task.slug {
        format!("{slug}.toml")
    } else {
        let title_slug: String = task
            .title
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let hex = task.id.to_string().replace('-', "");
        format!("{title_slug}-{}.toml", &hex[..8])
    };
    let content = toml::to_string_pretty(task).unwrap();
    fs::write(tasks_dir.join(filename), content).unwrap();
}

/// Collect task IDs from a store as a `HashSet` for set comparisons.
fn task_id_set(store: &impl Store) -> HashSet<String> {
    store
        .list_tasks()
        .unwrap()
        .into_iter()
        .map(|t| t.id.to_string())
        .collect()
}

/// Assert that the SQLite cache is byte-for-byte consistent with the TOML
/// files: same task IDs, same titles, same state.
///
/// Pass the current git HEAD hash so we can detect whether a rebuild is
/// required (different hash → rebuild then compare; same hash → cache is
/// used as-is and compared against a fresh TomlStore read).
fn assert_sync(dir: &Path, head_hash: &str) {
    let toml_store = TomlStore::open(dir.to_path_buf(), dir.join("state.toml")).unwrap();
    let mut toml_tasks = toml_store.list_tasks().unwrap();
    let toml_state = toml_store.get_state().unwrap();

    let cached = open_with_head(dir, head_hash);
    let mut cached_tasks = cached.list_tasks().unwrap();
    let cached_state = cached.get_state().unwrap();

    toml_tasks.sort_by_key(|t| t.id);
    cached_tasks.sort_by_key(|t| t.id);

    assert_eq!(
        toml_tasks.len(),
        cached_tasks.len(),
        "task count mismatch: TOML={}, SQLite={}",
        toml_tasks.len(),
        cached_tasks.len()
    );

    for (t, c) in toml_tasks.iter().zip(cached_tasks.iter()) {
        assert_eq!(t.id, c.id, "task ID mismatch");
        assert_eq!(t.title, c.title, "title mismatch for task {}", t.id);
        assert_eq!(t.status, c.status, "status mismatch for task {}", t.id);
        assert_eq!(
            t.priority, c.priority,
            "priority mismatch for task {}",
            t.id
        );
        assert_eq!(t.slug, c.slug, "slug mismatch for task {}", t.id);
        assert_eq!(t.tags, c.tags, "tags mismatch for task {}", t.id);
    }

    assert_eq!(toml_state.tags, cached_state.tags, "tag state mismatch");
    assert_eq!(
        toml_state.active_users, cached_state.active_users,
        "active_users mismatch"
    );
}

// ---------------------------------------------------------------------------
// Write-through synchronisation tests
// ---------------------------------------------------------------------------

#[test]
fn save_task_writes_to_both_stores() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let task = Task::new("Write report");
    store.save_task(&task).unwrap();

    let head = vcs.head_hash().unwrap();
    assert_sync(dir.path(), &head);

    // Verify via independent TomlStore read.
    let toml = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
    assert!(toml.get_task(task.id).is_ok());

    // Verify SQLite path directly.
    let loaded = store.get_task(task.id).unwrap();
    assert_eq!(loaded.title, "Write report");
}

#[test]
fn delete_task_removes_from_both_stores() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let task = Task::new("Temporary");
    store.save_task(&task).unwrap();
    store.delete_task(task.id).unwrap();

    let head = vcs.head_hash().unwrap();
    assert_sync(dir.path(), &head);

    // TOML file must be gone.
    let path = next::core::storage::task_path(dir.path(), &task);
    assert!(!path.exists(), "TOML file still present after delete");

    // SQLite must also not have it.
    let err = store.get_task(task.id).unwrap_err();
    assert!(matches!(err, next::core::error::TaskError::TaskNotFound(_)));
}

#[test]
fn update_task_syncs_to_both_stores() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let mut task = Task::new("Old title");
    task.slug = Some("my-task".into());
    store.save_task(&task).unwrap();

    task.title = "New title".into();
    task.priority = Priority::High;
    store.save_task(&task).unwrap();

    let head = vcs.head_hash().unwrap();
    assert_sync(dir.path(), &head);

    // Both storage layers see the updated values.
    let from_cache = store.get_task_by_slug("my-task").unwrap().unwrap();
    assert_eq!(from_cache.title, "New title");
    assert_eq!(from_cache.priority, Priority::High);

    let toml = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
    let from_toml = toml.get_task_by_slug("my-task").unwrap().unwrap();
    assert_eq!(from_toml.title, "New title");
    assert_eq!(from_toml.priority, Priority::High);
}

#[test]
fn done_status_syncs_to_both_stores() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let mut task = Task::new("Finish this");
    store.save_task(&task).unwrap();
    task.mark_done(chrono::Local::now().date_naive());
    store.save_task(&task).unwrap();

    let head = vcs.head_hash().unwrap();
    assert_sync(dir.path(), &head);

    let from_toml = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml"))
        .unwrap()
        .get_task(task.id)
        .unwrap();
    assert_eq!(from_toml.status, Status::Done);
    assert_eq!(store.get_task(task.id).unwrap().status, Status::Done);
}

#[test]
fn save_state_syncs_to_both_stores() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let mut state = GlobalState {
        active_users: vec!["alice".into()],
        ..Default::default()
    };
    state.set_state("@work", Some(TagState::Required));
    state.set_state("#printer", Some(TagState::Excluded));
    store.save_state(&state).unwrap();

    let head = vcs.head_hash().unwrap();
    assert_sync(dir.path(), &head);

    let from_toml = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml"))
        .unwrap()
        .get_state()
        .unwrap();
    assert_eq!(from_toml.tags, state.tags);
    assert_eq!(from_toml.active_users, state.active_users);

    let from_cache = store.get_state().unwrap();
    assert_eq!(from_cache.tags, state.tags);
    assert_eq!(from_cache.active_users, state.active_users);
}

#[test]
fn multiple_writes_stay_in_sync() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    // Add five tasks.
    let tasks: Vec<Task> = (1..=5).map(|i| Task::new(format!("Task {i}"))).collect();
    for t in &tasks {
        store.save_task(t).unwrap();
    }

    // Delete every other one (indices 0, 2, 4 → 3 deleted, 2 remain).
    for t in tasks.iter().step_by(2) {
        store.delete_task(t.id).unwrap();
    }

    let head = vcs.head_hash().unwrap();
    assert_sync(dir.path(), &head);
    assert_eq!(store.list_tasks().unwrap().len(), 2);
}

#[test]
fn slug_change_syncs_lookup_tables() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let mut task = Task::new("Water plants");
    task.slug = Some("old-slug".into());
    store.save_task(&task).unwrap();

    task.slug = Some("new-slug".into());
    store.save_task(&task).unwrap();

    let head = vcs.head_hash().unwrap();
    assert_sync(dir.path(), &head);

    assert!(store.get_task_by_slug("old-slug").unwrap().is_none());
    assert!(store.get_task_by_slug("new-slug").unwrap().is_some());
}

#[test]
fn no_phantom_sqlite_entries_after_deletes() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let t1 = Task::new("Keep me");
    let t2 = Task::new("Delete me");
    store.save_task(&t1).unwrap();
    store.save_task(&t2).unwrap();
    store.delete_task(t2.id).unwrap();

    let head = vcs.head_hash().unwrap();
    let all = store.list_tasks().unwrap();
    assert_eq!(all.len(), 1, "phantom entry in SQLite after delete");
    assert_eq!(all[0].id, t1.id);
    assert_sync(dir.path(), &head);
}

#[test]
fn find_by_prefix_consistent_after_write_through() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let task = Task::new("Prefix check");
    store.save_task(&task).unwrap();

    let prefix = &task.id.to_string()[..8];

    // SQLite path.
    let found = store.find_tasks_by_prefix(prefix).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, task.id);

    // TomlStore path.
    let toml = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
    let found_toml = toml.find_tasks_by_prefix(prefix).unwrap();
    assert_eq!(found_toml.len(), 1);
    assert_eq!(found_toml[0].id, task.id);

    let head = vcs.head_hash().unwrap();
    assert_sync(dir.path(), &head);
}

// ---------------------------------------------------------------------------
// Cache-reuse test (no rebuild when HEAD is unchanged)
// ---------------------------------------------------------------------------

#[test]
fn same_head_reuses_cache_does_not_see_bypass_writes() {
    // Prove the cache is actually used: write a task to TOML directly
    // (bypassing write-through) without advancing HEAD.  A new store opened
    // with the *same* HEAD must NOT see the bypass task, because no rebuild
    // is triggered.
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let known = Task::new("Known task");
    store.save_task(&known).unwrap();

    // Commit the known task so there's a real HEAD.
    let known_path = next::core::storage::task_path(dir.path(), &known);
    vcs.commit(&[known_path], "add known task").unwrap();
    let head = vcs.head_hash().unwrap();

    // Warm the cache to the new HEAD so the stored head_hash in SQLite matches.
    // Without this step the next open would rebuild (stored HEAD would be "unborn").
    drop(open_with_head(dir.path(), &head));

    // Write a bypass task directly to TOML without going through the store.
    // This deliberately bypasses write-through to prove the cache is not rebuilt.
    let mut bypass = Task::new("Bypass task");
    bypass.slug = Some("bypass".into());
    write_toml_task(dir.path(), &bypass);

    // Open with the SAME HEAD → stored HEAD matches → no rebuild.
    let store2 = open_with_head(dir.path(), &head);
    let ids = task_id_set(&store2);

    assert!(
        ids.contains(&known.id.to_string()),
        "known task must be in cache"
    );
    assert!(
        !ids.contains(&bypass.id.to_string()),
        "bypass task must NOT be visible when cache is reused (no rebuild)"
    );
}

// ---------------------------------------------------------------------------
// Git pull propagation tests
// ---------------------------------------------------------------------------

#[test]
fn pull_adds_new_task() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    // Commit an initial task so we have a real HEAD.
    let existing = Task::new("Existing");
    store.save_task(&existing).unwrap();
    let path = next::core::storage::task_path(dir.path(), &existing);
    vcs.commit(&[path], "add existing").unwrap();

    // Simulate pull: a new TOML file appears and is committed.
    let mut pulled = Task::new("Pulled task");
    pulled.slug = Some("pulled".into());
    write_toml_task(dir.path(), &pulled);
    let pulled_path = next::core::storage::task_path(dir.path(), &pulled);
    vcs.commit(&[pulled_path], "add pulled task").unwrap();

    let new_head = vcs.head_hash().unwrap();

    // Open a new store — different HEAD triggers rebuild.
    let store2 = open_with_head(dir.path(), &new_head);
    let ids = task_id_set(&store2);
    assert!(
        ids.contains(&existing.id.to_string()),
        "existing task must survive pull"
    );
    assert!(
        ids.contains(&pulled.id.to_string()),
        "pulled task must appear after rebuild"
    );
    assert_sync(dir.path(), &new_head);
}

#[test]
fn pull_removes_task() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let keep = Task::new("Keep");
    let remove = Task::new("Remove");
    store.save_task(&keep).unwrap();
    store.save_task(&remove).unwrap();

    let keep_path = next::core::storage::task_path(dir.path(), &keep);
    let remove_path = next::core::storage::task_path(dir.path(), &remove);
    vcs.commit(&[keep_path, remove_path], "add both tasks")
        .unwrap();

    // Simulate pull: the remove task is deleted from the working tree and committed.
    fs::remove_file(next::core::storage::task_path(dir.path(), &remove)).unwrap();
    let remove_path2 = next::core::storage::task_path(dir.path(), &remove);
    vcs.commit(&[remove_path2], "remove task").unwrap();

    let new_head = vcs.head_hash().unwrap();
    let store2 = open_with_head(dir.path(), &new_head);
    let ids = task_id_set(&store2);

    assert!(
        ids.contains(&keep.id.to_string()),
        "keep task must still be present"
    );
    assert!(
        !ids.contains(&remove.id.to_string()),
        "removed task must be gone from cache"
    );
    assert_sync(dir.path(), &new_head);
}

#[test]
fn pull_modifies_task_field() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let mut task = Task::new("Original title");
    task.slug = Some("my-task".into());
    store.save_task(&task).unwrap();
    let path = next::core::storage::task_path(dir.path(), &task);
    vcs.commit(std::slice::from_ref(&path), "add task").unwrap();

    // Simulate pull: the TOML file is rewritten with a new title and priority.
    task.title = "Pulled title".into();
    task.priority = Priority::High;
    let updated_toml = toml::to_string_pretty(&task).unwrap();
    fs::write(&path, updated_toml).unwrap();
    vcs.commit(&[path], "update task via pull").unwrap();

    let new_head = vcs.head_hash().unwrap();
    let store2 = open_with_head(dir.path(), &new_head);

    let loaded = store2.get_task_by_slug("my-task").unwrap().unwrap();
    assert_eq!(loaded.title, "Pulled title");
    assert_eq!(loaded.priority, Priority::High);
    assert_sync(dir.path(), &new_head);
}

#[test]
fn pull_adds_multiple_tasks() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    // Commit a seed task.
    let seed = Task::new("Seed");
    store.save_task(&seed).unwrap();
    let seed_path = next::core::storage::task_path(dir.path(), &seed);
    vcs.commit(&[seed_path], "seed").unwrap();

    // Simulate a pull that added 5 tasks.
    let new_tasks: Vec<Task> = (1..=5).map(|i| Task::new(format!("New {i}"))).collect();
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for t in &new_tasks {
        write_toml_task(dir.path(), t);
        paths.push(next::core::storage::task_path(dir.path(), t));
    }
    vcs.commit(&paths, "bulk add via pull").unwrap();

    let new_head = vcs.head_hash().unwrap();
    let store2 = open_with_head(dir.path(), &new_head);
    let all = store2.list_tasks().unwrap();
    assert_eq!(all.len(), 6, "seed + 5 pulled tasks");
    assert_sync(dir.path(), &new_head);
}

#[test]
fn pull_removes_multiple_tasks() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let tasks: Vec<Task> = (1..=6).map(|i| Task::new(format!("Task {i}"))).collect();
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for t in &tasks {
        store.save_task(t).unwrap();
        paths.push(next::core::storage::task_path(dir.path(), t));
    }
    vcs.commit(&paths, "add all").unwrap();

    // Simulate pull: remove the first three.
    let removed = &tasks[..3];
    let kept = &tasks[3..];
    let mut del_paths = Vec::new();
    for t in removed {
        let p = next::core::storage::task_path(dir.path(), t);
        fs::remove_file(&p).unwrap();
        del_paths.push(p);
    }
    vcs.commit(&del_paths, "remove first three").unwrap();

    let new_head = vcs.head_hash().unwrap();
    let store2 = open_with_head(dir.path(), &new_head);
    let ids = task_id_set(&store2);

    for t in removed {
        assert!(
            !ids.contains(&t.id.to_string()),
            "removed task {} must be gone",
            t.id
        );
    }
    for t in kept {
        assert!(
            ids.contains(&t.id.to_string()),
            "kept task {} must survive",
            t.id
        );
    }
    assert_eq!(ids.len(), 3);
    assert_sync(dir.path(), &new_head);
}

#[test]
fn pull_mixed_changes() {
    // Simulate a pull that adds, removes, and modifies tasks simultaneously.
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    let mut existing_a = Task::new("Task A");
    existing_a.slug = Some("task-a".into());
    let mut existing_b = Task::new("Task B");
    existing_b.slug = Some("task-b".into());
    let existing_c = Task::new("Task C");
    store.save_task(&existing_a).unwrap();
    store.save_task(&existing_b).unwrap();
    store.save_task(&existing_c).unwrap();

    let path_a = next::core::storage::task_path(dir.path(), &existing_a);
    let path_b = next::core::storage::task_path(dir.path(), &existing_b);
    let path_c = next::core::storage::task_path(dir.path(), &existing_c);
    vcs.commit(&[path_a.clone(), path_b.clone(), path_c.clone()], "initial")
        .unwrap();

    // Pull: modify A, remove B, add D.
    existing_a.title = "Task A (modified)".into();
    existing_a.priority = Priority::High;
    let updated_toml = toml::to_string_pretty(&existing_a).unwrap();
    fs::write(&path_a, updated_toml).unwrap();

    fs::remove_file(&path_b).unwrap();

    let task_d = Task::new("Task D");
    write_toml_task(dir.path(), &task_d);
    let path_d = next::core::storage::task_path(dir.path(), &task_d);

    vcs.commit(&[path_a, path_b, path_d], "mixed pull").unwrap();

    let new_head = vcs.head_hash().unwrap();
    let store2 = open_with_head(dir.path(), &new_head);

    // A modified.
    let a = store2.get_task_by_slug("task-a").unwrap().unwrap();
    assert_eq!(a.title, "Task A (modified)");
    assert_eq!(a.priority, Priority::High);

    // B removed.
    assert!(store2.get_task_by_slug("task-b").unwrap().is_none());

    // C untouched.
    let ids = task_id_set(&store2);
    assert!(ids.contains(&existing_c.id.to_string()));

    // D added.
    assert!(ids.contains(&task_d.id.to_string()));

    assert_eq!(ids.len(), 3); // A, C, D
    assert_sync(dir.path(), &new_head);
}

#[test]
fn pull_updates_state() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let (mut store, vcs) = open(dir.path());

    // Set initial state.
    let mut initial_state = GlobalState::default();
    initial_state.set_state("@home", Some(TagState::Required));
    store.save_state(&initial_state).unwrap();

    // Commit a task to have a real HEAD.
    let task = Task::new("Anchor");
    store.save_task(&task).unwrap();
    let task_path = next::core::storage::task_path(dir.path(), &task);
    vcs.commit(&[task_path], "anchor commit").unwrap();

    // Simulate pull: state.toml is replaced externally with new tag state.
    let mut new_state = GlobalState {
        active_users: vec!["alice".into()],
        ..Default::default()
    };
    new_state.set_state("@work", Some(TagState::Required));
    new_state.set_state("@office", Some(TagState::Required));
    let state_content = toml::to_string_pretty(&new_state).unwrap();
    let state_path = dir.path().join("state.toml");
    fs::write(&state_path, state_content).unwrap();
    vcs.commit(&[state_path], "update state via pull").unwrap();

    let new_head = vcs.head_hash().unwrap();
    let store2 = open_with_head(dir.path(), &new_head);
    let loaded = store2.get_state().unwrap();

    assert_eq!(loaded.tags, new_state.tags);
    assert_eq!(loaded.active_users, vec!["alice"]);
    assert_sync(dir.path(), &new_head);
}

#[test]
fn pull_with_no_prior_cache() {
    // A completely fresh repo with no prior .next.db receives new tasks via
    // simulated pull and the first open must see them all.
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let vcs = GitBackend::open(dir.path()).unwrap();

    // Directly write three TOML files and commit them (no store involved yet).
    let tasks: Vec<Task> = (1..=3).map(|i| Task::new(format!("Task {i}"))).collect();
    let mut paths = Vec::new();
    for t in &tasks {
        write_toml_task(dir.path(), t);
        paths.push(next::core::storage::task_path(dir.path(), t));
    }
    vcs.commit(&paths, "initial commit from pull").unwrap();

    let head = vcs.head_hash().unwrap();
    let store = open_with_head(dir.path(), &head);
    assert_eq!(store.list_tasks().unwrap().len(), 3);
    assert_sync(dir.path(), &head);
}

#[test]
fn pull_changes_are_queryable_by_slug() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let vcs = GitBackend::open(dir.path()).unwrap();

    let mut task = Task::new("Project X");
    task.slug = Some("project-x".into());
    write_toml_task(dir.path(), &task);
    let path = next::core::storage::task_path(dir.path(), &task);
    vcs.commit(&[path], "add via pull").unwrap();

    let head = vcs.head_hash().unwrap();
    let store = open_with_head(dir.path(), &head);

    let found = store.get_task_by_slug("project-x").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().title, "Project X");
}

#[test]
fn pull_changes_are_queryable_by_prefix() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let vcs = GitBackend::open(dir.path()).unwrap();

    let task = Task::new("Find by prefix");
    write_toml_task(dir.path(), &task);
    let path = next::core::storage::task_path(dir.path(), &task);
    vcs.commit(&[path], "add via pull").unwrap();

    let head = vcs.head_hash().unwrap();
    let store = open_with_head(dir.path(), &head);

    let prefix = &task.id.to_string()[..8];
    let found = store.find_tasks_by_prefix(prefix).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, task.id);
}

#[test]
fn successive_pulls_all_converge() {
    // Three successive "pull" rounds each adding tasks — every round the cache
    // must fully reflect the current TOML state.
    let dir = TempDir::new().unwrap();
    init_git(dir.path());
    let vcs = GitBackend::open(dir.path()).unwrap();

    for round in 1u32..=3 {
        let task = Task::new(format!("Round {round} task"));
        write_toml_task(dir.path(), &task);
        let path = next::core::storage::task_path(dir.path(), &task);
        vcs.commit(&[path], &format!("round {round}")).unwrap();

        let head = vcs.head_hash().unwrap();
        let store = open_with_head(dir.path(), &head);
        let count = store.list_tasks().unwrap().len();
        assert_eq!(
            count, round as usize,
            "after round {round}, expected {round} tasks"
        );
        assert_sync(dir.path(), &head);
    }
}
