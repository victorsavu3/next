//! `TaskRepository` — an opened task repository: the store (TOML + SQLite
//! cache), the git backend, and the repo root, plus the mutation transactions
//! and the plugin event buffer.
//!
//! This is the core handle the `mcp` server, the `forgejo` plugin, and the CLI
//! all build on. The CLI wraps it in `AppContext` to add `config.toml` handling
//! (which is CLI-only); core/mcp/forgejo use `TaskRepository` directly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::core::{plugin::TaskEvent, scoring::{ScoringConfig, TaskDates}, Store, VcsBackend};

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
            plugin_origin: std::env::var("NEXT_PLUGIN_ORIGIN").ok().filter(|s| !s.is_empty()),
        }
    }

    /// Opens the task store and git backend at `repo_root`. No config-file
    /// handling — that is the CLI's concern.
    pub fn open(repo_root: PathBuf) -> anyhow::Result<Self> {
        let (store, vcs) = crate::core::storage::open(repo_root.clone())?;
        Ok(Self::with_parts(Box::new(store), Box::new(vcs), repo_root))
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
    pub fn task_git_dates_for(&self, tasks: &[crate::core::domain::task::Task]) -> HashMap<Uuid, TaskDates> {
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
