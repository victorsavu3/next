use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

use crate::core::{
    error::{Result, TaskError},
    progress::{NoProgress, ProgressSink, ProgressTask},
    store::{PullResult, VcsBackend},
};
use git2::{build::CheckoutBuilder, Repository};

// Repository is Send but not Sync; wrapping in Mutex makes GitBackend Sync.
pub struct GitBackend {
    repo: Mutex<Repository>,
    /// Path to `.next.lock` — the same file used by `TomlStore::acquire_repo_lock`.
    lock_path: PathBuf,
    /// Working directory (repository root) used when spawning subprocess git calls.
    work_dir: PathBuf,
    /// When `true`, `pull` and `push` run `git` as a subprocess instead of
    /// using libgit2.  Useful when the system git handles authentication
    /// (SSH agents, credential managers) better than the embedded bindings.
    use_subprocess: bool,
    /// Explicit HTTPS credentials supplied at construction time (e.g. from the
    /// MCP config file).  Takes precedence over the `NEXT_GIT_USER` /
    /// `NEXT_GIT_TOKEN` env vars and the system credential helper.
    git_credentials: Option<(String, String)>,
    /// Where fetch/push report their progress. [`NoProgress`] until a caller
    /// installs something via [`VcsBackend::set_progress`], so a library
    /// consumer stays silent. An `Arc` because every VCS method takes `&self`.
    progress: Arc<dyn ProgressSink>,
}

impl GitBackend {
    pub fn open(root: &Path) -> Result<Self> {
        let repo = Repository::open(root)
            .map_err(|e| TaskError::Other(format!("failed to open git repository: {e}")))?;
        Ok(Self {
            repo: Mutex::new(repo),
            lock_path: root.join(".next.lock"),
            work_dir: root.to_path_buf(),
            use_subprocess: false,
            git_credentials: None,
            progress: Arc::new(NoProgress),
        })
    }

    /// The progress sink installed on this backend, [`NoProgress`] by default.
    pub fn progress(&self) -> &dyn ProgressSink {
        &*self.progress
    }

    /// Returns a new `GitBackend` that uses subprocess git for `pull`/`push`.
    ///
    /// All other operations (`commit`, `head_hash`) continue to use libgit2.
    pub fn with_subprocess(mut self, enabled: bool) -> Self {
        self.use_subprocess = enabled;
        self
    }

    /// Attaches explicit HTTPS credentials.  These take precedence over the
    /// `NEXT_GIT_USER` / `NEXT_GIT_TOKEN` environment variables and the system
    /// credential helper when libgit2 performs fetch/push operations.
    pub fn with_credentials(mut self, user: Option<String>, token: Option<String>) -> Self {
        self.git_credentials = match (user, token) {
            (Some(u), Some(t)) => Some((u, t)),
            _ => None,
        };
        self
    }

    /// Acquires the repository-level exclusive lock shared with `TomlStore`.
    /// Released when the returned guard is dropped.  Re-entrant within a thread
    /// via [`crate::core::storage::FileLock`], so a transaction holding the lock can
    /// call `commit` / `pull` / `push` without self-deadlocking.
    fn acquire_repo_lock(&self) -> Result<crate::core::storage::FileLock> {
        crate::core::storage::FileLock::acquire(&self.lock_path)
    }
}

/// Builds a `RemoteCallbacks` that handles SSH (via agent) and HTTP/HTTPS.
///
/// For HTTP/HTTPS, credentials are resolved in this order:
/// 1. `explicit` — user/token pair passed directly (e.g. from the MCP config file).
/// 2. `NEXT_GIT_USER` + `NEXT_GIT_TOKEN` environment variables.
/// 3. The system git credential helper as a fallback.
///
/// The `tried` flag prevents the callback from looping when credentials are
/// rejected — git2 re-invokes the callback on failure, so we return an error
/// on the second call instead of retrying forever.
fn remote_callbacks<'a>(explicit: Option<(String, String)>) -> git2::RemoteCallbacks<'a> {
    let mut tried = false;
    let mut cb = git2::RemoteCallbacks::new();
    cb.credentials(move |url, username, allowed| {
        if tried {
            return Err(git2::Error::from_str("authentication failed"));
        }
        tried = true;

        if allowed.contains(git2::CredentialType::SSH_KEY) {
            return git2::Cred::ssh_key_from_agent(username.unwrap_or("git"));
        }
        if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            // 1. Explicit credentials supplied at construction time.
            if let Some((ref u, ref t)) = explicit {
                return git2::Cred::userpass_plaintext(u, t);
            }
            // 2. Env-var credentials.
            let env_user = std::env::var("NEXT_GIT_USER").ok();
            let env_token = std::env::var("NEXT_GIT_TOKEN").ok();
            if let (Some(u), Some(t)) = (env_user.as_deref(), env_token.as_deref()) {
                return git2::Cred::userpass_plaintext(u, t);
            }
            // 3. System git credential helper.
            let config =
                git2::Config::open_default().map_err(|e| git2::Error::from_str(&e.to_string()))?;
            return git2::Cred::credential_helper(&config, url, username);
        }
        if allowed.contains(git2::CredentialType::DEFAULT) {
            return git2::Cred::default();
        }
        Err(git2::Error::from_str("no supported authentication method"))
    });
    cb
}

/// Detail lines for the two phases a fetch spends its time in. Both are
/// `&'static str`, so setting one allocates nothing on a callback that fires
/// once per packet.
const RECEIVING: &str = "receiving objects";
const RESOLVING: &str = "resolving deltas";

