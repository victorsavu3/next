#![allow(dead_code)]

use std::path::Path;

use next::Config;
use next::AppContext;
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
    let ctx = AppContext { config: Config::default(), repo: TaskRepository::with_parts(Box::new(store), Box::new(vcs), dir.path().to_path_buf()) };
    TestEnv { _dir: dir, ctx }
}

/// Initialise a git repo with test user config at `dir` (used by `setup` and
/// by tests that need a pre-existing repo before running `next init`).
pub fn setup_in(dir: &Path) {
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test"],
    ] {
        let status = std::process::Command::new("git")
            .args(&args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }
}

fn init_git_repo(dir: &Path) {
    setup_in(dir);
}
