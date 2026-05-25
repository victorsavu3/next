use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use git2::{build::CheckoutBuilder, Repository};
use next::{
    error::{AppError, Result},
    store::{PullResult, VcsBackend},
};

// Repository is Send but not Sync; wrapping in Mutex makes GitBackend Sync.
pub struct GitBackend {
    repo: Mutex<Repository>,
}

impl GitBackend {
    pub fn open(root: &Path) -> Result<Self> {
        let repo = Repository::open(root)
            .map_err(|e| AppError::Other(format!("failed to open git repository: {e}")))?;
        Ok(Self {
            repo: Mutex::new(repo),
        })
    }
}

fn ssh_callbacks<'a>() -> git2::RemoteCallbacks<'a> {
    let mut cb = git2::RemoteCallbacks::new();
    cb.credentials(|_url, username, _allowed| {
        git2::Cred::ssh_key_from_agent(username.unwrap_or("git"))
    });
    cb
}

impl VcsBackend for GitBackend {
    fn commit(&self, paths: &[PathBuf], message: &str) -> Result<()> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| AppError::Other("git lock poisoned".into()))?;

        let workdir = repo
            .workdir()
            .ok_or_else(|| AppError::Other("bare repository has no working directory".into()))?;

        let mut index = repo
            .index()
            .map_err(|e| AppError::Other(format!("git index: {e}")))?;

        for path in paths {
            let relative = path.strip_prefix(workdir).map_err(|_| {
                AppError::Other(format!("path not inside repository: {}", path.display()))
            })?;
            if path.exists() {
                index
                    .add_path(relative)
                    .map_err(|e| AppError::Other(format!("git add {}: {e}", path.display())))?;
            } else {
                index
                    .remove_path(relative)
                    .map_err(|e| AppError::Other(format!("git rm {}: {e}", path.display())))?;
            }
        }

        index
            .write()
            .map_err(|e| AppError::Other(format!("git index write: {e}")))?;

        let tree_oid = index
            .write_tree()
            .map_err(|e| AppError::Other(format!("git write-tree: {e}")))?;
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| AppError::Other(format!("git find-tree: {e}")))?;

        let sig = repo
            .signature()
            .map_err(|e| AppError::Other(format!("git signature: {e}")))?;

        let parent_commit = repo.head().and_then(|h| h.peel_to_commit()).ok();
        let parents: Vec<&git2::Commit> = parent_commit.iter().collect();

        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .map_err(|e| AppError::Other(format!("git commit: {e}")))?;

        Ok(())
    }

    fn pull(&self) -> Result<PullResult> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| AppError::Other("git lock poisoned".into()))?;

        let mut remote = repo
            .find_remote("origin")
            .map_err(|e| AppError::Other(format!("git remote 'origin': {e}")))?;

        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.remote_callbacks(ssh_callbacks());
        remote
            .fetch(&[] as &[&str], Some(&mut fetch_opts), None)
            .map_err(|e| AppError::Other(format!("git fetch: {e}")))?;
        drop(remote);

        let fetch_head = repo
            .find_reference("FETCH_HEAD")
            .map_err(|e| AppError::Other(format!("FETCH_HEAD: {e}")))?;
        let fetch_commit = repo
            .reference_to_annotated_commit(&fetch_head)
            .map_err(|e| AppError::Other(format!("annotated commit: {e}")))?;

        let (analysis, _) = repo
            .merge_analysis(&[&fetch_commit])
            .map_err(|e| AppError::Other(format!("merge analysis: {e}")))?;

        if analysis.is_up_to_date() {
            return Ok(PullResult::Clean);
        }

        if analysis.is_fast_forward() {
            let refname = {
                let head = repo
                    .head()
                    .map_err(|e| AppError::Other(format!("git HEAD: {e}")))?;
                head.name()
                    .ok_or_else(|| AppError::Other("invalid HEAD reference".into()))?
                    .to_owned()
            };
            let mut reference = repo
                .find_reference(&refname)
                .map_err(|e| AppError::Other(format!("find ref: {e}")))?;
            reference
                .set_target(fetch_commit.id(), "pull: fast-forward")
                .map_err(|e| AppError::Other(format!("fast-forward: {e}")))?;
            drop(reference);
            repo.checkout_head(Some(CheckoutBuilder::default().force()))
                .map_err(|e| AppError::Other(format!("checkout HEAD: {e}")))?;
            return Ok(PullResult::Clean);
        }

        // Non-fast-forward merge.
        let head_oid = repo
            .head()
            .and_then(|h| h.peel_to_commit())
            .map(|c| c.id())
            .map_err(|e| AppError::Other(format!("HEAD commit: {e}")))?;

        repo.merge(&[&fetch_commit], None, None)
            .map_err(|e| AppError::Other(format!("git merge: {e}")))?;

        let has_conflicts = repo.index().map(|i| i.has_conflicts()).unwrap_or(false);

        if has_conflicts {
            let conflict_paths: Vec<PathBuf> = {
                let index = repo
                    .index()
                    .map_err(|e| AppError::Other(format!("git index: {e}")))?;
                let paths: Vec<PathBuf> = index
                    .conflicts()
                    .map_err(|e| AppError::Other(format!("git conflicts: {e}")))?
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
                .map_err(|e| AppError::Other(format!("cleanup state: {e}")))?;
            return Ok(PullResult::Conflicts(conflict_paths));
        }

        // Complete the merge commit.
        let tree_oid = {
            let mut index = repo
                .index()
                .map_err(|e| AppError::Other(format!("git index: {e}")))?;
            index
                .write_tree()
                .map_err(|e| AppError::Other(format!("write-tree: {e}")))?
        };
        let tree = repo
            .find_tree(tree_oid)
            .map_err(|e| AppError::Other(format!("find tree: {e}")))?;
        let sig = repo
            .signature()
            .map_err(|e| AppError::Other(format!("git signature: {e}")))?;
        let head_commit = repo
            .find_commit(head_oid)
            .map_err(|e| AppError::Other(format!("find HEAD commit: {e}")))?;
        let fetch_commit_obj = repo
            .find_commit(fetch_commit.id())
            .map_err(|e| AppError::Other(format!("find fetch commit: {e}")))?;

        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "Merge remote-tracking branch",
            &tree,
            &[&head_commit, &fetch_commit_obj],
        )
        .map_err(|e| AppError::Other(format!("merge commit: {e}")))?;
        repo.cleanup_state()
            .map_err(|e| AppError::Other(format!("cleanup state: {e}")))?;

        Ok(PullResult::Clean)
    }

    fn push(&self) -> Result<()> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| AppError::Other("git lock poisoned".into()))?;

        let branch = {
            let head = repo
                .head()
                .map_err(|e| AppError::Other(format!("git HEAD: {e}")))?;
            head.shorthand()
                .ok_or_else(|| AppError::Other("no current branch".into()))?
                .to_owned()
        };
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");

        let mut remote = repo
            .find_remote("origin")
            .map_err(|e| AppError::Other(format!("git remote 'origin': {e}")))?;
        let mut push_opts = git2::PushOptions::new();
        push_opts.remote_callbacks(ssh_callbacks());
        remote
            .push(&[refspec.as_str()], Some(&mut push_opts))
            .map_err(|e| AppError::Other(format!("git push: {e}")))?;

        Ok(())
    }

    fn head_hash(&self) -> Result<String> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| AppError::Other("git lock poisoned".into()))?;
        // Bind to a local so temporaries borrowing `repo` are dropped before `repo` is.
        let result: Result<String> = match repo.head() {
            Ok(head) => match head.peel_to_commit() {
                Ok(commit) => Ok(commit.id().to_string()),
                Err(e) => Err(AppError::Other(format!("peel HEAD: {e}"))),
            },
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok("unborn".into()),
            Err(e) => Err(AppError::Other(format!("git HEAD: {e}"))),
        };
        result
    }
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
}