/// The advance one transfer callback invocation should report.
#[derive(Debug, Default, PartialEq, Eq)]
struct TransferStep {
    /// A total to announce with [`ProgressTask::set_total`], or `None` when it
    /// is still unknown or already announced.
    total: Option<u64>,
    /// How far to advance the bar. Never negative and never a re-count: the
    /// caller can pass this straight to [`ProgressTask::inc`].
    delta: u64,
}

/// Converts git2's transfer counters into what [`ProgressTask`] wants.
///
/// The two are shaped differently, and this is the whole reason the type
/// exists: git2 reports an **absolute** position on every invocation
/// (`received_objects()` for a fetch, `current` for a push — each a running
/// count that starts at zero and climbs to the total), while `inc` takes a
/// **delta**. Feeding the absolute position to `inc` would make a 1 000-object
/// fetch report half a million objects. So the last position is kept here and
/// only the difference is emitted.
///
/// Three cases the arithmetic has to survive, all of them observed from real
/// transports rather than invented:
///
/// * **The total arrives late.** git2 reports `total_objects() == 0` until the
///   remote has finished counting, so the bar starts as a spinner and becomes
///   determinate mid-transfer. Zero is treated as "not known yet", never as a
///   real total, and a total is announced once rather than on every packet.
/// * **The counter restarts.** A callback outlives a single phase — a fetch
///   that negotiates twice, a push of several refspecs — and the position then
///   drops back towards zero. A `saturating_sub` reports nothing for that step
///   and rebases on the new position, so the bar stalls for one tick instead
///   of running backwards or wrapping.
/// * **The position repeats.** Callbacks fire on byte progress too, so the
///   same object count arrives many times in a row; each repeat is a zero
///   delta, which the caller skips.
#[derive(Debug, Default)]
struct TransferCounter {
    /// The position the last [`step`](Self::step) was told about.
    seen: u64,
    /// The total already announced, so it is set once and not per packet.
    announced: Option<u64>,
}

impl TransferCounter {
    /// Folds one absolute `(position, total)` report into a [`TransferStep`].
    fn step(&mut self, position: u64, total: u64) -> TransferStep {
        let announce = if total != 0 && self.announced != Some(total) {
            self.announced = Some(total);
            Some(total)
        } else {
            None
        };
        // Saturating, so a restarted counter rebases instead of underflowing.
        let delta = position.saturating_sub(self.seen);
        self.seen = position;
        TransferStep {
            total: announce,
            delta,
        }
    }
}

/// Reports a fetch's object transfer into `task`, upgrading it from a spinner
/// to a determinate bar as soon as git2 knows the object count.
///
/// Only ever called when a real sink is installed — see the `is_noop` guard at
/// each call site — so the closure's cost is paid only when something renders
/// it. The callback returns `true` unconditionally: `false` cancels the fetch,
/// and reporting progress must never be able to fail a transfer.
fn install_fetch_progress<'a>(cb: &mut git2::RemoteCallbacks<'a>, task: &'a dyn ProgressTask) {
    let mut counter = TransferCounter::default();
    let mut phase = "";
    cb.transfer_progress(move |stats| {
        let step = counter.step(
            stats.received_objects() as u64,
            stats.total_objects() as u64,
        );
        if let Some(total) = step.total {
            task.set_total(total);
        }
        if step.delta > 0 {
            task.inc(step.delta);
        }
        // The bar counts objects received, so it sits full while the deltas
        // are resolved; the detail line is what explains the pause. Set only
        // when it changes, and always to a `&'static str`.
        let now = match (stats.received_objects(), stats.total_objects()) {
            (_, 0) => phase, // total not announced yet — say nothing
            (received, total) if received < total => RECEIVING,
            _ => RESOLVING,
        };
        if now != phase {
            phase = now;
            task.set_message(now);
        }
        true
    });
}

/// Reports a push's object transfer into `task`, the same way
/// [`install_fetch_progress`] does — `current` is absolute too, so the same
/// counter converts it. git2 gives no phase here, only counts and bytes.
fn install_push_progress<'a>(cb: &mut git2::RemoteCallbacks<'a>, task: &'a dyn ProgressTask) {
    let mut counter = TransferCounter::default();
    cb.push_transfer_progress(move |current, total, _bytes| {
        let step = counter.step(current as u64, total as u64);
        if let Some(total) = step.total {
            task.set_total(total);
        }
        if step.delta > 0 {
            task.inc(step.delta);
        }
    });
}

/// Environment variables that scope a `git` invocation to a repository.
/// Stripped from every subprocess so the command targets its working
/// directory, never a repository inherited from the caller's environment
/// (e.g. a git hook exporting `GIT_DIR`).
pub(crate) const GIT_SCOPE_VARS: [&str; 5] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
];

/// A `git` subprocess command rooted at `dir` with the scope env stripped.
fn git_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir);
    for var in GIT_SCOPE_VARS {
        cmd.env_remove(var);
    }
    cmd
}

