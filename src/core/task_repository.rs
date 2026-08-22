//! `TaskRepository` — an opened task repository: the store (TOML + SQLite
//! cache), the git backend, and the repo root, plus the mutation transactions
//! and the plugin event buffer.
//!
//! This is the core handle the `mcp` server, the `forgejo` plugin, and the CLI
//! all build on. The CLI wraps it in `AppContext` to add `config.toml` handling
//! (which is CLI-only); core/mcp/forgejo use `TaskRepository` directly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

use crate::core::{
    plugin::TaskEvent,
    progress::{NoProgress, ProgressSink},
    scoring::{ScoringConfig, TaskDates},
    Store, VcsBackend,
};

pub struct TaskRepository {
    pub store: Box<dyn Store>,
    pub vcs: Box<dyn VcsBackend>,
    /// Absolute path to the repository root (contains `.git`).
    pub repo_root: PathBuf,
    /// Urgency scoring weights for this repository. Defaults to
    /// [`ScoringConfig::default`]; the CLI overrides it from `config.toml` so
    /// every consumer (cli/mcp/forgejo) scores consistently.
    pub scoring: ScoringConfig,
    /// Task mutations performed this run, drained at the post-mutation
    /// chokepoint to notify subscribed plugins.
    task_events: Vec<TaskEvent>,
    /// The plugin that owns this process, from `NEXT_PLUGIN_ORIGIN`. Skipped
    /// when dispatching notifications so a plugin is never notified of its own
    /// changes (loop guard).
    plugin_origin: Option<String>,
    /// Where long operations report their progress.
    ///
    /// Carried here rather than passed down every signature because the slow
    /// code (the cache rebuild, the archive pass, a fetch) sits several layers
    /// below whatever knows a terminal is attached. Defaults to
    /// [`NoProgress`], so a consumer that installs nothing — `next-mcp`,
    /// `next-forgejo`, any library user — is silent by construction.
    progress: Arc<dyn ProgressSink>,
}

impl TaskRepository {
    /// Assembles a repository from already-opened parts, reading the
    /// `NEXT_PLUGIN_ORIGIN` loop-guard env var and the repo's committed scoring
    /// weights (`config/scoring.toml`). Callers that need a specific git backend
    /// (e.g. subprocess git) build it and pass it here.
    pub fn with_parts(store: Box<dyn Store>, vcs: Box<dyn VcsBackend>, repo_root: PathBuf) -> Self {
        Self {
            scoring: crate::core::storage::load_scoring(&repo_root),
            store,
            vcs,
            repo_root,
            task_events: Vec::new(),
            plugin_origin: std::env::var("NEXT_PLUGIN_ORIGIN")
                .ok()
                .filter(|s| !s.is_empty()),
            progress: Arc::new(NoProgress),
        }
    }

    /// Installs `sink` as the destination for progress reports, here and in the
    /// parts that do the slow work.
    ///
    /// A builder method because installing a renderer is a decision made once,
    /// at startup, by the binary that owns the terminal, and nothing else does
    /// it at all. The sink is pushed into the store and the VCS backend as well
    /// as kept here, so an operation reports from wherever it actually runs
    /// (the rebuild inside `CachedStore`, a fetch inside `GitBackend`) without
    /// either of them needing a path back to the repository.
    ///
    /// This is for a repository that is already open, and so covers everything
    /// *after* that point — a later reconcile, a fetch, the archive pass. The
    /// reconcile that opening itself performs is over by the time this can be
    /// called: pass the sink to
    /// [`open_with_progress`](Self::open_with_progress) to cover that one too.
    pub fn with_progress(mut self, sink: Arc<dyn ProgressSink>) -> Self {
        self.install_progress(sink);
        self
    }

    /// [`with_progress`](Self::with_progress) for a repository that is already
    /// owned by something else.
    ///
    /// A front-end that keeps its repository in a field (the CLI's
    /// `AppContext`, the TUI's loader) cannot use the consuming builder there
    /// without moving the field out and back — `mem::replace` gymnastics for
    /// what is a one-line assignment. Both forms install into the same three
    /// places: the store (a cache rebuild reports from there), the VCS backend
    /// (fetch and push), and here.
    pub fn install_progress(&mut self, sink: Arc<dyn ProgressSink>) {
        self.store.set_progress(sink.clone());
        self.vcs.set_progress(sink.clone());
        self.progress = sink;
    }

    /// The installed progress sink — [`NoProgress`] unless
    /// [`with_progress`](Self::with_progress) was called.
    pub fn progress(&self) -> &dyn ProgressSink {
        &*self.progress
    }

