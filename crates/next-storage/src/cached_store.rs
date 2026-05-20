use std::{path::PathBuf, sync::Mutex};

use next::{
    domain::{state::GlobalState, task::Task},
    error::{AppError, Result},
    store::Store,
};
use rusqlite::{params, Connection, OptionalExtension as _};
use uuid::Uuid;

use crate::TomlStore;

/// TOML-backed store with an SQLite read cache.
///
/// TOML files under `tasks/` are the source of truth.  SQLite lives at
/// `.next.db` in the repository root and is rebuilt automatically whenever
/// the git HEAD changes (e.g. after a pull).
///
/// All writes go to TOML first, then the cache is updated in-place, so reads
/// within the same session always see the latest data.  The cache is *not*
/// committed to git — add `.next.db` to your `.gitignore`.
pub struct CachedStore {
    inner: TomlStore,
    conn: Mutex<Connection>,
}

impl CachedStore {
    /// Opens (or creates) the SQLite cache at `db_path`.
    ///
    /// `head_hash` is the current git HEAD SHA or `"unborn"` for a fresh repo.
    /// If the stored hash differs from `head_hash` the cache is rebuilt from
    /// the TOML files before returning.
    pub fn open(inner: TomlStore, db_path: PathBuf, head_hash: &str) -> Result<Self> {
        let conn = Connection::open(&db_path)
            .map_err(|e| AppError::Other(format!("sqlite open {}: {e}", db_path.display())))?;
        setup_schema(&conn)?;
        let stored = get_meta(&conn, "head_hash")?;
        let this = Self {
            inner,
            conn: Mutex::new(conn),
        };
        if stored.as_deref() != Some(head_hash) {
            this.rebuild(head_hash)?;
        }
        Ok(this)
    }

    fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|_| AppError::Other("sqlite lock poisoned".into()))?;
        f(&conn)
    }

    fn rebuild(&self, head_hash: &str) -> Result<()> {
        let tasks = self.inner.list_tasks()?;
        let state = self.inner.get_state()?;
        self.with_conn(|conn| {
            conn.execute("DELETE FROM tasks", [])
                .map_err(|e| AppError::Other(format!("sqlite clear tasks: {e}")))?;
            for task in &tasks {
                upsert_task(conn, task)?;
            }
            let state_json = serde_json::to_string(&state)
                .map_err(|e| AppError::Other(format!("serialize state: {e}")))?;
            set_meta(conn, "state", &state_json)?;
            set_meta(conn, "head_hash", head_hash)?;
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// Store impl
// ---------------------------------------------------------------------------

impl Store for CachedStore {
    fn get_task(&self, id: Uuid) -> Result<Task> {
        let id_str = id.to_string();
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT data FROM tasks WHERE id = ?1",
                params![id_str],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Other(format!("sqlite get task: {e}")))?
            .ok_or_else(|| AppError::TaskNotFound(id_str.clone()))
            .and_then(deserialize_task)
        })
    }

    fn get_task_by_slug(&self, slug: &str) -> Result<Option<Task>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT data FROM tasks WHERE slug = ?1",
                params![slug],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Other(format!("sqlite get by slug: {e}")))?
            .map(deserialize_task)
            .transpose()
        })
    }

    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>> {
        let pattern = format!("{prefix}%");
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT data FROM tasks WHERE id LIKE ?1")
                .map_err(|e| AppError::Other(format!("sqlite prepare prefix: {e}")))?;
            let result = collect_task_rows(
                stmt.query_map(params![pattern], |row| row.get::<_, String>(0))
                    .map_err(|e| AppError::Other(format!("sqlite query prefix: {e}")))?,
            );
            result
        })
    }

    fn list_tasks(&self) -> Result<Vec<Task>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT data FROM tasks")
                .map_err(|e| AppError::Other(format!("sqlite prepare list: {e}")))?;
            let result = collect_task_rows(
                stmt.query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| AppError::Other(format!("sqlite query list: {e}")))?,
            );
            result
        })
    }

    fn save_task(&mut self, task: &Task) -> Result<()> {
        self.inner.save_task(task)?;
        self.with_conn(|conn| upsert_task(conn, task))
    }

    fn delete_task(&mut self, id: Uuid) -> Result<()> {
        self.inner.delete_task(id)?;
        let id_str = id.to_string();
        self.with_conn(|conn| {
            conn.execute("DELETE FROM tasks WHERE id = ?1", params![id_str])
                .map_err(|e| AppError::Other(format!("sqlite delete task: {e}")))?;
            Ok(())
        })
    }

    fn get_task_by_forgejo_issue(&self, url: &str) -> Result<Option<Task>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT data FROM tasks WHERE forgejo_issue = ?1",
                params![url],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Other(format!("sqlite get by forgejo: {e}")))?
            .map(deserialize_task)
            .transpose()
        })
    }

    fn get_task_by_webcal_uid(&self, uid: &str) -> Result<Option<Task>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT data FROM tasks WHERE webcal_uid = ?1",
                params![uid],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AppError::Other(format!("sqlite get by webcal: {e}")))?
            .map(deserialize_task)
            .transpose()
        })
    }

    fn get_state(&self) -> Result<GlobalState> {
        self.with_conn(|conn| match get_meta(conn, "state")? {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| AppError::Other(format!("deserialize state: {e}"))),
            None => Ok(GlobalState::default()),
        })
    }

    fn save_state(&mut self, state: &GlobalState) -> Result<()> {
        self.inner.save_state(state)?;
        let json = serde_json::to_string(state)
            .map_err(|e| AppError::Other(format!("serialize state: {e}")))?;
        self.with_conn(|conn| set_meta(conn, "state", &json))
    }
}

