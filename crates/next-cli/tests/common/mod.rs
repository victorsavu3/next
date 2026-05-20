use std::path::Path;

use next::Config;
use next_cli::{log::Logger, AppContext};
use tempfile::TempDir;

pub struct TestEnv {
    pub _dir: TempDir,
    pub ctx: AppContext,
}

pub fn setup() -> TestEnv {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    let (store, vcs) = next_storage::open(dir.path().to_path_buf()).unwrap();
    let log = Logger::new(dir.path());
    let ctx = AppContext {
        config: Config::default(),
        store: Box::new(store),
        vcs: Box::new(vcs),
        repo_root: dir.path().to_path_buf(),
        log,
    };
    TestEnv { _dir: dir, ctx }
}

fn init_git_repo(dir: &Path) {
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