    /// An owned handle on the installed sink.
    ///
    /// [`transaction`](Self::transaction) hands its closure the store, the
    /// backend and the repo root — deliberately not the repository, so a
    /// mutation cannot start a nested one — which leaves an operation that
    /// wants to report from *inside* a transaction (the archive pass, a tag
    /// rename) unable to borrow [`progress`](Self::progress) across it. Cloning
    /// the `Arc` out first and moving it into the closure is the way round
    /// that; it costs one refcount bump per operation.
    pub fn progress_handle(&self) -> Arc<dyn ProgressSink> {
        Arc::clone(&self.progress)
    }

    /// Opens the task store and git backend at `repo_root`. No config-file
    /// handling — that is the CLI's concern.
    ///
    /// Silent: see [`open_with_progress`](Self::open_with_progress).
    pub fn open(repo_root: PathBuf) -> anyhow::Result<Self> {
        Self::open_with_progress(repo_root, Arc::new(NoProgress))
    }

    /// [`open`](Self::open) with `sink` already installed as the store opens.
    ///
    /// [`with_progress`](Self::with_progress) cannot cover this: the cache
    /// reconciles *while* it opens, so by the time there is a repository to
    /// install a sink on, the rebuild it would have reported is already done.
    pub fn open_with_progress(
        repo_root: PathBuf,
        sink: Arc<dyn ProgressSink>,
    ) -> anyhow::Result<Self> {
        let (store, vcs) =
            crate::core::storage::open_with_progress(repo_root.clone(), Arc::clone(&sink))?;
        Ok(Self::with_parts(Box::new(store), Box::new(vcs), repo_root)
            // The parts already report into `sink`; this is what makes the
            // repository's own handle (`progress_handle`, used by the
            // archiver and the tag rename) agree with them.
            .with_progress(sink))
    }

    /// Returns a shared reference to the task store.
    pub fn store(&self) -> &dyn Store {
        &*self.store
    }

    /// Returns an exclusive reference to the task store.
    pub fn store_mut(&mut self) -> &mut dyn Store {
        &mut *self.store
    }

    /// Records a task mutation for later plugin notification. Called by each
    /// mutating handler after its transaction returns (lock released).
    pub fn record_task_event(&mut self, verb: &'static str, task_id: Uuid) {
        self.task_events.push(TaskEvent::new(verb, task_id));
    }

    /// Drains the buffered task events (called at the post-mutation chokepoint).
    pub fn take_task_events(&mut self) -> Vec<TaskEvent> {
        std::mem::take(&mut self.task_events)
    }

    /// The plugin origin of this process, if any (loop guard).
    pub fn plugin_origin(&self) -> Option<&str> {
        self.plugin_origin.as_deref()
    }

    /// Returns git-derived creation/update timestamps for the given tasks.
    ///
    /// Served from the store's date index (maintained incrementally by the
    /// cache from git history) — no git walk per call. Returns an empty map
    /// when the store has no dates (fresh repo without commits).
    pub fn task_git_dates_for(
        &self,
        tasks: &[crate::core::domain::task::Task],
    ) -> HashMap<Uuid, TaskDates> {
        let mut all = self.store.task_dates().unwrap_or_default();
        tasks
            .iter()
            .filter_map(|t| all.remove(&t.id).map(|dates| (t.id, dates)))
            .collect()
    }