impl GitBackend {
    /// Runs `git pull` as a subprocess in the repository directory.
    ///
    /// Progress is a bare spinner for the lifetime of the child. `git` writes
    /// its own counters ("Receiving objects: 43% …") to *its* stderr, which
    /// `output()` captures and this function only ever inspects for conflict
    /// markers; parsing that stream back into bar updates would mean giving up
    /// the captured stderr the error path reports, so the subprocess mode
    /// deliberately reports duration only, not fraction.
    fn subprocess_pull(&self) -> Result<PullResult> {
        let _pulling = self.progress.begin("Pulling", None);
        let _lock = self.acquire_repo_lock()?;
        let output = git_cmd(&self.work_dir)
            .args(["pull", "--no-edit"])
            .output()
            .map_err(|e| TaskError::Other(format!("spawn git pull: {e}")))?;

        if output.status.success() {
            return Ok(PullResult::Clean);
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("CONFLICT") || stderr.contains("Automatic merge failed") {
            // git pull already left conflict markers in the working tree.
            // We can't easily enumerate conflict paths from subprocess output,
            // so return an empty list — the user sees the conflict markers on disk.
            return Ok(PullResult::Conflicts(vec![]));
        }

        Err(TaskError::Other(format!(
            "git pull failed: {}",
            stderr.trim()
        )))
    }

    /// Runs `git push` as a subprocess in the repository directory.
    ///
    /// A spinner only, for the same reason as [`Self::subprocess_pull`]:
    /// `git`'s own stderr progress is captured for the error message, not
    /// parsed.
    fn subprocess_push(&self) -> Result<()> {
        let _pushing = self.progress.begin("Pushing", None);
        let _lock = self.acquire_repo_lock()?;
        let output = git_cmd(&self.work_dir)
            .args(["push"])
            .output()
            .map_err(|e| TaskError::Other(format!("spawn git push: {e}")))?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(TaskError::Other(format!(
            "git push failed: {}",
            stderr.trim()
        )))
    }
}

impl VcsBackend for GitBackend {
    fn set_progress(&mut self, sink: Arc<dyn ProgressSink>) {
        self.progress = sink;
    }

    fn commit(&self, paths: &[PathBuf], message: &str) -> Result<()> {
        let _lock = self.acquire_repo_lock()?;
        let repo = self
            .repo
            .lock()
            .map_err(|_| TaskError::Other("git lock poisoned".into()))?;

        let workdir = repo
            .workdir()
            .ok_or_else(|| TaskError::Other("bare repository has no working directory".into()))?;

        let mut index = repo
            .index()
            .map_err(|e| TaskError::Other(format!("git index: {e}")))?;

        for path in paths {
            let relative = path.strip_prefix(workdir).map_err(|_| {
                TaskError::Other(format!("path not inside repository: {}", path.display()))
            })?;
            if path.exists() {
                index
                    .add_path(relative)
                    .map_err(|e| TaskError::Other(format!("git add {}: {e}", path.display())))?;
            } else {
                index
                    .remove_path(relative)
                    .map_err(|e| TaskError::Other(format!("git rm {}: {e}", path.display())))?;
            }
        }

        index
            .write()
            .map_err(|e| TaskError::Other(format!("git index write: {e}")))?;

        let tree_oid = index
            .write_tree()
            .map_err(|e| TaskError::Other(format!("git write-tree: {e}")))?;
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| TaskError::Other(format!("git find-tree: {e}")))?;

        let sig = repo
            .signature()
            .map_err(|e| TaskError::Other(format!("git signature: {e}")))?;

        let parent_commit = repo.head().and_then(|h| h.peel_to_commit()).ok();
        let parents: Vec<&git2::Commit> = parent_commit.iter().collect();

        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .map_err(|e| TaskError::Other(format!("git commit: {e}")))?;

        Ok(())
    }

    fn pull(&self) -> Result<PullResult> {
        if self.use_subprocess {
            return self.subprocess_pull();
        }
        // Begun before the lock so the wait for another process's transaction
        // is on screen too, and declared before everything that borrows it so
        // it is finished last — by `Drop`, on every `?` below as well.
        let pulling = self.progress.begin("Pulling", None);
        let _lock = self.acquire_repo_lock()?;
        let repo = self
            .repo
            .lock()
            .map_err(|_| TaskError::Other("git lock poisoned".into()))?;

        let mut remote = repo
            .find_remote("origin")
            .map_err(|e| TaskError::Other(format!("git remote 'origin': {e}")))?;

        let mut callbacks = remote_callbacks(self.git_credentials.clone());
        // Under the default `NoProgress` the callback is not installed at all,
        // rather than installed and discarded: git2 skips the per-packet
        // indexer bookkeeping, and a fetch of a large repo pays literally
        // nothing for a sink nobody is watching.
        if !self.progress.is_noop() {
            install_fetch_progress(&mut callbacks, &*pulling);
        }
        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);
        remote
            .fetch(&[] as &[&str], Some(&mut fetch_opts), None)
            .map_err(|e| TaskError::Other(format!("git fetch: {e}")))?;
        drop(remote);

        let fetch_head = repo
            .find_reference("FETCH_HEAD")
            .map_err(|e| TaskError::Other(format!("FETCH_HEAD: {e}")))?;
        let fetch_commit = repo
            .reference_to_annotated_commit(&fetch_head)
            .map_err(|e| TaskError::Other(format!("annotated commit: {e}")))?;

        let (analysis, _) = repo
            .merge_analysis(&[&fetch_commit])
            .map_err(|e| TaskError::Other(format!("merge analysis: {e}")))?;

        if analysis.is_up_to_date() {
            return Ok(PullResult::Clean);
        }

