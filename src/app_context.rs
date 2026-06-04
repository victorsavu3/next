use std::path::{Path, PathBuf};

use anyhow::Context as _;
use uuid::Uuid;
use crate::{plugin::TaskEvent, Config, Store, VcsBackend};

pub struct AppContext {
    pub config: Config,
    pub store: Box<dyn Store>,
    pub vcs: Box<dyn VcsBackend>,
    /// Absolute path to the repository root (contains `.git` and `state.toml`).
    pub repo_root: PathBuf,
    /// Task mutations performed this run, drained at the post-mutation
    /// chokepoint to notify subscribed plugins.
    task_events: Vec<TaskEvent>,
    /// The plugin that owns this process, from `NEXT_PLUGIN_ORIGIN`.  Skipped
    /// when dispatching notifications so a plugin is never notified of its own
    /// changes (loop guard).
    plugin_origin: Option<String>,
}

impl AppContext {
    /// Assembles a context from its parts, reading the `NEXT_PLUGIN_ORIGIN`
    /// loop-guard env var.  Used by [`AppContext::new`] and by tests that wire
    /// up a store directly.
    pub fn with_parts(
        config: Config,
        store: Box<dyn Store>,
        vcs: Box<dyn VcsBackend>,
        repo_root: PathBuf,
    ) -> Self {
        Self {
            config,
            store,
            vcs,
            repo_root,
            task_events: Vec::new(),
            plugin_origin: std::env::var("NEXT_PLUGIN_ORIGIN").ok().filter(|s| !s.is_empty()),
        }
    }

    /// Records a task mutation for later plugin notification.  Called by each
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

    /// Returns a shared reference to the task store.
    pub fn store(&self) -> &dyn Store {
        &*self.store
    }

    /// Returns an exclusive reference to the task store.
    pub fn store_mut(&mut self) -> &mut dyn Store {
        &mut *self.store
    }

    /// Runs `f` as a repository mutation transaction.
    ///
    /// Holds the re-entrant repository lock for the entire closure so the
    /// read-modify-write-commit sequence cannot interleave with another
    /// process, reconciles the cache with the on-disk git HEAD before `f` runs
    /// (so reads see other processes' commits), and records the new HEAD
    /// afterwards.  `f` receives the store, the VCS backend, and the repo root,
    /// and is responsible for performing the read, mutation, save, and commit.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Store, &dyn VcsBackend, &Path) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _lock =
            crate::domain::service::begin_mutation(&self.repo_root, &mut *self.store, &*self.vcs)?;
        let out = f(&mut *self.store, &*self.vcs, &self.repo_root)?;
        crate::domain::service::end_mutation(&mut *self.store, &*self.vcs)?;
        Ok(out)
    }

    /// Runs `f` as a machine-local state mutation transaction.
    ///
    /// Holds the exclusive state-file lock (`.state.toml.lock`) — *separate* from
    /// the repository lock — across the entire closure, so a `get_state` →
    /// modify → `save_state` sequence cannot interleave with another process and
    /// lose updates.  State is not committed to git, so unlike [`transaction`]
    /// there is no HEAD reconciliation.
    pub fn state_transaction<T>(
        &mut self,
        f: impl FnOnce(&mut dyn Store) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _lock = crate::storage::lock_state(&self.repo_root)?;
        f(&mut *self.store)
    }

    /// Construct an application context.
    ///
    /// * `config_path` — use this config file instead of the XDG default.
    /// * `repo` — use this repository root instead of the config value or the
    ///   upward directory search.
    pub fn new(config_path: Option<&Path>, repo: Option<&Path>) -> anyhow::Result<Self> {
        let config = load_config(config_path);

        let root = if let Some(p) = repo {
            p.to_path_buf()
        } else if let Some(ref p) = config.repository {
            p.clone()
        } else {
            find_repo_root().context(
                "not inside a task repository — run `next init` to set one up, \
                 or set `repository` in the config file",
            )?
        };
        let (s, v) = crate::storage::open(root.clone())
            .context("failed to open local task store")?;
        let v = v.with_subprocess(config.sync.git_subprocess);
        let store: Box<dyn Store> = Box::new(s);
        let vcs: Box<dyn VcsBackend> = Box::new(v);

        Ok(Self::with_parts(config, store, vcs, root))
    }
}

/// Walks up from the current directory until a `.git` directory is found.
fn find_repo_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Loads configuration from the given path, or from
/// `$XDG_CONFIG_HOME/task-manager/config.toml` when `override_path` is `None`.
/// Returns `Config::default()` when the file is absent or unreadable.
fn load_config(override_path: Option<&Path>) -> Config {
    let path = if let Some(p) = override_path {
        Some(p.to_path_buf())
    } else {
        dirs::config_dir().map(|d| d.join("task-manager").join("config.toml"))
    };

    let Some(path) = path else {
        return Config::default();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Config::default();
    };

    match toml::from_str::<Config>(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("warning: failed to parse config file {}: {e}", path.display());
            Config::default()
        }
    }
}
