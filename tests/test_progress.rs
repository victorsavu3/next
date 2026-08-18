//! Progress rendering, end to end — the only tests that spawn the real `next`
//! binary.
//!
//! They have to: what is under test is a decision about the *process's* stderr
//! (is it a terminal? was it redirected?) and the bytes that land there, and an
//! in-process call cannot observe either. `env!("CARGO_BIN_EXE_next")` is the
//! binary cargo just built for this test run, so there is no PATH lookup and no
//! stale install to trip over.
//!
//! `Command::output()` pipes both streams, which means every run here has a
//! non-terminal stderr — the same situation as `next list 2>log`. That is the
//! default the gate must get right, and `NEXT_FORCE_PROGRESS=1` is how the
//! rendering path is reached anyway (it also skips the 100 ms startup delay, so
//! a fast operation still paints and these tests stay deterministic).

mod common;

use std::path::Path;
use std::process::{Command, Output, Stdio};

use next::core::domain::task::Task;
use tempfile::TempDir;

/// Labels core reports during a cache rebuild. If the renderer runs, these are
/// what it puts on the screen.
const REBUILD_LABELS: [&str; 2] = ["Reading git history", "Indexing tasks"];

/// A seeded repository plus the machine-local state directory the spawned
/// binary is confined to.
struct Fixture {
    env: common::TestEnv,
    state: TempDir,
}

impl Fixture {
    /// A repository with `tasks` committed tasks — enough that the
    /// *Indexing tasks* phase has something to count.
    fn seeded(tasks: usize) -> Self {
        let mut env = common::setup();
        let made: Vec<Task> = (0..tasks)
            .map(|i| {
                let mut t = Task::new(format!("Task {i}"));
                t.slug = Some(format!("task-{i}"));
                t
            })
            .collect();
        // One transaction and one commit: the tasks are the fixture, not the
        // thing under test, and forty commits would dominate the runtime.
        env.ctx
            .repo
            .transaction(|store, vcs, root| {
                let mut paths = Vec::new();
                for task in &made {
                    store.save_task(task)?;
                    paths.push(next::core::storage::task_path(root, task));
                }
                vcs.commit(&paths, "next: seed")?;
                Ok(())
            })
            .unwrap();
        Self {
            env,
            state: TempDir::new().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        &self.env.ctx.repo.repo_root
    }

    /// The binary, pointed at this repository and nothing of the developer's.
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_next"));
        cmd.arg("--repo")
            .arg(self.root())
            // A config path that does not exist resolves to `Config::default()`,
            // so the developer's own config.toml cannot change what these
            // tests see (autopush, in particular).
            .arg("--config")
            .arg(self.root().join("no-such-config.toml"))
            .args(args)
            .env("XDG_STATE_HOME", self.state.path())
            .env_remove("NEXT_FORCE_PROGRESS")
            .stdin(Stdio::null());
        // The repo-scoping variables `git commit` exports to hook subprocesses
        // would point the spawned binary at the enclosing repository.
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
        ] {
            cmd.env_remove(var);
        }
        cmd
    }

    fn run(&self, args: &[&str]) -> Run {
        Run::of(self.cmd(args).output().unwrap())
    }

    fn run_forced(&self, args: &[&str]) -> Run {
        Run::of(
            self.cmd(args)
                .env("NEXT_FORCE_PROGRESS", "1")
                .output()
                .unwrap(),
        )
    }
}

/// One invocation's captured output.
struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Run {
    fn of(out: Output) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn assert_ok(&self) -> &Self {
        assert!(self.success, "command failed; stderr: {}", self.stderr);
        self
    }

    /// No label, and no escape sequence either: a suppressed bar must leave
    /// nothing at all behind, not an invisible cursor movement.
    fn assert_no_progress(&self) -> &Self {
        for label in REBUILD_LABELS {
            assert!(
                !self.stderr.contains(label),
                "stderr carries progress: {:?}",
                self.stderr
            );
        }
        assert!(
            !self.stderr.contains('\x1b'),
            "stderr carries terminal control sequences: {:?}",
            self.stderr
        );
        self
    }

    fn assert_progress(&self) -> &Self {
        for label in REBUILD_LABELS {
            assert!(
                self.stderr.contains(label),
                "expected {label:?} on stderr, got {:?}",
                self.stderr
            );
        }
        self
    }

    /// The results side of the contract: whatever progress did, stdout still
    /// parses.
    fn assert_stdout_json(&self) -> serde_json::Value {
        serde_json::from_str(self.stdout.trim())
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {:?}", self.stdout))
    }
}