        if analysis.is_fast_forward() {
            if analysis.is_unborn() {
                // Local branch does not exist yet (fresh clone / init with no commits).
                // Create the branch reference directly instead of updating an existing one.
                let refname = repo
                    .find_reference("HEAD")
                    .ok()
                    .and_then(|h| h.symbolic_target().map(str::to_owned))
                    .unwrap_or_else(|| "refs/heads/master".to_owned());
                repo.reference(&refname, fetch_commit.id(), true, "pull: initial")
                    .map_err(|e| TaskError::Other(format!("create branch ref: {e}")))?;
            } else {
                let refname = {
                    let head = repo
                        .head()
                        .map_err(|e| TaskError::Other(format!("git HEAD: {e}")))?;
                    head.name()
                        .ok_or_else(|| TaskError::Other("invalid HEAD reference".into()))?
                        .to_owned()
                };
                let mut reference = repo
                    .find_reference(&refname)
                    .map_err(|e| TaskError::Other(format!("find ref: {e}")))?;
                reference
                    .set_target(fetch_commit.id(), "pull: fast-forward")
                    .map_err(|e| TaskError::Other(format!("fast-forward: {e}")))?;
            }
            repo.checkout_head(Some(CheckoutBuilder::default().force()))
                .map_err(|e| TaskError::Other(format!("checkout HEAD: {e}")))?;
            return Ok(PullResult::Clean);
        }

        // Non-fast-forward merge.
        let head_oid = repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map(|c| c.id())
            .map_err(|e| TaskError::Other(format!("HEAD commit: {e}")))?;

        repo.merge(&[&fetch_commit], None, None)
            .map_err(|e| TaskError::Other(format!("git merge: {e}")))?;

        let has_conflicts = repo
            .index()
            .map_err(|e| TaskError::Other(format!("git index after merge: {e}")))?
            .has_conflicts();

        if has_conflicts {
            let conflict_paths: Vec<PathBuf> = {
                let index = repo
                    .index()
                    .map_err(|e| TaskError::Other(format!("git index: {e}")))?;
                let paths: Vec<PathBuf> = index
                    .conflicts()
                    .map_err(|e| TaskError::Other(format!("git conflicts: {e}")))?
                    .filter_map(|c| {
                        c.ok().and_then(|conflict| {
                            conflict.our.or(conflict.their).map(|entry| {
                                PathBuf::from(String::from_utf8_lossy(&entry.path).into_owned())
                            })
                        })
                    })
                    .collect();
                paths
            };
            repo.cleanup_state()
                .map_err(|e| TaskError::Other(format!("cleanup state: {e}")))?;
            return Ok(PullResult::Conflicts(conflict_paths));
        }

        // Complete the merge commit.
        let tree_oid = {
            let mut index = repo
                .index()
                .map_err(|e| TaskError::Other(format!("git index: {e}")))?;
            index
                .write_tree()
                .map_err(|e| TaskError::Other(format!("write-tree: {e}")))?
        };
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| TaskError::Other(format!("find tree: {e}")))?;
        let sig = repo
            .signature()
            .map_err(|e| TaskError::Other(format!("git signature: {e}")))?;
        let head_commit = repo
            .find_commit(head_oid)
            .map_err(|e| TaskError::Other(format!("find HEAD commit: {e}")))?;
        let fetch_commit_obj = repo
            .find_commit(fetch_commit.id())
            .map_err(|e| TaskError::Other(format!("find fetch commit: {e}")))?;

        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "Merge remote-tracking branch",
            &tree,
            &[&head_commit, &fetch_commit_obj],
        )
        .map_err(|e| TaskError::Other(format!("merge commit: {e}")))?;
        repo.cleanup_state()
            .map_err(|e| TaskError::Other(format!("cleanup state: {e}")))?;

        Ok(PullResult::Clean)
    }

    fn push(&self) -> Result<()> {
        if self.use_subprocess {
            return self.subprocess_push();
        }
        // Same shape as `pull`: a spinner covering the lock wait and the
        // whole transfer, upgraded to a bar by the callback below.
        let pushing = self.progress.begin("Pushing", None);
        let _lock = self.acquire_repo_lock()?;
        let repo = self
            .repo
            .lock()
            .map_err(|_| TaskError::Other("git lock poisoned".into()))?;

        let branch = {
            let head = repo
                .head()
                .map_err(|e| TaskError::Other(format!("git HEAD: {e}")))?;
            head.shorthand()
                .ok_or_else(|| TaskError::Other("no current branch".into()))?
                .to_owned()
        };
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");

        let mut remote = repo
            .find_remote("origin")
            .map_err(|e| TaskError::Other(format!("git remote 'origin': {e}")))?;
        let mut callbacks = remote_callbacks(self.git_credentials.clone());
        if !self.progress.is_noop() {
            install_push_progress(&mut callbacks, &*pushing);
        }
        let mut push_opts = git2::PushOptions::new();
        push_opts.remote_callbacks(callbacks);
        remote
            .push(&[refspec.as_str()], Some(&mut push_opts))
            .map_err(|e| TaskError::Other(format!("git push: {e}")))?;

        Ok(())
    }

    fn head_hash(&self) -> Result<String> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| TaskError::Other("git lock poisoned".into()))?;
        // Bind to a local so temporaries borrowing `repo` are dropped before `repo` is.
        let result: Result<String> = match repo.head() {
            Ok(head) => match head.peel_to_commit() {
                Ok(commit) => Ok(commit.id().to_string()),
                Err(e) => Err(TaskError::Other(format!("peel HEAD: {e}"))),
            },
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok("unborn".into()),
            Err(e) => Err(TaskError::Other(format!("git HEAD: {e}"))),
        };
        result
    }

    fn diff(&self) -> crate::core::error::Result<String> {
        let _lock = self.acquire_repo_lock()?;

        let status = git_cmd(&self.work_dir)
            .args(["status", "--short"])
            .output()
            .map_err(|e| TaskError::Other(format!("git status: {e}")))?;

        let diff = git_cmd(&self.work_dir)
            .args(["diff", "HEAD"])
            .output()
            .map_err(|e| TaskError::Other(format!("git diff: {e}")))?;

        let status_text = String::from_utf8_lossy(&status.stdout);
        let diff_text = String::from_utf8_lossy(&diff.stdout);

        let mut out = String::new();
        if !status_text.trim().is_empty() {
            out.push_str("# git status\n");
            out.push_str(&status_text);
        }
        if !diff_text.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("# git diff HEAD\n");
            out.push_str(&diff_text);
        }
        if out.is_empty() {
            out.push_str("working tree is clean");
        }
        Ok(out)
    }

    fn force_pull(&self) -> crate::core::error::Result<String> {
        let _lock = self.acquire_repo_lock()?;

        let fetch = git_cmd(&self.work_dir)
            .args(["fetch", "origin"])
            .output()
            .map_err(|e| TaskError::Other(format!("git fetch: {e}")))?;
        if !fetch.status.success() {
            let stderr = String::from_utf8_lossy(&fetch.stderr);
            return Err(TaskError::Other(format!(
                "git fetch failed: {}",
                stderr.trim()
            )));
        }

        let reset = git_cmd(&self.work_dir)
            .args(["reset", "--hard", "FETCH_HEAD"])
            .output()
            .map_err(|e| TaskError::Other(format!("git reset: {e}")))?;
        if !reset.status.success() {
            let stderr = String::from_utf8_lossy(&reset.stderr);
            return Err(TaskError::Other(format!(
                "git reset failed: {}",
                stderr.trim()
            )));
        }

        // Return the new HEAD so callers can update any caches.
        let rev = git_cmd(&self.work_dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| TaskError::Other(format!("git rev-parse: {e}")))?;
        let head = String::from_utf8_lossy(&rev.stdout).trim().to_owned();
        Ok(head)
    }
}