// ---------------------------------------------------------------------------
// Schema helpers
// ---------------------------------------------------------------------------

fn setup_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tasks (
            id            TEXT PRIMARY KEY,
            slug          TEXT,
            forgejo_issue TEXT,
            webcal_uid    TEXT,
            data          TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_tasks_slug    ON tasks(slug);
        CREATE INDEX IF NOT EXISTS idx_tasks_forgejo ON tasks(forgejo_issue);
        CREATE INDEX IF NOT EXISTS idx_tasks_webcal  ON tasks(webcal_uid);
        ",
    )
    .map_err(|e| AppError::Other(format!("sqlite schema setup: {e}")))
}

fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| AppError::Other(format!("sqlite meta get '{key}': {e}")))
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES(?1, ?2)",
        params![key, value],
    )
    .map_err(|e| AppError::Other(format!("sqlite meta set '{key}': {e}")))?;
    Ok(())
}

fn upsert_task(conn: &Connection, task: &Task) -> Result<()> {
    let data = serde_json::to_string(task)
        .map_err(|e| AppError::Other(format!("serialize task {}: {e}", task.id)))?;
    conn.execute(
        "INSERT OR REPLACE INTO tasks(id, slug, forgejo_issue, webcal_uid, data)
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![
            task.id.to_string(),
            task.slug.as_deref(),
            task.forgejo_issue.as_deref(),
            task.webcal_uid.as_deref(),
            data,
        ],
    )
    .map_err(|e| AppError::Other(format!("sqlite upsert task {}: {e}", task.id)))?;
    Ok(())
}

fn deserialize_task(data: String) -> Result<Task> {
    serde_json::from_str(&data).map_err(|e| AppError::Other(format!("deserialize task: {e}")))
}

