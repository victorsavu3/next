mod common;

use std::fs;
use next_cli::cli::commands::init;

fn run(dir: &std::path::Path) -> anyhow::Result<()> {
    init::run(init::Args {}, dir)
}

#[test]
fn init_creates_git_repo() {
    let dir = tempfile::tempdir().unwrap();
    run(dir.path()).unwrap();
    assert!(dir.path().join(".git").exists(), ".git must exist after init");
}

#[test]
fn init_creates_tasks_dir() {
    let dir = tempfile::tempdir().unwrap();
    run(dir.path()).unwrap();
    assert!(dir.path().join("tasks").is_dir(), "tasks/ must exist after init");
}

#[test]
fn init_adds_next_db_to_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    run(dir.path()).unwrap();

    let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(
        content.lines().any(|l| l.trim() == ".next.db"),
        ".gitignore must contain .next.db; got:\n{content}"
    );
}

#[test]
fn init_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    run(dir.path()).unwrap();
    // Running a second time must succeed and not corrupt anything.
    run(dir.path()).unwrap();

    let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    let db_lines: Vec<&str> = content.lines().filter(|l| l.trim() == ".next.db").collect();
    assert_eq!(db_lines.len(), 1, ".next.db must appear exactly once in .gitignore");
}

#[test]
fn init_preserves_existing_gitignore_content() {
    let dir = tempfile::tempdir().unwrap();

    // Create a .gitignore with some pre-existing content.
    let gitignore = dir.path().join(".gitignore");
    fs::write(&gitignore, "target/\n*.log\n").unwrap();

    run(dir.path()).unwrap();

    let content = fs::read_to_string(&gitignore).unwrap();
    assert!(content.contains("target/"), "existing entries must be preserved");
    assert!(content.contains("*.log"), "existing entries must be preserved");
    assert!(content.contains(".next.db"), ".next.db entry must be appended");
}

#[test]
fn init_skips_gitignore_entry_when_already_present() {
    let dir = tempfile::tempdir().unwrap();
    let gitignore = dir.path().join(".gitignore");
    fs::write(&gitignore, ".next.db\n").unwrap();

    run(dir.path()).unwrap();

    let content = fs::read_to_string(&gitignore).unwrap();
    let count = content.lines().filter(|l| l.trim() == ".next.db").count();
    assert_eq!(count, 1, ".next.db must not be duplicated");
}

#[test]
fn init_on_existing_git_repo_succeeds() {
    // Init a bare git repo first, then run `next init` — it must not try to
    // re-initialise git and must still create tasks/ and .gitignore.
    let dir = tempfile::tempdir().unwrap();

    // Bootstrap a real git repo with user config so the commit can succeed.
    common::setup_in(dir.path());

    run(dir.path()).unwrap();

    assert!(dir.path().join("tasks").is_dir());
    let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.contains(".next.db"));
}

#[test]
fn init_then_add_task_works() {
    // End-to-end: init a repo, then use it as a normal task store.
    let dir = tempfile::tempdir().unwrap();
    run(dir.path()).unwrap();

    // Set up git user so commits succeed (init already ran `git init`).
    for args in [
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .status()
            .unwrap();
    }

    let (store, vcs) = next_storage::open(dir.path().to_path_buf()).unwrap();
    let mut ctx = next_cli::AppContext {
        config: next::Config::default(),
        store: Box::new(store),
        vcs: Box::new(vcs),
        repo_root: dir.path().to_path_buf(),
        log: next_cli::log::Logger::new(dir.path()),
    };

    use next_cli::cli::commands::add;
    add::run(
        add::Args {
            title: "First task".into(),
            due: None, start: None, priority: None, slug: None,
            assignee: None, tags: vec![], parent: None, blocked_by: vec![],
            notes: None, stage: None, wait_for: None, recur_schedule: None,
            recur_completion: None, long_term: false, adjust: None, json: false,
        },
        &mut ctx,
    )
    .unwrap();

    assert_eq!(ctx.store.list_tasks().unwrap().len(), 1);
}