/// Git-derived first/last commit times for every task file under `tasks_dir`.
///
/// One `git log` walk over the directory, newest commit first. Each file's
/// dates are recorded under two keys: its exact filename, and — when the stem
/// carries the 8-hex-char UUID suffix — that suffix, which stays stable across
/// slug and title renames. Used only to backfill the cache's date columns on
/// a full rebuild; steady-state maintenance is incremental.
pub(crate) fn task_git_dates(
    tasks_dir: &Path,
) -> crate::core::error::Result<HashMap<String, crate::core::scoring::TaskDates>> {
    use crate::core::scoring::TaskDates;
    use chrono::{DateTime, Utc};

    // Output format: a "COMMIT <unix-timestamp>" header line followed by one
    // path per changed task file.
    let output = git_cmd(tasks_dir)
        .args(["log", "--format=COMMIT %at", "--name-only", "--", "."])
        .output()
        .map_err(|e| TaskError::Other(format!("git log for task dates: {e}")))?;

    let text = String::from_utf8_lossy(&output.stdout);
    let mut dates: HashMap<String, TaskDates> = HashMap::new();
    let mut current_ts: Option<DateTime<Utc>> = None;

    for line in text.lines() {
        if let Some(ts_str) = line.strip_prefix("COMMIT ") {
            let secs: i64 = ts_str.trim().parse().unwrap_or(0);
            current_ts = DateTime::from_timestamp(secs, 0);
        } else if !line.is_empty() {
            let Some(ts) = current_ts else { continue };
            let file = std::path::Path::new(line);
            let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");

            // git log is newest→oldest: the first sighting of a key fixes
            // updated_at, every later (older) sighting moves created_at back.
            let mut record = |key: &str| {
                let entry = dates.entry(key.to_owned()).or_insert(TaskDates {
                    created_at: ts,
                    updated_at: ts,
                });
                entry.created_at = ts;
            };
            record(name);
            let hex8 = stem.rsplit('-').next().unwrap_or(stem);
            if hex8.len() == 8 && hex8.chars().all(|c| c.is_ascii_hexdigit()) {
                record(hex8);
            }
        }
    }

    Ok(dates)
}

/// Author time of `rev` in the repository at `root`, if it resolves.
pub(crate) fn commit_time(root: &Path, rev: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let repo = Repository::open(root).ok()?;
    let commit = repo.revparse_single(rev).ok()?.peel_to_commit().ok()?;
    chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
}

/// Blob SHA of `rel_path` in the HEAD tree of the repository at `root`.
pub(crate) fn blob_id_at_head(root: &Path, rel_path: &str) -> Option<String> {
    let repo = Repository::open(root).ok()?;
    let tree = repo.head().ok()?.peel_to_commit().ok()?.tree().ok()?;
    let entry = tree.get_path(Path::new(rel_path)).ok()?;
    Some(entry.id().to_string())
}

