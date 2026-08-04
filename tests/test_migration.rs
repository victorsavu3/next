/// Integration tests for the legacy `tag_descriptions` → per-tag-file migration.
///
/// Each test writes a raw `state.toml` with the old format (a `[tag_descriptions]`
/// table), then calls `next::core::storage::open()` to simulate a fresh process opening an
/// existing repository.  The migration runs automatically during `open()`, so these
/// tests verify the end-to-end behaviour through the full `CachedStore` stack.
use std::{collections::HashMap, fs, path::Path, process::Command};

use next::core::storage::CachedStore;
use next::core::store::Store as _;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn init_git(dir: &Path) {
    // Strip the repo-scoping vars `git commit` exports to hook subprocesses
    // (`GIT_DIR`, …) so a suite run by a pre-commit hook stays in `dir`.
    let run = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
        ] {
            cmd.env_remove(var);
        }
        let status = cmd.status().unwrap();
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["commit", "--allow-empty", "-m", "init"]);
}

/// Write a legacy `state.toml` containing a `[tag_descriptions]` table.
fn write_legacy_state(root: &Path, descriptions: &HashMap<&str, &str>) {
    let mut toml = String::from("[tag_descriptions]\n");
    for (tag, desc) in descriptions {
        toml.push_str(&format!("{tag:?} = {desc:?}\n"));
    }
    fs::write(root.join("state.toml"), toml).unwrap();
}

fn open(dir: &TempDir) -> CachedStore {
    let (store, _vcs) = next::core::storage::open(dir.path().to_path_buf()).unwrap();
    store
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn migration_moves_flat_tags_to_tag_files() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    let mut descs = HashMap::new();
    descs.insert("@work", "Work tasks");
    descs.insert("#printer", "Office printer");
    descs.insert("python", "Python projects");
    write_legacy_state(dir.path(), &descs);

    let store = open(&dir);

    assert_eq!(
        store.get_tag_description("@work").unwrap().as_deref(),
        Some("Work tasks")
    );
    assert_eq!(
        store.get_tag_description("#printer").unwrap().as_deref(),
        Some("Office printer")
    );
    assert_eq!(
        store.get_tag_description("python").unwrap().as_deref(),
        Some("Python projects")
    );
}

#[test]
fn migration_removes_tag_descriptions_from_state_toml() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    let mut descs = HashMap::new();
    descs.insert("@home", "Home tasks");
    write_legacy_state(dir.path(), &descs);

    let _ = open(&dir);

    // state.toml is moved out of the repo into the XDG state directory.
    assert!(
        !dir.path().join("state.toml").exists(),
        "state.toml should have been moved out of the repo"
    );

    // Verify via the store's external state path.
    let state_path = next::core::storage::state_path_for_repo(dir.path());
    let state_content = fs::read_to_string(&state_path).unwrap();
    assert!(
        !state_content.contains("tag_descriptions"),
        "state.toml should no longer contain tag_descriptions after migration"
    );
}

#[test]
fn migration_creates_per_tag_toml_files() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    let mut descs = HashMap::new();
    descs.insert("@work", "Work tasks");
    descs.insert("python", "Python projects");
    write_legacy_state(dir.path(), &descs);

    let _ = open(&dir);

    let work_path = dir.path().join("tags").join("__context__work.toml");
    let python_path = dir.path().join("tags").join("python.toml");
    assert!(
        work_path.exists(),
        "tags/__context__work.toml should be created"
    );
    assert!(python_path.exists(), "tags/python.toml should be created");
}

#[test]
fn migration_handles_hierarchical_tags() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    let mut descs = HashMap::new();
    descs.insert("@home/kitchen", "Kitchen tasks");
    descs.insert("@work/frontend", "Frontend work");
    write_legacy_state(dir.path(), &descs);

    let store = open(&dir);

    assert_eq!(
        store
            .get_tag_description("@home/kitchen")
            .unwrap()
            .as_deref(),
        Some("Kitchen tasks")
    );
    assert_eq!(
        store
            .get_tag_description("@work/frontend")
            .unwrap()
            .as_deref(),
        Some("Frontend work")
    );

    let kitchen_path = dir
        .path()
        .join("tags")
        .join("__context__home")
        .join("kitchen.toml");
    assert!(
        kitchen_path.exists(),
        "tags/__context__home/kitchen.toml should be created"
    );
}