fn collect_task_rows<'a>(
    rows: impl Iterator<Item = rusqlite::Result<String>> + 'a,
) -> Result<Vec<Task>> {
    let mut tasks = Vec::new();
    for row in rows {
        let data = row.map_err(|e| AppError::Other(format!("sqlite row: {e}")))?;
        tasks.push(deserialize_task(data)?);
    }
    Ok(tasks)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use next::{
        domain::task::{Priority, Status},
        store::VcsBackend as _,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::GitBackend;

    fn setup() -> (TempDir, CachedStore, GitBackend) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Test").unwrap();
            cfg.set_str("user.email", "test@test.com").unwrap();
        }
        let inner = TomlStore::open(dir.path().to_path_buf()).unwrap();
        let vcs = GitBackend::open(dir.path()).unwrap();
        let head_hash = vcs.head_hash().unwrap();
        let db_path = dir.path().join(".next.db");
        let store = CachedStore::open(inner, db_path, &head_hash).unwrap();
        (dir, store, vcs)
    }

    #[test]
    fn save_and_retrieve_task() {
        let (_dir, mut store, _vcs) = setup();
        let task = Task::new("Buy milk");
        store.save_task(&task).unwrap();

        let loaded = store.get_task(task.id).unwrap();
        assert_eq!(loaded.title, "Buy milk");
        assert_eq!(loaded.id, task.id);
    }

    #[test]
    fn list_tasks_returns_saved() {
        let (_dir, mut store, _vcs) = setup();
        store.save_task(&Task::new("One")).unwrap();
        store.save_task(&Task::new("Two")).unwrap();
        store.save_task(&Task::new("Three")).unwrap();

        let all = store.list_tasks().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn get_task_not_found() {
        let (_dir, store, _vcs) = setup();
        let err = store.get_task(Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, AppError::TaskNotFound(_)));
    }

    #[test]
    fn get_by_slug() {
        let (_dir, mut store, _vcs) = setup();
        let mut task = Task::new("Water plants");
        task.slug = Some("water-plants".into());
        store.save_task(&task).unwrap();

        let found = store.get_task_by_slug("water-plants").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, task.id);

        let miss = store.get_task_by_slug("nope").unwrap();
        assert!(miss.is_none());
    }

    #[test]
    fn find_by_prefix() {
        let (_dir, mut store, _vcs) = setup();
        let task = Task::new("Prefix test");
        store.save_task(&task).unwrap();

        let prefix = &task.id.to_string()[..8];
        let found = store.find_tasks_by_prefix(prefix).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, task.id);
    }

    #[test]
    fn find_by_prefix_no_match() {
        let (_dir, mut store, _vcs) = setup();
        store.save_task(&Task::new("Irrelevant")).unwrap();
        let found = store.find_tasks_by_prefix("00000000-0000").unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn delete_task() {
        let (_dir, mut store, _vcs) = setup();
        let task = Task::new("Temporary");
        store.save_task(&task).unwrap();
        assert!(store.get_task(task.id).is_ok());

        store.delete_task(task.id).unwrap();
        assert!(matches!(
            store.get_task(task.id).unwrap_err(),
            AppError::TaskNotFound(_)
        ));
    }

    #[test]
    fn delete_also_removes_from_sqlite() {
        let (_dir, mut store, _vcs) = setup();
        let task = Task::new("Ephemeral");
        store.save_task(&task).unwrap();
        store.delete_task(task.id).unwrap();

        // Confirm via list (SQLite path).
        assert!(store.list_tasks().unwrap().is_empty());
    }

    #[test]
    fn state_round_trip() {
        let (_dir, mut store, _vcs) = setup();
        let default = store.get_state().unwrap();
        assert!(default.active_contexts.is_empty());

        let mut state = GlobalState::default();
        state.active_contexts = vec!["@work".into()];
        store.save_state(&state).unwrap();

        let loaded = store.get_state().unwrap();
        assert_eq!(loaded.active_contexts, vec!["@work"]);
    }

    #[test]
    fn update_task_reflects_in_cache() {
        let (_dir, mut store, _vcs) = setup();
        let mut task = Task::new("Original");
        store.save_task(&task).unwrap();

        task.title = "Updated".into();
        task.priority = Priority::High;
        store.save_task(&task).unwrap();

        let loaded = store.get_task(task.id).unwrap();
        assert_eq!(loaded.title, "Updated");
        assert_eq!(loaded.priority, Priority::High);
        assert_eq!(store.list_tasks().unwrap().len(), 1);
    }

    #[test]
    fn done_status_persisted() {
        let (_dir, mut store, _vcs) = setup();
        let mut task = Task::new("Finish me");
        store.save_task(&task).unwrap();

        task.mark_done();
        store.save_task(&task).unwrap();

        let loaded = store.get_task(task.id).unwrap();
        assert_eq!(loaded.status, Status::Done);
    }

    #[test]
    fn rebuild_on_head_change() {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Test").unwrap();
            cfg.set_str("user.email", "test@test.com").unwrap();
        }
        let vcs = GitBackend::open(dir.path()).unwrap();

        // Session 1: add a task and commit it.
        {
            let inner = TomlStore::open(dir.path().to_path_buf()).unwrap();
            let head = vcs.head_hash().unwrap();
            let db_path = dir.path().join(".next.db");
            let mut store = CachedStore::open(inner, db_path, &head).unwrap();

            let task = Task::new("Persisted");
            store.save_task(&task).unwrap();

            // Commit the TOML file to advance HEAD.
            let task_path = crate::task_path(dir.path(), &task);
            vcs.commit(&[task_path], "add task").unwrap();
        }

        // Session 2: open with the new HEAD — cache must be rebuilt.
        {
            let inner = TomlStore::open(dir.path().to_path_buf()).unwrap();
            let head = vcs.head_hash().unwrap();
            let db_path = dir.path().join(".next.db");
            let store = CachedStore::open(inner, db_path, &head).unwrap();

            let all = store.list_tasks().unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].title, "Persisted");
        }
    }

    #[test]
    fn get_by_forgejo_issue() {
        let (_dir, mut store, _vcs) = setup();
        let mut task = Task::new("Linked task");
        task.forgejo_issue = Some("https://forgejo.example.com/org/repo/issues/42".into());
        store.save_task(&task).unwrap();

        let found = store
            .get_task_by_forgejo_issue("https://forgejo.example.com/org/repo/issues/42")
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, task.id);

        let miss = store
            .get_task_by_forgejo_issue("https://forgejo.example.com/other/99")
            .unwrap();
        assert!(miss.is_none());
    }

    #[test]
    fn stale_toml_file_triggers_rebuild() {
        // Write a TOML task directly (bypassing the store) then open with a
        // mismatched head_hash to force a rebuild that picks it up.
        let dir = TempDir::new().unwrap();
        git2::Repository::init(dir.path()).unwrap();

        let tasks_dir = dir.path().join("tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let mut task = Task::new("External task");
        task.slug = Some("external".into());
        let toml = toml::to_string_pretty(&task).unwrap();
        fs::write(tasks_dir.join("external.toml"), toml).unwrap();

        // Open with a head_hash that doesn't match stored (empty DB → will rebuild).
        let inner = TomlStore::open(dir.path().to_path_buf()).unwrap();
        let db_path = dir.path().join(".next.db");
        let store = CachedStore::open(inner, db_path, "fake-head").unwrap();

        let found = store.get_task_by_slug("external").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, "External task");
    }
}