/// Content of the blob `sha` in the repository at `root`, as UTF-8.
///
/// One object read — recovering a pruned segment never walks history. When
/// the object is not in the local store (a partial clone omits historical
/// blobs), falls back to `git cat-file`, whose promisor machinery fetches
/// the blob from the remote on demand; libgit2 cannot do that.
pub(crate) fn blob_content(root: &Path, sha: &str) -> Option<String> {
    let local = (|| {
        let repo = Repository::open(root).ok()?;
        let oid = git2::Oid::from_str(sha).ok()?;
        let blob = repo.find_blob(oid).ok()?;
        String::from_utf8(blob.content().to_vec()).ok()
    })();
    if local.is_some() {
        return local;
    }
    let output = git_cmd(root)
        .args(["cat-file", "blob", sha])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// A file change between two commits, as reported by a tree diff.
///
/// Paths are repo-relative with forward slashes, exactly as git reports them.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileChange {
    /// The file exists in the new tree — its content should be (re)loaded.
    Upsert(String),
    /// The file is gone from the new tree.
    Delete(String),
}

/// Diffs the trees of `old_head`..`new_head` in the repository at `root`.
///
/// Returns `None` when the diff cannot be computed — unknown revisions (an
/// unborn branch, a head garbage-collected away, a corrupt store) or an
/// unrepresentable delta — in which case the caller must fall back to a full
/// scan. Cost is proportional to the number of changed files, not to history
/// length or tree size.
pub(crate) fn changed_paths(
    root: &Path,
    old_head: &str,
    new_head: &str,
) -> Option<Vec<FileChange>> {
    let repo = Repository::open(root).ok()?;
    let tree_of = |rev: &str| {
        repo.revparse_single(rev)
            .ok()?
            .peel_to_commit()
            .ok()?
            .tree()
            .ok()
    };
    let old_tree = tree_of(old_head)?;
    let new_tree = tree_of(new_head)?;
    let diff = repo
        .diff_tree_to_tree(Some(&old_tree), Some(&new_tree), None)
        .ok()?;

    let mut changes = Vec::new();
    for delta in diff.deltas() {
        let path_of = |file: git2::DiffFile<'_>| Some(file.path()?.to_str()?.to_owned());
        use git2::Delta;
        match delta.status() {
            Delta::Added | Delta::Modified | Delta::Copied | Delta::Typechange => {
                changes.push(FileChange::Upsert(path_of(delta.new_file())?));
            }
            Delta::Deleted => {
                changes.push(FileChange::Delete(path_of(delta.old_file())?));
            }
            // Rename detection is off, so renames arrive as Delete + Add;
            // handle the status anyway in case a caller enables it later.
            Delta::Renamed => {
                changes.push(FileChange::Delete(path_of(delta.old_file())?));
                changes.push(FileChange::Upsert(path_of(delta.new_file())?));
            }
            // Anything else (conflicts, unreadable entries) → full scan.
            _ => return None,
        }
    }
    Some(changes)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn init_repo(dir: &Path) -> Repository {
        let repo = Repository::init(dir).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Test User").unwrap();
            config.set_str("user.email", "test@example.com").unwrap();
        }
        repo
    }

    /// A repository with one commit and a bare `origin` it already tracks.
    ///
    /// The upstream is configured by the seed push because `subprocess_pull` /
    /// `subprocess_push` shell out to a bare `git pull` / `git push`, which
    /// refuse to guess a branch without one. Both directories are returned so
    /// the caller keeps them alive; dropping either deletes the repo.
    fn repo_with_remote() -> (tempfile::TempDir, tempfile::TempDir) {
        use crate::core::test_git::{git, init_test_repo};

        let remote = tempfile::TempDir::new().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);

        let local = tempfile::TempDir::new().unwrap();
        init_test_repo(local.path());
        fs::write(local.path().join("seed.txt"), "seed").unwrap();
        git(local.path(), &["add", "seed.txt"]);
        git(local.path(), &["commit", "-q", "-m", "seed"]);
        git(
            local.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        git(local.path(), &["push", "-q", "-u", "origin", "HEAD"]);
        (local, remote)
    }

    /// Commits a new file, so the next push has something to transfer.
    fn commit_a_change(backend: &GitBackend, root: &Path, name: &str) {
        let file = root.join(name);
        fs::write(&file, name).unwrap();
        backend.commit(&[file], name).unwrap();
    }

    #[test]
    fn a_fresh_backend_reports_no_progress() {
        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let backend = GitBackend::open(dir.path()).unwrap();
        assert!(
            backend.progress().is_noop(),
            "a backend nobody handed a sink must stay silent"
        );
    }

    #[test]
    fn set_progress_installs_the_sink_for_later_fetches() {
        use crate::core::progress::testing::RecordingSink;

        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let mut backend = GitBackend::open(dir.path()).unwrap();
        let sink = Arc::new(RecordingSink::new());
        backend.set_progress(sink.clone());
        drop(backend.progress().begin("fetch", None));
        assert_eq!(sink.labels(), vec!["fetch".to_owned()]);
    }

    #[test]
    fn head_hash_on_empty_repo_returns_unborn() {
        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let backend = GitBackend::open(dir.path()).unwrap();
        assert_eq!(backend.head_hash().unwrap(), "unborn");
    }

    #[test]
    fn commit_and_head_hash() {
        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let backend = GitBackend::open(dir.path()).unwrap();

        let file = dir.path().join("hello.txt");
        fs::write(&file, "hello").unwrap();
        backend.commit(&[file], "initial commit").unwrap();

        let hash = backend.head_hash().unwrap();
        assert_eq!(hash.len(), 40);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn second_commit_advances_head() {
        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let backend = GitBackend::open(dir.path()).unwrap();

        let file = dir.path().join("a.txt");
        fs::write(&file, "v1").unwrap();
        backend
            .commit(std::slice::from_ref(&file), "first")
            .unwrap();
        let hash1 = backend.head_hash().unwrap();

        fs::write(&file, "v2").unwrap();
        backend.commit(&[file], "second").unwrap();
        let hash2 = backend.head_hash().unwrap();

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn commit_deleted_file() {
        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let backend = GitBackend::open(dir.path()).unwrap();

        let file = dir.path().join("to-delete.txt");
        fs::write(&file, "data").unwrap();
        backend
            .commit(std::slice::from_ref(&file), "add file")
            .unwrap();

        fs::remove_file(&file).unwrap();
        backend.commit(&[file], "remove file").unwrap();

        let hash = backend.head_hash().unwrap();
        assert_eq!(hash.len(), 40);
    }

    #[test]
    fn changed_paths_reports_add_modify_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let backend = GitBackend::open(dir.path()).unwrap();

        let f1 = dir.path().join("one.txt");
        let f2 = dir.path().join("two.txt");
        fs::write(&f1, "1").unwrap();
        fs::write(&f2, "2").unwrap();
        backend.commit(&[f1.clone(), f2.clone()], "first").unwrap();
        let h1 = backend.head_hash().unwrap();

        fs::write(&f1, "1 modified").unwrap();
        fs::remove_file(&f2).unwrap();
        let f3 = dir.path().join("three.txt");
        fs::write(&f3, "3").unwrap();
        backend.commit(&[f1, f2, f3], "second").unwrap();
        let h2 = backend.head_hash().unwrap();

        let mut changes = changed_paths(dir.path(), &h1, &h2).unwrap();
        changes.sort_by_key(|c| match c {
            FileChange::Upsert(p) | FileChange::Delete(p) => p.clone(),
        });
        assert_eq!(
            changes,
            vec![
                FileChange::Upsert("one.txt".into()),
                FileChange::Upsert("three.txt".into()),
                FileChange::Delete("two.txt".into()),
            ]
        );

        // Unknown revisions → None; the caller falls back to a full scan.
        assert!(
            changed_paths(dir.path(), "0000000000000000000000000000000000000000", &h2).is_none()
        );
        assert!(changed_paths(dir.path(), "unborn", &h2).is_none());
    }

    // ── The absolute → delta conversion ──────────────────────────────────
    //
    // git2 reports where a transfer *is*; `ProgressTask::inc` is told how far
    // it moved. Everything that can go wrong with progress on the network
    // paths goes wrong here, so this is tested on its own rather than through
    // a transport.

    #[test]
    fn transfer_counter_turns_absolute_positions_into_deltas() {
        let mut counter = TransferCounter::default();
        assert_eq!(
            counter.step(0, 10),
            TransferStep {
                total: Some(10),
                delta: 0
            },
            "the first report announces the total and moves nothing"
        );
        assert_eq!(
            counter.step(3, 10),
            TransferStep {
                total: None,
                delta: 3
            }
        );
        assert_eq!(
            counter.step(3, 10),
            TransferStep {
                total: None,
                delta: 0
            },
            "a repeated position is not re-counted"
        );
        assert_eq!(
            counter.step(10, 10),
            TransferStep {
                total: None,
                delta: 7
            }
        );
    }

    #[test]
    fn transfer_counter_deltas_sum_to_the_final_position() {
        // The property that makes the bar land exactly on full: whatever the
        // callback reports, the deltas add up to the last absolute position.
        let mut counter = TransferCounter::default();
        let sum: u64 = [1, 1, 2, 3, 5, 8, 13, 21, 34, 55]
            .into_iter()
            .map(|position| counter.step(position, 55).delta)
            .sum();
        assert_eq!(sum, 55);
    }

    #[test]
    fn transfer_counter_announces_a_late_total_once() {
        // git2 reports a total of 0 until the remote finishes counting: the
        // task stays a spinner, then becomes a bar mid-transfer.
        let mut counter = TransferCounter::default();
        assert_eq!(
            counter.step(0, 0),
            TransferStep {
                total: None,
                delta: 0
            },
            "zero is 'unknown', never a real total"
        );
        assert_eq!(
            counter.step(2, 0),
            TransferStep {
                total: None,
                delta: 2
            },
            "a spinner still advances"
        );
        assert_eq!(
            counter.step(4, 8),
            TransferStep {
                total: Some(8),
                delta: 2
            },
            "the total arrives late and is announced then"
        );
        assert_eq!(
            counter.step(5, 8),
            TransferStep {
                total: None,
                delta: 1
            },
            "and is not re-announced on every later packet"
        );
    }

    #[test]
    fn transfer_counter_rebases_when_the_counter_restarts() {
        // A second phase on the same callback starts back near zero. The bar
        // must stall for one tick, not run backwards or wrap around.
        let mut counter = TransferCounter::default();
        assert_eq!(counter.step(5, 5).delta, 5);
        assert_eq!(
            counter.step(0, 9),
            TransferStep {
                total: Some(9),
                delta: 0
            },
            "the restart reports no advance"
        );
        assert_eq!(
            counter.step(4, 9),
            TransferStep {
                total: None,
                delta: 4
            },
            "and counts from the new base afterwards"
        );
    }

    // ── The network paths ────────────────────────────────────────────────

    #[test]
    fn a_push_reports_a_pushing_phase() {
        use crate::core::progress::testing::RecordingSink;

        let (local, _remote) = repo_with_remote();
        let mut backend = GitBackend::open(local.path()).unwrap();
        let sink = Arc::new(RecordingSink::new());
        backend.set_progress(sink.clone());

        commit_a_change(&backend, local.path(), "pushed.txt");
        backend.push().unwrap();

        assert_eq!(
            sink.labels(),
            vec!["Pushing".to_owned()],
            "one phase, and `commit` reports none"
        );
        let rec = sink.task("Pushing").expect("push reports a phase");
        assert_eq!(rec.finishes, 1, "exactly one finish: {rec:?}");
        // The local transport may complete without ever reporting a count, so
        // the lifecycle is what is pinned; when a total does arrive, the
        // deltas must not overshoot it.
        if let Some(total) = rec.total {
            assert!(rec.progressed <= total, "{rec:?}");
        }
    }

    #[test]
    fn a_pull_that_fast_forwards_reports_a_pulling_phase() {
        use crate::core::progress::testing::RecordingSink;
        use crate::core::test_git::git;

        let (local, remote) = repo_with_remote();

        // A second working copy pushes a commit, so the pull below actually
        // transfers objects instead of returning "up to date" immediately.
        let other = tempfile::TempDir::new().unwrap();
        git(
            other.path(),
            &["clone", "-q", remote.path().to_str().unwrap(), "."],
        );
        git(other.path(), &["config", "user.email", "test@test.com"]);
        git(other.path(), &["config", "user.name", "Test"]);
        fs::write(other.path().join("second.txt"), "second").unwrap();
        git(other.path(), &["add", "second.txt"]);
        git(other.path(), &["commit", "-q", "-m", "second"]);
        git(other.path(), &["push", "-q", "origin", "HEAD"]);

        let mut backend = GitBackend::open(local.path()).unwrap();
        let sink = Arc::new(RecordingSink::new());
        backend.set_progress(sink.clone());

        assert!(matches!(backend.pull().unwrap(), PullResult::Clean));
        assert!(
            local.path().join("second.txt").exists(),
            "the pull fast-forwarded the checkout"
        );

        assert_eq!(sink.labels(), vec!["Pulling".to_owned()]);
        let rec = sink.task("Pulling").expect("pull reports a phase");
        assert_eq!(rec.finishes, 1, "exactly one finish: {rec:?}");
        if let Some(total) = rec.total {
            assert!(rec.progressed <= total, "{rec:?}");
        }
        for message in &rec.messages {
            assert!(
                message == RECEIVING || message == RESOLVING,
                "detail lines are the two static phase names: {message}"
            );
        }
    }

    #[test]
    fn a_failed_pull_still_finishes_its_phase_once() {
        use crate::core::progress::testing::RecordingSink;

        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let mut backend = GitBackend::open(dir.path()).unwrap();
        let sink = Arc::new(RecordingSink::new());
        backend.set_progress(sink.clone());

        let err = backend.pull().unwrap_err().to_string();
        assert!(err.contains("git remote 'origin'"), "{err}");

        let rec = sink.task("Pulling").expect("the phase was begun first");
        assert_eq!(
            rec.finishes, 1,
            "`Drop` finishes the phase on the `?` path: {rec:?}"
        );
    }

    #[test]
    fn a_failed_push_still_finishes_its_phase_once() {
        use crate::core::progress::testing::RecordingSink;

        let dir = tempfile::TempDir::new().unwrap();
        init_repo(dir.path());
        let mut backend = GitBackend::open(dir.path()).unwrap();
        let sink = Arc::new(RecordingSink::new());
        backend.set_progress(sink.clone());
        commit_a_change(&backend, dir.path(), "only.txt");

        let err = backend.push().unwrap_err().to_string();
        assert!(err.contains("git remote 'origin'"), "{err}");

        let rec = sink.task("Pushing").expect("the phase was begun first");
        assert_eq!(
            rec.finishes, 1,
            "`Drop` finishes the phase on the `?` path: {rec:?}"
        );
    }

    #[test]
    fn subprocess_mode_reports_the_same_two_phases() {
        use crate::core::progress::testing::RecordingSink;

        let (local, _remote) = repo_with_remote();
        let mut backend = GitBackend::open(local.path())
            .unwrap()
            .with_subprocess(true);
        let sink = Arc::new(RecordingSink::new());
        backend.set_progress(sink.clone());

        commit_a_change(&backend, local.path(), "shelled-out.txt");
        backend.push().unwrap();
        assert!(matches!(backend.pull().unwrap(), PullResult::Clean));

        assert_eq!(
            sink.labels(),
            vec!["Pushing".to_owned(), "Pulling".to_owned()]
        );
        for label in ["Pushing", "Pulling"] {
            let rec = sink.task(label).expect("phase begun");
            assert_eq!(rec.finishes, 1, "{label}: {rec:?}");
            // `git`'s own stderr counters are captured for the error path, not
            // parsed, so the subprocess phases stay spinners: no total, no
            // advance, no detail lines.
            assert_eq!(rec.total, None, "{label}: {rec:?}");
            assert_eq!(rec.progressed, 0, "{label}: {rec:?}");
            assert!(rec.messages.is_empty(), "{label}: {rec:?}");
        }
    }

    #[test]
    fn a_backend_without_a_sink_pulls_and_pushes_unchanged() {
        let (local, _remote) = repo_with_remote();
        let backend = GitBackend::open(local.path()).unwrap();
        assert!(backend.progress().is_noop());

        commit_a_change(&backend, local.path(), "silent.txt");
        backend.push().unwrap();
        assert!(matches!(backend.pull().unwrap(), PullResult::Clean));
    }
}