#[test]
fn piped_stderr_gets_no_progress() {
    let fx = Fixture::seeded(40);
    let run = fx.run(&["--offline", "maintenance", "rebuild-cache"]);
    run.assert_ok().assert_no_progress();
    assert!(
        run.stdout.contains("Rebuilt the local cache"),
        "the result still goes to stdout: {:?}",
        run.stdout
    );
}

#[test]
fn json_output_gets_no_progress() {
    let fx = Fixture::seeded(40);
    let run = fx.run(&["--offline", "maintenance", "rebuild-cache", "--json"]);
    run.assert_ok().assert_no_progress();
    assert_eq!(run.assert_stdout_json()["rebuilt"], serde_json::json!(true));
}

#[test]
fn forcing_renders_progress_without_touching_stdout() {
    let fx = Fixture::seeded(40);
    let run = fx.run_forced(&["--offline", "maintenance", "rebuild-cache", "--json"]);
    run.assert_ok().assert_progress();
    // The whole point of drawing on stderr: a forced, machine-readable run
    // still emits a clean JSON document.
    let json = run.assert_stdout_json();
    assert_eq!(json["rebuilt"], serde_json::json!(true));
    assert_eq!(json["active_tasks"], serde_json::json!(40));
}

#[test]
fn quiet_and_no_progress_beat_the_force_env() {
    let fx = Fixture::seeded(40);
    for flag in ["--quiet", "--no-progress"] {
        fx.run_forced(&["--offline", flag, "maintenance", "rebuild-cache"])
            .assert_ok()
            .assert_no_progress();
    }
    // -q is the same request spelled shorter.
    fx.run_forced(&["--offline", "-q", "maintenance", "rebuild-cache"])
        .assert_ok()
        .assert_no_progress();
}

#[test]
fn quiet_suppresses_the_autopull_note() {
    let fx = Fixture::seeded(5);
    // No remote is configured, so the staleness pull fails — which is exactly
    // the informational note `--quiet` is meant to silence.
    let loud = fx.run(&["--autopull", "maintenance", "rebuild-cache"]);
    loud.assert_ok();
    assert!(
        loud.stderr.contains("auto-pull failed"),
        "expected the note without --quiet, got {:?}",
        loud.stderr
    );

    let quiet = fx.run(&["--autopull", "--quiet", "maintenance", "rebuild-cache"]);
    quiet.assert_ok();
    assert!(
        !quiet.stderr.contains("auto-pull"),
        "--quiet must silence the note: {:?}",
        quiet.stderr
    );
    assert!(
        quiet.stdout.contains("Rebuilt the local cache"),
        "--quiet must not silence the result: {:?}",
        quiet.stdout
    );
}

#[test]
fn quiet_suppresses_the_sync_confirmation() {
    let fx = Fixture::seeded(5);
    // A bare repository as the remote, so `next sync` can actually complete.
    let remote = TempDir::new().unwrap();
    common::git(remote.path(), &["init", "--bare", "-q", "."]);
    common::git(
        fx.root(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    // Seed the remote with the current branch: fetching from a repository with
    // no refs at all fails inside git2, which would be a fixture problem
    // masquerading as a sync failure.
    common::git(fx.root(), &["push", "-q", "-u", "origin", "HEAD"]);

    let loud = fx.run(&["sync"]);
    loud.assert_ok();
    assert!(
        loud.stdout.contains("Synced with remote."),
        "expected the confirmation without --quiet, got {:?}",
        loud.stdout
    );

    let quiet = fx.run(&["--quiet", "sync"]);
    quiet.assert_ok();
    assert!(
        !quiet.stdout.contains("Synced with remote."),
        "--quiet must silence the confirmation: {:?}",
        quiet.stdout
    );
}

#[test]
fn a_failing_command_leaves_stderr_clean() {
    let fx = Fixture::seeded(5);
    let run = fx.run(&["--offline", "show", "no-such-task"]);
    assert!(!run.success, "the command was supposed to fail");
    // No half-drawn bar, and nothing that would need a terminal to interpret.
    run.assert_no_progress();
}

#[test]
fn a_failing_command_under_forced_progress_leaves_no_bar_behind() {
    let fx = Fixture::seeded(5);
    let run = fx.run_forced(&["--offline", "show", "no-such-task"]);
    assert!(!run.success, "the command was supposed to fail");
    // Progress was allowed to draw here, so the assertion is about cleanup:
    // whatever it drew, it cleared, leaving no label from an unfinished phase.
    for label in REBUILD_LABELS {
        assert!(
            !run.stderr.contains(label),
            "an errored run left {label:?} on screen: {:?}",
            run.stderr
        );
    }
}
