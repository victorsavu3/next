#![allow(dead_code)]

use std::path::Path;

use next::AppContext;
use next::Config;
use next::TaskRepository;
use tempfile::TempDir;

pub struct TestEnv {
    pub _dir: TempDir,
    pub ctx: AppContext,
}

pub fn setup() -> TestEnv {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    let (store, vcs) = next::core::storage::open(dir.path().to_path_buf()).unwrap();
    let ctx = AppContext {
        config: Config::default(),
        repo: TaskRepository::with_parts(Box::new(store), Box::new(vcs), dir.path().to_path_buf()),
    };
    TestEnv { _dir: dir, ctx }
}

/// Initialise a git repo with test user config at `dir` (used by `setup` and
/// by tests that need a pre-existing repo before running `next init`).
pub fn setup_in(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "test@test.com"]);
    git(dir, &["config", "user.name", "Test"]);
}

/// Runs `git <args>` in `dir`, stripping the repo-scoping variables that
/// `git commit` exports to hook subprocesses (`GIT_DIR`, `GIT_WORK_TREE`, …).
/// Without this, a suite run by a pre-commit hook operates on the enclosing
/// repository instead of the test's temp dir.
pub fn git(dir: &Path, args: &[&str]) {
    let mut cmd = std::process::Command::new("git");
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
}

fn init_git_repo(dir: &Path) {
    setup_in(dir);
}