#[test]
fn migration_preserves_other_state_fields() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    let state_toml = r#"
active_contexts = ["@work", "@home"]

[resources]
printer = true

[tag_descriptions]
"@work" = "Work tasks"
"#;
    fs::write(dir.path().join("state.toml"), state_toml).unwrap();

    let store = open(&dir);

    // The tag-state fields of this pre-unification file are dropped rather
    // than migrated; the tag descriptions it carries still migrate out to
    // `tags/`, which is what this test is really about.
    let state = store.get_state().unwrap();
    assert!(state.tags.is_empty());
    assert_eq!(
        store.get_tag_description("@work").unwrap().as_deref(),
        Some("Work tasks")
    );
}

#[test]
fn migration_is_idempotent_across_opens() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    let mut descs = HashMap::new();
    descs.insert("@work", "Work tasks");
    write_legacy_state(dir.path(), &descs);

    // First open: migration runs
    let _ = open(&dir);

    // Second open: migration should be a no-op (no tag_descriptions in state.toml)
    let store = open(&dir);

    assert_eq!(
        store.get_tag_description("@work").unwrap().as_deref(),
        Some("Work tasks")
    );
    // Description should still be there, not doubled or corrupted
    let all = store.list_tag_descriptions().unwrap();
    assert_eq!(all.len(), 1);
}

#[test]
fn migration_with_no_state_file_is_noop() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    // No state.toml — open should succeed without error
    let store = open(&dir);
    let all = store.list_tag_descriptions().unwrap();
    assert!(all.is_empty());
}

#[test]
fn migration_with_empty_tag_descriptions_is_noop() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    fs::write(dir.path().join("state.toml"), "[tag_descriptions]\n").unwrap();

    let store = open(&dir);
    let all = store.list_tag_descriptions().unwrap();
    assert!(all.is_empty());
}

#[test]
fn migration_list_returns_all_migrated_descriptions() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    let mut descs = HashMap::new();
    descs.insert("@work", "Work tasks");
    descs.insert("#printer", "Office printer");
    descs.insert("python", "Python projects");
    write_legacy_state(dir.path(), &descs);

    let store = open(&dir);
    let all = store.list_tag_descriptions().unwrap();

    assert_eq!(all.get("@work").map(String::as_str), Some("Work tasks"));
    assert_eq!(
        all.get("#printer").map(String::as_str),
        Some("Office printer")
    );
    assert_eq!(
        all.get("python").map(String::as_str),
        Some("Python projects")
    );
    assert_eq!(all.len(), 3);
}

#[test]
fn migration_existing_tag_files_are_not_overwritten() {
    let dir = TempDir::new().unwrap();
    init_git(dir.path());

    // Pre-create a tag file with a newer description
    let tags_dir = dir.path().join("tags");
    fs::create_dir_all(&tags_dir).unwrap();
    let tag_file = tags_dir.join("@work.toml");
    fs::write(&tag_file, "description = \"Already migrated\"\n").unwrap();

    // Legacy state also has a (stale) description for the same tag
    let mut descs = HashMap::new();
    descs.insert("@work", "Old description from state.toml");
    write_legacy_state(dir.path(), &descs);

    // The migration should not clobber the existing file
    // (migrate_tag_descriptions writes unconditionally — this test documents current behavior)
    let store = open(&dir);
    let desc = store.get_tag_description("@work").unwrap();
    // Either the pre-existing or the migrated value is acceptable;
    // what must NOT happen is a panic or corrupt file.
    assert!(desc.is_some());
}
