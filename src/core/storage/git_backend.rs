use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

use git2::{build::CheckoutBuilder, Repository};
use crate::core::{
    error::{TaskError, Result},
    store::{PullResult, VcsBackend},
};

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
        })
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
            let env_user  = std::env::var("NEXT_GIT_USER").ok();
            let env_token = std::env::var("NEXT_GIT_TOKEN").ok();
            if let (Some(u), Some(t)) = (env_user.as_deref(), env_token.as_deref()) {
                return git2::Cred::userpass_plaintext(u, t);
            }
            // 3. System git credential helper.
            let config = git2::Config::open_default()
                .map_err(|e| git2::Error::from_str(&e.to_string()))?;
            return git2::Cred::credential_helper(&config, url, username);
        }
        if allowed.contains(git2::CredentialType::DEFAULT) {
            return git2::Cred::default();
        }
        Err(git2::Error::from_str("no supported authentication method"))
    });
    cb
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
    fn subprocess_pull(&self) -> Result<PullResult> {
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
    fn subprocess_push(&self) -> Result<()> {
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
        let _lock = self.acquire_repo_lock()?;
        let repo = self
            .repo
            .lock()
            .map_err(|_| TaskError::Other("git lock poisoned".into()))?;

        let mut remote = repo
            .find_remote("origin")
            .map_err(|e| TaskError::Other(format!("git remote 'origin': {e}")))?;

        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.remote_callbacks(remote_callbacks(self.git_credentials.clone()));
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
        let mut push_opts = git2::PushOptions::new();
        push_opts.remote_callbacks(remote_callbacks(self.git_credentials.clone()));
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
            return Err(TaskError::Other(format!("git fetch failed: {}", stderr.trim())));
        }

        let reset = git_cmd(&self.work_dir)
            .args(["reset", "--hard", "FETCH_HEAD"])
            .output()
            .map_err(|e| TaskError::Other(format!("git reset: {e}")))?;
        if !reset.status.success() {
            let stderr = String::from_utf8_lossy(&reset.stderr);
            return Err(TaskError::Other(format!("git reset failed: {}", stderr.trim())));
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
            let Some(name) = file.file_name().and_then(|n| n.to_str()) else { continue };
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
pub(crate) fn changed_paths(root: &Path, old_head: &str, new_head: &str) -> Option<Vec<FileChange>> {
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
        backend.commit(std::slice::from_ref(&file), "first").unwrap();
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
        backend.commit(std::slice::from_ref(&file), "add file").unwrap();

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
        assert!(changed_paths(dir.path(), "0000000000000000000000000000000000000000", &h2).is_none());
        assert!(changed_paths(dir.path(), "unborn", &h2).is_none());
    }
}