    /// Runs `f` as a repository mutation transaction.
    ///
    /// Holds the re-entrant repository lock for the entire closure so the
    /// read-modify-write-commit sequence cannot interleave with another process,
    /// reconciles the cache with the on-disk git HEAD before `f` runs, and
    /// records the new HEAD afterwards.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Store, &dyn VcsBackend, &Path) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _lock =
            crate::core::service::begin_mutation(&self.repo_root, &mut *self.store, &*self.vcs)?;
        let out = f(&mut *self.store, &*self.vcs, &self.repo_root)?;
        crate::core::service::end_mutation(&mut *self.store, &*self.vcs)?;
        Ok(out)
    }

    /// Runs `f` as a machine-local state mutation transaction, holding the
    /// exclusive state-file lock (separate from the repo lock) across the whole
    /// closure. No HEAD reconciliation — state is not committed to git.
    pub fn state_transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Store) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _lock = crate::core::storage::lock_state(&self.repo_root)?;
        f(&mut *self.store)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use uuid::Uuid;

    use super::*;
    use crate::core::domain::{state::GlobalState, tag::TagMeta, task::Task};
    use crate::core::error::Result as StoreResult;
    use crate::core::progress::testing::RecordingSink;
    use crate::core::store::PullResult;

    /// A store that answers nothing but reports the sink it was handed, by
    /// beginning a task named after itself. Observing the *label* proves the
    /// propagation reached this object with the caller's sink — a boolean flag
    /// would only prove that some sink arrived.
    struct SpyStore;

    impl Store for SpyStore {
        fn set_progress(&mut self, sink: Arc<dyn ProgressSink>) {
            drop(sink.begin("store", None));
        }
        fn get_task(&self, _id: Uuid) -> StoreResult<Task> {
            unimplemented!()
        }
        fn get_task_by_slug(&self, _slug: &str) -> StoreResult<Option<Task>> {
            unimplemented!()
        }
        fn find_tasks_by_prefix(&self, _prefix: &str) -> StoreResult<Vec<Task>> {
            unimplemented!()
        }
        fn list_tasks(&self) -> StoreResult<Vec<Task>> {
            unimplemented!()
        }
        fn save_task(&mut self, _task: &Task) -> StoreResult<()> {
            unimplemented!()
        }
        fn delete_task(&mut self, _id: Uuid) -> StoreResult<()> {
            unimplemented!()
        }
        fn get_state(&self) -> StoreResult<GlobalState> {
            unimplemented!()
        }
        fn save_state(&mut self, _state: &GlobalState) -> StoreResult<()> {
            unimplemented!()
        }
        fn get_tag_meta(&self, _tag: &str) -> StoreResult<Option<TagMeta>> {
            unimplemented!()
        }
        fn set_tag_meta(&mut self, _tag: &str, _meta: TagMeta) -> StoreResult<()> {
            unimplemented!()
        }
        fn delete_tag_meta(&mut self, _tag: &str) -> StoreResult<()> {
            unimplemented!()
        }
        fn list_tag_metas(&self) -> StoreResult<HashMap<String, TagMeta>> {
            unimplemented!()
        }
    }

    /// The `VcsBackend` counterpart of [`SpyStore`].
    struct SpyVcs;

    impl VcsBackend for SpyVcs {
        fn set_progress(&mut self, sink: Arc<dyn ProgressSink>) {
            drop(sink.begin("vcs", None));
        }
        fn commit(&self, _paths: &[PathBuf], _message: &str) -> StoreResult<()> {
            unimplemented!()
        }
        fn pull(&self) -> StoreResult<PullResult> {
            unimplemented!()
        }
        fn push(&self) -> StoreResult<()> {
            unimplemented!()
        }
        fn head_hash(&self) -> StoreResult<String> {
            unimplemented!()
        }
    }

    fn spy_repo() -> (tempfile::TempDir, TaskRepository) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let repo = TaskRepository::with_parts(
            Box::new(SpyStore),
            Box::new(SpyVcs),
            dir.path().to_path_buf(),
        );
        (dir, repo)
    }

    #[test]
    fn a_fresh_repository_carries_no_progress() {
        let (_dir, repo) = spy_repo();
        assert!(
            repo.progress().is_noop(),
            "installing nothing must mean reporting nothing"
        );
        // And the no-op sink is usable, not a trap.
        let task = repo.progress().begin("rebuild", Some(2));
        task.set_message("tasks/foo.toml");
        task.inc(2);
        task.finish(None);
    }

    #[test]
    fn with_progress_installs_the_sink_in_the_repository_store_and_vcs() {
        let (_dir, repo) = spy_repo();
        let sink = Arc::new(RecordingSink::new());
        let repo = repo.with_progress(sink.clone());

        drop(repo.progress().begin("repo", None));
        assert_eq!(
            sink.labels(),
            vec!["store".to_owned(), "vcs".to_owned(), "repo".to_owned()],
            "the sink must reach the store and the VCS backend, not just the handle"
        );
        assert!(!repo.progress().is_noop());
    }

    #[test]
    fn install_progress_reaches_the_same_three_places() {
        // The CLI's repository lives in a field, so it installs through the
        // non-consuming form; it must not be the weaker of the two.
        let (_dir, mut repo) = spy_repo();
        let sink = Arc::new(RecordingSink::new());
        repo.install_progress(sink.clone());

        drop(repo.progress().begin("repo", None));
        assert_eq!(
            sink.labels(),
            vec!["store".to_owned(), "vcs".to_owned(), "repo".to_owned()]
        );
        assert!(!repo.progress().is_noop());
    }
}
