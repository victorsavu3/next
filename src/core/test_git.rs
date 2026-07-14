//! Test-only `git` subprocess helpers.
//!
//! `git commit` exports repo-scoping variables (`GIT_DIR`, `GIT_WORK_TREE`,
//! `GIT_INDEX_FILE`, …) to hook subprocesses. A test that spawns `git` while
//! the suite runs under a pre-commit hook would otherwise operate on the
//! *enclosing* repository instead of its own temp dir — concurrent tests then
//! collide on the real repo's `.git/config` lock.

use std::path::Path;
use std::process::Command;

/// Environment variables that scope a `git` invocation to a repository.
const GIT_SCOPE_VARS: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
];

/// Runs `git <args>` in `dir` with the hook environment stripped, asserting success.
pub fn git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    for var in GIT_SCOPE_VARS {
        cmd.env_remove(var);
    }
    let status = cmd.status().expect("failed to spawn git");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// `git init` plus a test identity — the standard test-repo bootstrap.
pub fn init_test_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "test@test.com"]);
    git(dir, &["config", "user.name", "Test"]);
}
