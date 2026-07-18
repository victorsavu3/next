use std::{collections::HashMap, path::PathBuf, sync::Mutex};

use crate::core::{
    domain::{state::GlobalState, tag::TagMeta, task::Task},
    error::{TaskError, Result},
    scoring::TaskDates,
    store::{Page, Store, TaskLocation, TaskQuery, DEFAULT_PAGE_SIZE},
};
use rusqlite::{params, Connection, OptionalExtension as _};
use uuid::Uuid;

use super::git_backend::FileChange;
use super::TomlStore;

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
    /// Repository root, used to resolve repo-relative paths from git diffs.
    root: PathBuf,
    conn: Mutex<Connection>,
}

impl CachedStore {
    /// Opens (or creates) the SQLite cache at `db_path`.
    ///
    /// `head_hash` is the current git HEAD SHA or `"unborn"` for a fresh repo.
    /// If the stored hash differs from `head_hash` the cache is reconciled
    /// (incrementally when possible) with the TOML files before returning.
    pub fn open(inner: TomlStore, db_path: PathBuf, head_hash: &str) -> Result<Self> {
        let conn = Connection::open(&db_path)
            .map_err(|e| TaskError::Other(format!("sqlite open {}: {e}", db_path.display())))?;
        configure_connection(&conn)?;
        setup_schema(&conn)?;
        heal_on_version_change(&conn)?;
        let root = inner.root().to_path_buf();
        let this = Self {
            inner,
            root,
            conn: Mutex::new(conn),
        };
        this.reconcile(head_hash)?;
        Ok(this)
    }

    fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|_| TaskError::Other("sqlite lock poisoned".into()))?;
        f(&conn)
    }

    fn rebuild(&self, head_hash: &str) -> Result<()> {
        let tasks = self.inner.list_tasks_with_paths()?;
        let state = self.inner.get_state()?;
        // One git-history walk to backfill the date columns; empty when the
        // repo has no history (fresh init, tests).
        let backfill =
            super::git_backend::task_git_dates(&self.root.join("tasks")).unwrap_or_default();
        // Archive segments are self-contained: entries carry frozen dates,
        // so they never touch git history.
        let segments = super::archive::segment_paths(&self.root)?;
        self.with_conn(|conn| {
            conn.execute("DELETE FROM tasks", [])
                .map_err(|e| TaskError::Other(format!("sqlite clear tasks: {e}")))?;
            conn.execute("DELETE FROM task_tags", [])
                .map_err(|e| TaskError::Other(format!("sqlite clear task_tags: {e}")))?;
            for (path, task) in &tasks {
                // Prefer the rename-stable UUID suffix; fall back to the
                // exact filename (covers slug-named files).
                let hex = task.id.simple().to_string();
                let filename = path.strip_prefix("tasks/").unwrap_or(path);
                let dates = backfill.get(&hex[..8]).or_else(|| backfill.get(filename));
                upsert_task_row(conn, task, path, dates, 0)?;
            }
            let state_json = serde_json::to_string(&state)
                .map_err(|e| TaskError::Other(format!("serialize state: {e}")))?;
            set_meta(conn, "state", &state_json)?;
            set_meta(conn, "head_hash", head_hash)?;
            Ok(())
        })?;
        for (rel, abs) in &segments {
            let entries = super::archive::read_segment(abs)?;
            self.with_conn(|conn| upsert_segment_rows(conn, rel, &entries, 1))?;
        }
        // Cold tier: manifest-listed segments no longer in the checkout are
        // recovered from their blobs — one object read each.
        let in_checkout: std::collections::HashSet<&str> =
            segments.iter().map(|(rel, _)| rel.as_str()).collect();
        for pruned in super::archive::read_manifest(&self.root)? {
            if in_checkout.contains(pruned.path.as_str()) {
                continue;
            }
            let Some(content) = super::git_backend::blob_content(&self.root, &pruned.blob) else {
                tracing::warn!(
                    "pruned segment {} (blob {}) is unreachable; its tasks are missing from the cache",
                    pruned.path,
                    pruned.blob
                );
                continue;
            };
            let entries = super::archive::parse_segment(&content)?;
            self.with_conn(|conn| upsert_segment_rows(conn, &pruned.path, &entries, 2))?;
        }
        Ok(())
    }

    /// Repo-relative path (`tasks/<filename>`) where `task` is stored on disk.
    fn rel_task_path(task: &Task) -> String {
        format!("tasks/{}", crate::core::storage::filenames::generate_filename(task))
    }

    /// Brings the cache in line with `new_head`.
    ///
    /// No-op when the stored head already matches. Otherwise tries an
    /// incremental update from the git tree diff — cost proportional to the
    /// number of changed files — and falls back to a full rebuild when the
    /// diff cannot be computed (no stored head, an unborn branch, a stored
    /// head that no longer resolves).
    fn reconcile(&self, new_head: &str) -> Result<()> {
        let stored = self.with_conn(|conn| get_meta(conn, "head_hash"))?;
        if stored.as_deref() == Some(new_head) {
            return Ok(());
        }
        if let Some(stored) = stored {
            if let Some(changes) = super::git_backend::changed_paths(&self.root, &stored, new_head) {
                return self.apply_changes(&changes, new_head);
            }
        }
        self.rebuild(new_head)
    }

    /// Applies an incremental set of file changes to the cache.
    fn apply_changes(&self, changes: &[FileChange], new_head: &str) -> Result<()> {
        // The exact change time of each file lies somewhere in the diffed
        // commit range; the new head's time is the closest cheap bound.
        let head_time = super::git_backend::commit_time(&self.root, new_head);
        let head_dates = head_time.map(|t| TaskDates { created_at: t, updated_at: t });

        // Deletes first, so a rename (delete + add of the same task under a
        // new filename) nets out to the surviving row. Stash the deleted
        // rows' creation times so a rename does not reset task age. A deleted
        // segment drops every row it carried.
        let mut stashed_created: HashMap<String, String> = HashMap::new();
        self.with_conn(|conn| {
            for change in changes {
                let FileChange::Delete(path) = change else { continue };
                if is_task_file(path) {
                    if let Some((id, Some(created))) = delete_by_path(conn, path)? {
                        stashed_created.insert(id, created);
                    }
                } else if super::archive::is_segment_path(path) {
                    while delete_by_path(conn, path)?.is_some() {}
                }
            }
            Ok(())
        })?;
        for change in changes {
            let FileChange::Upsert(path) = change else { continue };
            if is_task_file(path) {
                let abs = self.root.join(path);
                match std::fs::read_to_string(&abs) {
                    Ok(content) => {
                        let task = toml::from_str::<Task>(&content).map_err(|e| {
                            TaskError::Other(format!("parse error in {}: {e}", abs.display()))
                        })?;
                        let restored = stashed_created.get(&task.id.to_string());
                        self.with_conn(|conn| {
                            upsert_task_row(conn, &task, path, head_dates.as_ref(), 0)?;
                            if let Some(created) = restored {
                                conn.execute(
                                    "UPDATE tasks SET created_at = ?2 WHERE id = ?1",
                                    params![task.id.to_string(), created],
                                )
                                .map_err(|e| TaskError::Other(format!("sqlite restore created_at: {e}")))?;
                            }
                            Ok(())
                        })?;
                    }
                    // Vanished between diff and read (e.g. a concurrent writer) —
                    // drop any stale row for that path.
                    Err(_) => self.with_conn(|conn| delete_by_path(conn, path).map(|_| ()))?,
                }
            } else if super::archive::is_segment_path(path) {
                // A changed segment replaces all rows it previously carried.
                let entries = super::archive::read_segment(&self.root.join(path))?;
                self.with_conn(|conn| upsert_segment_rows(conn, path, &entries, 1))?;
            } else if path == super::archive::MANIFEST_REL_PATH {
                // An external prune arrived (segment delete + manifest
                // append in one commit; deletes ran first above). Re-add
                // rows for manifest paths absent from the checkout.
                for pruned in super::archive::read_manifest(&self.root)? {
                    if self.root.join(&pruned.path).exists() {
                        continue;
                    }
                    let Some(content) =
                        super::git_backend::blob_content(&self.root, &pruned.blob)
                    else {
                        continue;
                    };
                    let entries = super::archive::parse_segment(&content)?;
                    self.with_conn(|conn| upsert_segment_rows(conn, &pruned.path, &entries, 2))?;
                }
            }
        }
        self.with_conn(|conn| set_meta(conn, "head_hash", new_head))
    }
}

/// Replaces every cached row stored at segment `rel_path` with `entries`
/// in the given archive tier (1 = warm checkout segment, 2 = cold pruned
/// segment); frozen dates are carried by the entries themselves.
fn upsert_segment_rows(
    conn: &Connection,
    rel_path: &str,
    entries: &[super::archive::ArchivedTask],
    tier: i64,
) -> Result<()> {
    while delete_by_path(conn, rel_path)?.is_some() {}
    for entry in entries {
        let dates = match (entry.created_at, entry.updated_at) {
            (Some(c), Some(u)) => Some(TaskDates { created_at: c, updated_at: u }),
            _ => None,
        };
        upsert_task_row(conn, &entry.task, rel_path, dates.as_ref(), tier)?;
    }
    Ok(())
}

/// Whether a repo-relative path is an individual task file.
fn is_task_file(path: &str) -> bool {
    path.starts_with("tasks/") && path.ends_with(".toml")
}

/// Parses an RFC 3339 timestamp column value into UTC.
fn parse_rfc3339(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Removes the task row (and its tag rows) stored at `path`, if any.
/// Returns the removed row's id and creation time so a rename (delete + add
/// of the same task) can carry the creation time over.
fn delete_by_path(conn: &Connection, path: &str) -> Result<Option<(String, Option<String>)>> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT id, created_at FROM tasks WHERE path = ?1",
            params![path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| TaskError::Other(format!("sqlite lookup by path: {e}")))?;
    if let Some((id, _)) = &row {
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])
            .map_err(|e| TaskError::Other(format!("sqlite delete by path: {e}")))?;
        conn.execute("DELETE FROM task_tags WHERE task_id = ?1", params![id])
            .map_err(|e| TaskError::Other(format!("sqlite delete tags by path: {e}")))?;
    }
    Ok(row)
}

// ---------------------------------------------------------------------------
// Store impl
// ---------------------------------------------------------------------------

impl Store for CachedStore {
    fn rebuild_cache(&self, head_hash: &str) -> Result<()> {
        self.rebuild(head_hash)
    }

    fn get_task(&self, id: Uuid) -> Result<Task> {
        let id_str = id.to_string();
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT data FROM tasks WHERE id = ?1",
                params![id_str],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| TaskError::Other(format!("sqlite get task: {e}")))?
            .ok_or_else(|| TaskError::TaskNotFound(id_str.clone()))
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
            .map_err(|e| TaskError::Other(format!("sqlite get by slug: {e}")))?
            .map(deserialize_task)
            .transpose()
        })
    }

    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>> {
        let pattern = format!("{prefix}%");
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT data FROM tasks WHERE id LIKE ?1")
                .map_err(|e| TaskError::Other(format!("sqlite prepare prefix: {e}")))?;
            let result = collect_task_rows(
                stmt.query_map(params![pattern], |row| row.get::<_, String>(0))
                    .map_err(|e| TaskError::Other(format!("sqlite query prefix: {e}")))?,
            );
            result
        })
    }

    fn list_tasks(&self) -> Result<Vec<Task>> {
        // Active tiers only: archived tasks are reached through query_tasks
        // (archived: true) or the tier-transparent id/slug/prefix lookups.
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT data FROM tasks WHERE archived = 0")
                .map_err(|e| TaskError::Other(format!("sqlite prepare list: {e}")))?;
            let result = collect_task_rows(
                stmt.query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| TaskError::Other(format!("sqlite query list: {e}")))?,
            );
            result
        })
    }

    fn save_task(&mut self, task: &Task) -> Result<()> {
        // Hold the (re-entrant) repo lock across the index lookups and the
        // file write, so the slug check cannot interleave with another
        // process's write.
        let _lock = self.inner.acquire_repo_lock()?;

        // Writing an individual file for a task whose bytes live in a
        // segment would duplicate it; mutations must resurrect first
        // (archiver::resurrect_if_archived), which flips the row before
        // saving through resurrect_task. A task the cache does not know yet
        // is simply new.
        if let Ok(TaskLocation::Archived(seg)) = self.task_location(task.id) {
            return Err(TaskError::Other(format!(
                "task {} is archived (in {seg}); resurrect it before editing",
                task.id
            )));
        }

        let id_str = task.id.to_string();
        if let Some(ref slug) = task.slug {
            let taken: Option<String> = self.with_conn(|conn| {
                conn.query_row(
                    "SELECT id FROM tasks WHERE slug = ?1 AND id <> ?2",
                    params![slug, id_str],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| TaskError::Other(format!("sqlite slug check: {e}")))
            })?;
            if taken.is_some() {
                return Err(TaskError::SlugConflict(slug.clone()));
            }
        }

        let old_path: Option<String> = self.with_conn(|conn| {
            conn.query_row("SELECT path FROM tasks WHERE id = ?1", params![id_str], |r| r.get(0))
                .optional()
                .map_err(|e| TaskError::Other(format!("sqlite path lookup: {e}")))
        })?;

        self.inner.save_task_at(task, old_path.as_deref())?;
        // A local write is about to be committed, so "now" matches the commit
        // author time; on an existing row only updated_at advances.
        let now = chrono::Utc::now();
        let dates = TaskDates { created_at: now, updated_at: now };
        self.with_conn(|conn| upsert_task_row(conn, task, &Self::rel_task_path(task), Some(&dates), 0))
    }

    fn delete_task(&mut self, id: Uuid) -> Result<()> {
        let _lock = self.inner.acquire_repo_lock()?;
        // The path hint below points at the whole segment for an archived
        // task — deleting it would take every other entry along. (A task
        // unknown to the cache falls through to the scan-based delete.)
        if let Ok(TaskLocation::Archived(seg)) = self.task_location(id) {
            return Err(TaskError::Other(format!(
                "task {id} is archived (in {seg}); resurrect it before deleting"
            )));
        }
        let id_str = id.to_string();
        let path: Option<String> = self.with_conn(|conn| {
            conn.query_row("SELECT path FROM tasks WHERE id = ?1", params![id_str], |r| r.get(0))
                .optional()
                .map_err(|e| TaskError::Other(format!("sqlite path lookup: {e}")))
        })?;
        match path {
            Some(rel) => self.inner.delete_task_at(id, &rel)?,
            // Not in the cache — fall back to the scan (errors when absent
            // on disk too, preserving TaskNotFound semantics).
            None => self.inner.delete_task(id)?,
        }
        self.with_conn(|conn| {
            conn.execute("DELETE FROM tasks WHERE id = ?1", params![id_str])
                .map_err(|e| TaskError::Other(format!("sqlite delete task: {e}")))?;
            conn.execute("DELETE FROM task_tags WHERE task_id = ?1", params![id_str])
                .map_err(|e| TaskError::Other(format!("sqlite delete task tags: {e}")))?;
            Ok(())
        })
    }

    fn get_state(&self) -> Result<GlobalState> {
        self.with_conn(|conn| match get_meta(conn, "state")? {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| TaskError::Other(format!("deserialize state: {e}"))),
            None => Ok(GlobalState::default()),
        })
    }

    fn save_state(&mut self, state: &GlobalState) -> Result<()> {
        self.inner.save_state(state)?;
        let json = serde_json::to_string(state)
            .map_err(|e| TaskError::Other(format!("serialize state: {e}")))?;
        self.with_conn(|conn| set_meta(conn, "state", &json))
    }

    fn get_tag_meta(&self, tag: &str) -> Result<Option<TagMeta>> {
        self.inner.get_tag_meta(tag)
    }

    fn set_tag_meta(&mut self, tag: &str, meta: TagMeta) -> Result<()> {
        self.inner.set_tag_meta(tag, meta)
    }

    fn delete_tag_meta(&mut self, tag: &str) -> Result<()> {
        self.inner.delete_tag_meta(tag)
    }

    fn list_tag_metas(&self) -> Result<HashMap<String, TagMeta>> {
        self.inner.list_tag_metas()
    }

    fn after_pull(&mut self, new_head: &str) -> Result<()> {
        self.reconcile(new_head)
    }

    fn get_tasks(&self, ids: &[Uuid]) -> Result<Vec<Task>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; ids.len()].join(", ");
        let id_strs: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!("SELECT data FROM tasks WHERE id IN ({marks})"))
                .map_err(|e| TaskError::Other(format!("sqlite prepare get_tasks: {e}")))?;
            let result = collect_task_rows(
                stmt.query_map(rusqlite::params_from_iter(id_strs.iter()), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| TaskError::Other(format!("sqlite get_tasks: {e}")))?,
            );
            result
        })
    }

    fn query_tasks(&self, q: &TaskQuery) -> Result<Page<Task>> {
        let page_size = if q.page_size == 0 { DEFAULT_PAGE_SIZE } else { q.page_size };
        let page = q.page.max(1);

        let mut where_sql =
            String::from(if q.archived { "archived <> 0" } else { "archived = 0" });
        let mut args: Vec<String> = Vec::new();

        if let Some(statuses) = &q.statuses {
            if statuses.is_empty() {
                return Ok(Page { items: Vec::new(), page, page_size, total: 0 });
            }
            let marks = vec!["?"; statuses.len()].join(", ");
            where_sql.push_str(&format!(" AND status IN ({marks})"));
            args.extend(statuses.iter().map(|s| status_name(s).to_owned()));
        }
        if let Some(pid) = q.parent_id {
            where_sql.push_str(" AND parent_id = ?");
            args.push(pid.to_string());
        }
        // Hierarchical tag match: exact tag or any descendant (`tag/...`).
        const TAG_MATCH: &str = "(SELECT 1 FROM task_tags tt \
             WHERE tt.task_id = tasks.id AND (tt.tag = ? OR tt.tag LIKE ? || '/%'))";
        for tag in &q.required_tags {
            where_sql.push_str(&format!(" AND EXISTS {TAG_MATCH}"));
            args.push(tag.clone());
            args.push(tag.clone());
        }
        for tag in &q.excluded_tags {
            where_sql.push_str(&format!(" AND NOT EXISTS {TAG_MATCH}"));
            args.push(tag.clone());
            args.push(tag.clone());
        }

        self.with_conn(|conn| {
            let total = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM tasks WHERE {where_sql}"),
                    rusqlite::params_from_iter(args.iter()),
                    |r| r.get::<_, i64>(0),
                )
                .map_err(|e| TaskError::Other(format!("sqlite query count: {e}")))?
                as u64;

            // Same order as store::sort_for_query: unresolved first (by id),
            // then most recent completion; ISO dates sort lexicographically.
            let offset = (page as u64 - 1) * page_size as u64;
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT data FROM tasks WHERE {where_sql} \
                     ORDER BY (completed_at IS NOT NULL), completed_at DESC, id \
                     LIMIT {page_size} OFFSET {offset}"
                ))
                .map_err(|e| TaskError::Other(format!("sqlite prepare query: {e}")))?;
            let items = collect_task_rows(
                stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| TaskError::Other(format!("sqlite query tasks: {e}")))?,
            )?;
            Ok(Page { items, page, page_size, total })
        })
    }

    fn note_archived_segment(
        &mut self,
        rel_path: &str,
        entries: &[super::archive::ArchivedTask],
    ) -> Result<()> {
        self.with_conn(|conn| upsert_segment_rows(conn, rel_path, entries, 1))
    }

    fn note_cold_segment(&mut self, rel_path: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE tasks SET archived = 2 WHERE path = ?1",
                params![rel_path],
            )
            .map_err(|e| TaskError::Other(format!("sqlite mark cold: {e}")))?;
            Ok(())
        })
    }

    fn task_location(&self, id: Uuid) -> Result<TaskLocation> {
        let id_str = id.to_string();
        let row: Option<(i64, String)> = self.with_conn(|conn| {
            conn.query_row(
                "SELECT archived, path FROM tasks WHERE id = ?1",
                params![id_str],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| TaskError::Other(format!("sqlite task_location: {e}")))
        })?;
        match row {
            Some((0, _)) => Ok(TaskLocation::Active),
            Some((_, path)) => Ok(TaskLocation::Archived(path)),
            None => Err(TaskError::TaskNotFound(id_str)),
        }
    }

    fn resurrect_task(&mut self, task: &Task) -> Result<()> {
        let _lock = self.inner.acquire_repo_lock()?;
        // Like save_task, but expects (and flips) an archived row: the
        // ON CONFLICT upsert moves it to the active tier and the tasks/ path
        // while preserving the frozen created_at; updated_at advances to now.
        self.inner.save_task_at(task, None)?;
        let now = chrono::Utc::now();
        let dates = TaskDates { created_at: now, updated_at: now };
        self.with_conn(|conn| {
            upsert_task_row(conn, task, &Self::rel_task_path(task), Some(&dates), 0)
        })
    }

    fn task_dates(&self) -> Result<HashMap<Uuid, TaskDates>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, created_at, updated_at FROM tasks
                     WHERE created_at IS NOT NULL AND updated_at IS NOT NULL",
                )
                .map_err(|e| TaskError::Other(format!("sqlite prepare dates: {e}")))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| TaskError::Other(format!("sqlite query dates: {e}")))?;
            let mut dates = HashMap::new();
            for row in rows {
                let (id, created, updated) =
                    row.map_err(|e| TaskError::Other(format!("sqlite dates row: {e}")))?;
                let (Ok(id), Some(created), Some(updated)) =
                    (Uuid::parse_str(&id), parse_rfc3339(&created), parse_rfc3339(&updated))
                else {
                    continue;
                };
                dates.insert(id, TaskDates { created_at: created, updated_at: updated });
            }
            Ok(dates)
        })
    }

    fn note_head(&mut self, new_head: &str) -> Result<()> {
        self.with_conn(|conn| set_meta(conn, "head_hash", new_head))
    }
}

// ---------------------------------------------------------------------------
// Schema helpers
// ---------------------------------------------------------------------------

/// Configures the connection for safe concurrent multi-process access.
///
/// `.next.db` is a per-process cache, but several `next` processes (CLI
/// invocations, the long-running MCP server, and future plugin processes) open
/// the *same* database file at once.  WAL mode lets readers and a writer
/// proceed concurrently instead of blocking, and `busy_timeout` makes a process
/// wait briefly for a transient lock rather than failing immediately with
/// `SQLITE_BUSY`.
fn configure_connection(conn: &Connection) -> Result<()> {
    // WAL persists in the database header; setting it on every open is
    // idempotent.  `synchronous=NORMAL` is the standard, safe companion to WAL.
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .map_err(|e| TaskError::Other(format!("sqlite pragma setup: {e}")))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| TaskError::Other(format!("sqlite busy_timeout: {e}")))?;
    Ok(())
}

/// Bumped whenever the table layout changes. A mismatch drops and recreates
/// the task tables and clears the stored head so `open()` rebuilds from TOML.
const SCHEMA_VERSION: &str = "3";

/// The version of the binary that wrote the cache, stamped into `meta`.
///
/// [`SCHEMA_VERSION`] only tracks the table *layout*; it stays put when the
/// reconcile or query *logic* changes between releases, or when a
/// cache-unaware older binary writes TOML behind the cache's back. Stamping
/// the crate version lets [`heal_on_version_change`] force a one-time full
/// rebuild on any version change, so an upgrade can never serve results
/// derived from a mismatched build. The rebuild reads from the committed TOML
/// (the source of truth), so no committed data is touched.
const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

fn setup_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
    .map_err(|e| TaskError::Other(format!("sqlite meta setup: {e}")))?;

    if get_meta(conn, "schema_version")?.as_deref() != Some(SCHEMA_VERSION) {
        conn.execute_batch(
            "
            DROP TABLE IF EXISTS tasks;
            DROP TABLE IF EXISTS task_tags;
            DELETE FROM meta WHERE key = 'head_hash';
            ",
        )
        .map_err(|e| TaskError::Other(format!("sqlite schema upgrade: {e}")))?;
        set_meta(conn, "schema_version", SCHEMA_VERSION)?;
    }

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS tasks (
            id           TEXT PRIMARY KEY,
            slug         TEXT,
            status       TEXT NOT NULL,
            priority     TEXT NOT NULL,
            due          TEXT,
            start        TEXT,
            completed_at TEXT,
            parent_id    TEXT,
            assignee     TEXT,
            archived     INTEGER NOT NULL DEFAULT 0,
            path         TEXT NOT NULL,
            created_at   TEXT,
            updated_at   TEXT,
            data         TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS task_tags (
            task_id TEXT NOT NULL,
            tag     TEXT NOT NULL,
            PRIMARY KEY (task_id, tag)
        );
        CREATE INDEX IF NOT EXISTS idx_tasks_slug   ON tasks(slug);
        CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(archived, status);
        CREATE INDEX IF NOT EXISTS idx_tasks_parent ON tasks(parent_id);
        CREATE INDEX IF NOT EXISTS idx_tasks_path   ON tasks(path);
        CREATE INDEX IF NOT EXISTS idx_task_tags_tag ON task_tags(tag);
        ",
    )
    .map_err(|e| TaskError::Other(format!("sqlite schema setup: {e}")))
}

/// Forces a full rebuild when the cache was last written by a different build.
///
/// Called after [`setup_schema`] and before the first `reconcile`. When the
/// stored `build_version` differs from the running binary's (including the
/// `None` of a pre-stamp or cache-unaware cache), it clears the stored git
/// head so `reconcile` cannot take the "head unchanged, nothing to do" fast
/// path and instead rebuilds every row from the TOML source of truth. The
/// stamp is then advanced, so the heal is a one-time cost per upgrade.
///
/// The tables are left in place — the rebuild repopulates them — so this is
/// cheaper than a [`SCHEMA_VERSION`] bump, which additionally drops them.
fn heal_on_version_change(conn: &Connection) -> Result<()> {
    if get_meta(conn, "build_version")?.as_deref() != Some(BUILD_VERSION) {
        conn.execute("DELETE FROM meta WHERE key = 'head_hash'", [])
            .map_err(|e| TaskError::Other(format!("sqlite version heal: {e}")))?;
        set_meta(conn, "build_version", BUILD_VERSION)?;
        tracing::info!(
            build_version = BUILD_VERSION,
            "cache written by a different build; rebuilding from TOML"
        );
    }
    Ok(())
}

fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| TaskError::Other(format!("sqlite meta get '{key}': {e}")))
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES(?1, ?2)",
        params![key, value],
    )
    .map_err(|e| TaskError::Other(format!("sqlite meta set '{key}': {e}")))?;
    Ok(())
}

fn status_name(status: &crate::core::domain::task::Status) -> &'static str {
    use crate::core::domain::task::Status;
    match status {
        Status::Open => "open",
        Status::Started => "started",
        Status::Done => "done",
        Status::Cancelled => "cancelled",
    }
}

fn priority_str(task: &Task) -> &'static str {
    use crate::core::domain::task::Priority;
    match task.priority {
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
    }
}

/// Inserts or updates a task row.
///
/// `dates` seeds the git-derived timestamp columns: on a fresh row both are
/// taken from it; on an existing row `created_at` is preserved and only
/// `updated_at` advances (kept when `dates` is `None`). `archived` records
/// the storage tier (0 = active `tasks/` file, 1 = warm archive segment).
fn upsert_task_row(
    conn: &Connection,
    task: &Task,
    rel_path: &str,
    dates: Option<&TaskDates>,
    archived: i64,
) -> Result<()> {
    let data = serde_json::to_string(task)
        .map_err(|e| TaskError::Other(format!("serialize task {}: {e}", task.id)))?;
    let id_str = task.id.to_string();
    conn.execute(
        "INSERT INTO tasks(id, slug, status, priority, due, start,
             completed_at, parent_id, assignee, archived, path,
             created_at, updated_at, data)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?14, ?10, ?11, ?12, ?13)
         ON CONFLICT(id) DO UPDATE SET
             slug = excluded.slug,
             status = excluded.status,
             priority = excluded.priority,
             due = excluded.due,
             start = excluded.start,
             completed_at = excluded.completed_at,
             parent_id = excluded.parent_id,
             assignee = excluded.assignee,
             archived = excluded.archived,
             path = excluded.path,
             updated_at = COALESCE(excluded.updated_at, tasks.updated_at),
             data = excluded.data",
        params![
            id_str,
            task.slug.as_deref(),
            status_name(&task.status),
            priority_str(task),
            task.due.map(|d| d.to_string()),
            task.start.map(|d| d.to_string()),
            task.completed_at.map(|d| d.to_string()),
            task.parent_id.map(|p| p.to_string()),
            task.assignee.as_deref(),
            rel_path,
            dates.map(|d| d.created_at.to_rfc3339()),
            dates.map(|d| d.updated_at.to_rfc3339()),
            data,
            archived,
        ],
    )
    .map_err(|e| TaskError::Other(format!("sqlite upsert task {}: {e}", task.id)))?;
    conn.execute("DELETE FROM task_tags WHERE task_id = ?1", params![id_str])
        .map_err(|e| TaskError::Other(format!("sqlite clear tags for {}: {e}", task.id)))?;
    for tag in &task.tags {
        conn.execute(
            "INSERT OR IGNORE INTO task_tags(task_id, tag) VALUES(?1, ?2)",
            params![id_str, tag],
        )
        .map_err(|e| TaskError::Other(format!("sqlite insert tag for {}: {e}", task.id)))?;
    }
    Ok(())
}

fn deserialize_task(data: String) -> Result<Task> {
    serde_json::from_str(&data).map_err(|e| TaskError::Other(format!("deserialize task: {e}")))
}

fn collect_task_rows<'a>(
    rows: impl Iterator<Item = rusqlite::Result<String>> + 'a,
) -> Result<Vec<Task>> {
    let mut tasks = Vec::new();
    for row in rows {
        let data = row.map_err(|e| TaskError::Other(format!("sqlite row: {e}")))?;
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

    use crate::core::{
        domain::task::{Priority, Status},
        store::VcsBackend as _,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::core::storage::GitBackend;

    fn setup() -> (TempDir, CachedStore, GitBackend) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Test").unwrap();
            cfg.set_str("user.email", "test@test.com").unwrap();
        }
        let inner = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        let vcs = GitBackend::open(dir.path()).unwrap();
        let head_hash = vcs.head_hash().unwrap();
        let db_path = dir.path().join(".next.db");
        let store = CachedStore::open(inner, db_path, &head_hash).unwrap();
        (dir, store, vcs)
    }

    #[test]
    fn connection_uses_wal_and_busy_timeout() {
        let (_dir, store, _vcs) = setup();
        let mode: String = store
            .with_conn(|conn| {
                conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                    .map_err(|e| TaskError::Other(e.to_string()))
            })
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "cache must run in WAL mode");
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
        assert!(matches!(err, TaskError::TaskNotFound(_)));
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
            TaskError::TaskNotFound(_)
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

        let state = GlobalState { active_contexts: vec!["@work".into()], ..Default::default() };
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

        task.mark_done(chrono::Local::now().date_naive());
        store.save_task(&task).unwrap();

        let loaded = store.get_task(task.id).unwrap();
        assert_eq!(loaded.status, Status::Done);
        assert_eq!(loaded.completed_at, task.completed_at);
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
            let inner = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
            let head = vcs.head_hash().unwrap();
            let db_path = dir.path().join(".next.db");
            let mut store = CachedStore::open(inner, db_path, &head).unwrap();

            let task = Task::new("Persisted");
            store.save_task(&task).unwrap();

            // Commit the TOML file to advance HEAD.
            let task_path = crate::core::storage::task_path(dir.path(), &task);
            vcs.commit(&[task_path], "add task").unwrap();
        }

        // Session 2: open with the new HEAD — cache must be rebuilt.
        {
            let inner = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
            let head = vcs.head_hash().unwrap();
            let db_path = dir.path().join(".next.db");
            let store = CachedStore::open(inner, db_path, &head).unwrap();

            let all = store.list_tasks().unwrap();
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].title, "Persisted");
        }
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
        let inner = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        let db_path = dir.path().join(".next.db");
        let store = CachedStore::open(inner, db_path, "fake-head").unwrap();

        let found = store.get_task_by_slug("external").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, "External task");
    }

    #[test]
    fn after_pull_picks_up_new_toml_files() {
        // Simulates what happens in the MCP server: store is opened once,
        // then a git pull adds new TOML files to disk. after_pull() must
        // rebuild the cache so the new tasks become visible.
        let dir = TempDir::new().unwrap();
        git2::Repository::init(dir.path()).unwrap();

        let tasks_dir = dir.path().join("tasks");
        fs::create_dir_all(&tasks_dir).unwrap();

        let inner = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        let db_path = dir.path().join(".next.db");
        let mut store = CachedStore::open(inner, db_path, "head-before-pull").unwrap();

        // Initially empty.
        assert!(store.list_tasks().unwrap().is_empty());

        // Simulate a git pull: write a new TOML file to disk directly.
        let task = Task::new("Pulled task");
        let toml = toml::to_string_pretty(&task).unwrap();
        fs::write(tasks_dir.join("pulled.toml"), toml).unwrap();

        // Cache still stale — task not visible yet.
        assert!(store.list_tasks().unwrap().is_empty());

        // after_pull with a new head hash triggers a rebuild.
        store.after_pull("head-after-pull").unwrap();

        let all = store.list_tasks().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "Pulled task");
    }

    #[test]
    fn after_pull_applies_incremental_changes() {
        let (dir, mut store, vcs) = setup();
        let mut alpha = Task::new("Alpha");
        alpha.slug = Some("alpha".into());
        let mut beta = Task::new("Beta");
        beta.slug = Some("beta".into());
        let mut delta = Task::new("Delta");
        delta.slug = Some("delta".into());
        store.save_task(&alpha).unwrap();
        store.save_task(&beta).unwrap();
        store.save_task(&delta).unwrap();
        let paths: Vec<_> = [&alpha, &beta, &delta]
            .iter()
            .map(|t| crate::core::storage::task_path(dir.path(), t))
            .collect();
        vcs.commit(&paths, "initial").unwrap();
        store.note_head(&vcs.head_hash().unwrap()).unwrap();

        // External changes, as a pull would leave them on disk.
        alpha.title = "Alpha II".into();
        fs::write(
            dir.path().join("tasks/alpha.toml"),
            toml::to_string_pretty(&alpha).unwrap(),
        )
        .unwrap();
        fs::remove_file(dir.path().join("tasks/beta.toml")).unwrap();
        let mut gamma = Task::new("Gamma");
        gamma.slug = Some("gamma".into());
        fs::write(
            dir.path().join("tasks/gamma.toml"),
            toml::to_string_pretty(&gamma).unwrap(),
        )
        .unwrap();
        vcs.commit(
            &[
                dir.path().join("tasks/alpha.toml"),
                dir.path().join("tasks/beta.toml"),
                dir.path().join("tasks/gamma.toml"),
            ],
            "external",
        )
        .unwrap();
        let head2 = vcs.head_hash().unwrap();

        // Discriminator: drop delta's row directly. The incremental path must
        // not touch it (its file did not change between the heads), while a
        // full rebuild would resurrect it.
        store
            .with_conn(|conn| {
                conn.execute("DELETE FROM tasks WHERE slug = 'delta'", [])
                    .map_err(|e| TaskError::Other(e.to_string()))?;
                Ok(())
            })
            .unwrap();

        store.after_pull(&head2).unwrap();

        assert_eq!(store.get_task(alpha.id).unwrap().title, "Alpha II");
        assert!(store.get_task_by_slug("beta").unwrap().is_none());
        assert!(store.get_task_by_slug("gamma").unwrap().is_some());
        assert!(
            store.get_task_by_slug("delta").unwrap().is_none(),
            "incremental reconcile must not rescan unchanged files"
        );
    }

    #[test]
    fn rename_via_git_diff_updates_path() {
        let (dir, mut store, vcs) = setup();
        let mut task = Task::new("Movable");
        task.slug = Some("before".into());
        store.save_task(&task).unwrap();
        vcs.commit(&[dir.path().join("tasks/before.toml")], "add").unwrap();
        store.note_head(&vcs.head_hash().unwrap()).unwrap();

        // Externally rename the file (slug change): delete + add in one commit.
        task.slug = Some("after".into());
        fs::write(
            dir.path().join("tasks/after.toml"),
            toml::to_string_pretty(&task).unwrap(),
        )
        .unwrap();
        fs::remove_file(dir.path().join("tasks/before.toml")).unwrap();
        vcs.commit(
            &[
                dir.path().join("tasks/before.toml"),
                dir.path().join("tasks/after.toml"),
            ],
            "rename",
        )
        .unwrap();

        store.after_pull(&vcs.head_hash().unwrap()).unwrap();

        assert!(store.get_task_by_slug("before").unwrap().is_none());
        let found = store.get_task_by_slug("after").unwrap().unwrap();
        assert_eq!(found.id, task.id);
        assert_eq!(store.list_tasks().unwrap().len(), 1);
    }

    #[test]
    fn schema_v2_columns_populated() {
        let (_dir, mut store, _vcs) = setup();
        let mut task = Task::new("Columned");
        task.slug = Some("columned".into());
        task.status = Status::Done;
        task.priority = Priority::High;
        task.due = chrono::NaiveDate::from_ymd_opt(2026, 8, 1);
        task.completed_at = chrono::NaiveDate::from_ymd_opt(2026, 7, 1);
        task.assignee = Some("victor".into());
        store.save_task(&task).unwrap();

        let (status, priority, due, completed, assignee, path): (String, String, String, String, String, String) = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status, priority, due, completed_at, assignee, path
                     FROM tasks WHERE id = ?1",
                    params![task.id.to_string()],
                    |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
                    },
                )
                .map_err(|e| TaskError::Other(e.to_string()))
            })
            .unwrap();
        assert_eq!(status, "done");
        assert_eq!(priority, "high");
        assert_eq!(due, "2026-08-01");
        assert_eq!(completed, "2026-07-01");
        assert_eq!(assignee, "victor");
        assert_eq!(path, "tasks/columned.toml");
    }

    #[test]
    fn task_tags_rows_maintained() {
        let (_dir, mut store, _vcs) = setup();
        let mut task = Task::new("Tagged");
        task.tags = vec!["@work".into(), "rust".into()];
        store.save_task(&task).unwrap();

        let count_tags = |store: &CachedStore, id: &str| -> i64 {
            store
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM task_tags WHERE task_id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .map_err(|e| TaskError::Other(e.to_string()))
                })
                .unwrap()
        };
        let id = task.id.to_string();
        assert_eq!(count_tags(&store, &id), 2);

        // Removing a tag must remove its row.
        task.tags = vec!["rust".into()];
        store.save_task(&task).unwrap();
        assert_eq!(count_tags(&store, &id), 1);

        // Deleting the task must remove all its tag rows.
        store.delete_task(task.id).unwrap();
        assert_eq!(count_tags(&store, &id), 0);
    }

    #[test]
    fn query_status_filter_and_pagination() {
        let (_dir, mut store, _vcs) = setup();
        for i in 0..3 {
            let mut t = Task::new(format!("Done {i}"));
            t.mark_done(chrono::NaiveDate::from_ymd_opt(2026, 6, 10 + i).unwrap());
            store.save_task(&t).unwrap();
        }
        store.save_task(&Task::new("Open A")).unwrap();
        store.save_task(&Task::new("Open B")).unwrap();

        let q = TaskQuery {
            statuses: Some(vec![Status::Done]),
            page_size: 2,
            ..Default::default()
        };
        let p1 = store.query_tasks(&q).unwrap();
        assert_eq!(p1.total, 3);
        assert_eq!(p1.items.len(), 2);
        // Most recent completion first.
        assert_eq!(p1.items[0].title, "Done 2");

        let p2 = store.query_tasks(&TaskQuery { page: 2, ..q.clone() }).unwrap();
        assert_eq!(p2.total, 3);
        assert_eq!(p2.items.len(), 1);
        let p3 = store.query_tasks(&TaskQuery { page: 3, ..q }).unwrap();
        assert!(p3.items.is_empty());

        // No overlap or gaps across pages.
        let mut ids: Vec<_> = p1.items.iter().chain(&p2.items).map(|t| t.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn query_tag_hierarchy() {
        let (_dir, mut store, _vcs) = setup();
        let mut work = Task::new("Frontend");
        work.tags = vec!["@work/frontend".into()];
        let mut home = Task::new("Kitchen");
        home.tags = vec!["@home".into()];
        let mut worker = Task::new("Networking");
        worker.tags = vec!["@workshop".into()]; // must NOT match "@work"
        store.save_task(&work).unwrap();
        store.save_task(&home).unwrap();
        store.save_task(&worker).unwrap();

        let q = TaskQuery { required_tags: vec!["@work".into()], ..Default::default() };
        let got = store.query_tasks(&q).unwrap();
        assert_eq!(got.items.len(), 1, "parent tag matches descendants only");
        assert_eq!(got.items[0].id, work.id);

        let q = TaskQuery { excluded_tags: vec!["@work".into()], ..Default::default() };
        let got = store.query_tasks(&q).unwrap();
        let titles: Vec<_> = got.items.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(got.items.len(), 2);
        assert!(titles.contains(&"Kitchen") && titles.contains(&"Networking"));
    }

    #[test]
    fn get_tasks_batch_skips_missing() {
        let (_dir, mut store, _vcs) = setup();
        let a = Task::new("A");
        let b = Task::new("B");
        store.save_task(&a).unwrap();
        store.save_task(&b).unwrap();

        let got = store.get_tasks(&[a.id, Uuid::new_v4(), b.id]).unwrap();
        assert_eq!(got.len(), 2);
        assert!(store.get_tasks(&[]).unwrap().is_empty());
    }

    #[test]
    fn query_by_parent_returns_all_status_children() {
        let (_dir, mut store, _vcs) = setup();
        let parent = Task::new("Project");
        let mut open_child = Task::new("Open child");
        open_child.parent_id = Some(parent.id);
        let mut done_child = Task::new("Done child");
        done_child.parent_id = Some(parent.id);
        done_child.mark_done(chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap());
        let unrelated = Task::new("Unrelated");
        for t in [&parent, &open_child, &done_child, &unrelated] {
            store.save_task(t).unwrap();
        }

        let got = store
            .query_tasks(&TaskQuery { parent_id: Some(parent.id), ..TaskQuery::unpaginated() })
            .unwrap();
        assert_eq!(got.total, 2);
        let ids: Vec<_> = got.items.iter().map(|t| t.id).collect();
        assert!(ids.contains(&open_child.id) && ids.contains(&done_child.id));
    }

    #[test]
    fn query_archived_is_empty_for_now() {
        let (_dir, mut store, _vcs) = setup();
        store.save_task(&Task::new("Active")).unwrap();
        let got = store
            .query_tasks(&TaskQuery { archived: true, ..Default::default() })
            .unwrap();
        assert_eq!(got.total, 0);
        assert!(got.items.is_empty());
    }

    #[test]
    fn archive_segments_flow_through_cache() {
        use crate::core::storage::archive::{write_segment, ArchivedTask};

        let (dir, mut store, vcs) = setup();
        store.save_task(&Task::new("Active")).unwrap();

        // An external archive commit: a new segment appears on disk.
        let mut old = Task::new("Ancient");
        old.slug = Some("ancient".into());
        old.mark_done(chrono::NaiveDate::from_ymd_opt(2025, 12, 5).unwrap());
        let seg_abs = dir.path().join("archive/2025/12-001.toml");
        let frozen = chrono::DateTime::from_timestamp(1_700_000_000, 0);
        write_segment(
            &seg_abs,
            vec![ArchivedTask { created_at: frozen, updated_at: frozen, task: old.clone() }],
        )
        .unwrap();
        vcs.commit(std::slice::from_ref(&seg_abs), "archive pass").unwrap();
        store.after_pull(&vcs.head_hash().unwrap()).unwrap();

        // Hidden from active listings and queries…
        assert_eq!(store.list_tasks().unwrap().len(), 1);
        assert_eq!(store.query_tasks(&TaskQuery::default()).unwrap().total, 1);
        // …visible through the archived query, ordered lookups intact…
        let archived = store
            .query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() })
            .unwrap();
        assert_eq!(archived.total, 1);
        assert_eq!(archived.items[0].id, old.id);
        // …and tier-transparent for direct reads, with frozen dates served.
        assert_eq!(store.get_task(old.id).unwrap().title, "Ancient");
        assert!(store.get_task_by_slug("ancient").unwrap().is_some());
        assert_eq!(store.task_dates().unwrap()[&old.id].created_at, frozen.unwrap());

        // A fresh rebuild (new db) also picks segments up without history.
        let inner = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        let store2 =
            CachedStore::open(inner, dir.path().join(".next2.db"), &vcs.head_hash().unwrap())
                .unwrap();
        assert_eq!(
            store2.query_tasks(&TaskQuery { archived: true, ..TaskQuery::unpaginated() }).unwrap().total,
            1
        );

        // Segment removal (e.g. cold-tier prune) drops the rows.
        std::fs::remove_file(&seg_abs).unwrap();
        vcs.commit(&[seg_abs], "prune segment").unwrap();
        store.after_pull(&vcs.head_hash().unwrap()).unwrap();
        assert!(store.get_task(old.id).is_err());
    }

    #[test]
    fn query_matches_reference_implementation() {
        // The SQL pushdown must agree with the trait's in-memory default.
        let (dir, mut store, _vcs) = setup();
        let mut a = Task::new("A");
        a.tags = vec!["@work/frontend".into(), "rust".into()];
        let mut b = Task::new("B");
        b.tags = vec!["@work".into()];
        b.mark_done(chrono::NaiveDate::from_ymd_opt(2026, 5, 1).unwrap());
        let mut c = Task::new("C");
        c.tags = vec!["@home".into(), "rust".into()];
        for t in [&a, &b, &c] {
            store.save_task(t).unwrap();
        }

        let queries = [
            TaskQuery::default(),
            TaskQuery { statuses: Some(vec![Status::Open]), ..Default::default() },
            TaskQuery { required_tags: vec!["@work".into()], ..Default::default() },
            TaskQuery {
                required_tags: vec!["rust".into()],
                excluded_tags: vec!["@home".into()],
                ..Default::default()
            },
            TaskQuery { page_size: 2, page: 2, ..Default::default() },
        ];
        // A bare TomlStore over the same directory exercises the default impl.
        let reference =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        for q in queries {
            let sql = store.query_tasks(&q).unwrap();
            let mem = reference.query_tasks(&q).unwrap();
            let sql_ids: Vec<_> = sql.items.iter().map(|t| t.id).collect();
            let mem_ids: Vec<_> = mem.items.iter().map(|t| t.id).collect();
            assert_eq!(sql_ids, mem_ids, "query {q:?} diverged from reference");
            assert_eq!(sql.total, mem.total, "total for {q:?}");
        }
    }

    #[test]
    fn save_task_sets_and_preserves_created_at() {
        let (_dir, mut store, _vcs) = setup();
        let mut task = Task::new("Dated");
        store.save_task(&task).unwrap();
        let d1 = store.task_dates().unwrap()[&task.id].clone();
        assert_eq!(d1.created_at, d1.updated_at);

        std::thread::sleep(std::time::Duration::from_millis(10));
        task.title = "Dated II".into();
        store.save_task(&task).unwrap();
        let d2 = store.task_dates().unwrap()[&task.id].clone();
        assert_eq!(d2.created_at, d1.created_at, "created_at must not change on update");
        assert!(d2.updated_at > d1.updated_at, "updated_at must advance");
    }

    #[test]
    fn rebuild_backfills_dates_from_git_history() {
        let (dir, mut store, vcs) = setup();
        let mut slugged = Task::new("Slugged");
        slugged.slug = Some("slugged".into());
        let plain = Task::new("Plain");
        store.save_task(&slugged).unwrap();
        store.save_task(&plain).unwrap();
        vcs.commit(
            &[
                crate::core::storage::task_path(dir.path(), &slugged),
                crate::core::storage::task_path(dir.path(), &plain),
            ],
            "add tasks",
        )
        .unwrap();
        let head = vcs.head_hash().unwrap();

        // Fresh cache in a separate db file → full rebuild with backfill.
        let inner = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        let store2 = CachedStore::open(inner, dir.path().join(".next2.db"), &head).unwrap();
        let dates = store2.task_dates().unwrap();
        assert!(dates.contains_key(&plain.id), "uuid-suffixed file must be backfilled");
        assert!(
            dates.contains_key(&slugged.id),
            "slug-named file must be backfilled via its filename key"
        );
    }

    #[test]
    fn incremental_upsert_keeps_created_at() {
        let (dir, mut store, vcs) = setup();
        let mut task = Task::new("Evolving");
        task.slug = Some("evolving".into());
        store.save_task(&task).unwrap();
        vcs.commit(&[dir.path().join("tasks/evolving.toml")], "add").unwrap();
        store.note_head(&vcs.head_hash().unwrap()).unwrap();
        let before = store.task_dates().unwrap()[&task.id].clone();

        task.title = "Evolved".into();
        fs::write(
            dir.path().join("tasks/evolving.toml"),
            toml::to_string_pretty(&task).unwrap(),
        )
        .unwrap();
        vcs.commit(&[dir.path().join("tasks/evolving.toml")], "modify").unwrap();
        store.after_pull(&vcs.head_hash().unwrap()).unwrap();

        let after = store.task_dates().unwrap()[&task.id].clone();
        assert_eq!(after.created_at, before.created_at, "external modify keeps created_at");
        assert_eq!(store.get_task(task.id).unwrap().title, "Evolved");
    }

    #[test]
    fn rename_keeps_created_at() {
        let (dir, mut store, vcs) = setup();
        let mut task = Task::new("Renamed");
        task.slug = Some("r1".into());
        store.save_task(&task).unwrap();
        vcs.commit(&[dir.path().join("tasks/r1.toml")], "add").unwrap();
        store.note_head(&vcs.head_hash().unwrap()).unwrap();
        let before = store.task_dates().unwrap()[&task.id].clone();

        task.slug = Some("r2".into());
        fs::write(dir.path().join("tasks/r2.toml"), toml::to_string_pretty(&task).unwrap()).unwrap();
        fs::remove_file(dir.path().join("tasks/r1.toml")).unwrap();
        vcs.commit(
            &[dir.path().join("tasks/r1.toml"), dir.path().join("tasks/r2.toml")],
            "rename",
        )
        .unwrap();
        store.after_pull(&vcs.head_hash().unwrap()).unwrap();

        let after = store.task_dates().unwrap()[&task.id].clone();
        assert_eq!(after.created_at, before.created_at, "rename keeps created_at");
    }

    #[test]
    fn slug_conflict_detected_via_cache() {
        let (_dir, mut store, _vcs) = setup();
        let mut t1 = Task::new("First");
        t1.slug = Some("shared".into());
        let mut t2 = Task::new("Second");
        t2.slug = Some("shared".into());

        store.save_task(&t1).unwrap();
        let err = store.save_task(&t2).unwrap_err();
        assert!(matches!(err, TaskError::SlugConflict(_)));

        // Re-saving the owner of the slug is not a conflict.
        t1.title = "First renamed".into();
        store.save_task(&t1).unwrap();
    }

    #[test]
    fn slug_rename_moves_file_and_updates_path() {
        let (dir, mut store, _vcs) = setup();
        let mut task = Task::new("Mover");
        task.slug = Some("old-name".into());
        store.save_task(&task).unwrap();
        assert!(dir.path().join("tasks/old-name.toml").exists());

        task.slug = Some("new-name".into());
        store.save_task(&task).unwrap();

        assert!(!dir.path().join("tasks/old-name.toml").exists());
        assert!(dir.path().join("tasks/new-name.toml").exists());
        assert_eq!(store.list_tasks().unwrap().len(), 1);

        let path: String = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT path FROM tasks WHERE id = ?1",
                    params![task.id.to_string()],
                    |r| r.get(0),
                )
                .map_err(|e| TaskError::Other(e.to_string()))
            })
            .unwrap();
        assert_eq!(path, "tasks/new-name.toml");
    }

    #[test]
    fn delete_uses_path_hint() {
        let (dir, mut store, _vcs) = setup();
        let mut task = Task::new("Doomed");
        task.slug = Some("doomed".into());
        store.save_task(&task).unwrap();
        assert!(dir.path().join("tasks/doomed.toml").exists());

        store.delete_task(task.id).unwrap();
        assert!(!dir.path().join("tasks/doomed.toml").exists());
        assert!(store.list_tasks().unwrap().is_empty());
    }

    #[test]
    fn schema_v1_db_is_dropped_and_rebuilt() {
        let dir = TempDir::new().unwrap();
        git2::Repository::init(dir.path()).unwrap();

        // A real task on disk that the rebuild must pick up.
        let mut inner = TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        let task = Task::new("Survivor");
        inner.save_task(&task).unwrap();

        // Hand-craft a v1 database: no schema_version, old table layout, a
        // stale row, and a head_hash matching what we will open with (so only
        // the version mismatch can trigger the rebuild).
        let db_path = dir.path().join(".next.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE meta  (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                CREATE TABLE tasks (id TEXT PRIMARY KEY, slug TEXT, data TEXT NOT NULL);
                INSERT INTO meta(key, value) VALUES('head_hash', 'same-head');
                INSERT INTO tasks(id, slug, data) VALUES('stale', NULL, '{}');
                ",
            )
            .unwrap();
        }

        let store = CachedStore::open(inner, db_path, "same-head").unwrap();
        let all = store.list_tasks().unwrap();
        assert_eq!(all.len(), 1, "stale v1 row must be gone, disk task present");
        assert_eq!(all[0].title, "Survivor");

        let version = store.with_conn(|conn| get_meta(conn, "schema_version")).unwrap();
        assert_eq!(version.as_deref(), Some(SCHEMA_VERSION));
    }

    #[test]
    fn build_version_mismatch_forces_rebuild_despite_matching_head() {
        let dir = TempDir::new().unwrap();
        git2::Repository::init(dir.path()).unwrap();

        // A real task on disk the rebuild must surface.
        let mut inner =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        let task = Task::new("Survivor");
        inner.save_task(&task).unwrap();

        // A cache with the *current* table layout and a head that matches the
        // one we open with, but stamped with an old build version and holding a
        // stale row. Only the build-version mismatch can trigger a rebuild here
        // — reconcile's fast path would otherwise keep the stale row.
        let db_path = dir.path().join(".next.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            setup_schema(&conn).unwrap();
            set_meta(&conn, "build_version", "0.0.0-old").unwrap();
            set_meta(&conn, "head_hash", "same-head").unwrap();
            let stale = Task::new("Stale");
            upsert_task_row(&conn, &stale, "tasks/stale.toml", None, 0).unwrap();
        }

        let store = CachedStore::open(inner, db_path, "same-head").unwrap();
        let all = store.list_tasks().unwrap();
        assert_eq!(all.len(), 1, "stale row must be gone, disk task present");
        assert_eq!(all[0].title, "Survivor");

        let stamped = store.with_conn(|conn| get_meta(conn, "build_version")).unwrap();
        assert_eq!(stamped.as_deref(), Some(BUILD_VERSION));
    }

    #[test]
    fn matching_build_version_keeps_reconcile_fast_path() {
        let dir = TempDir::new().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        let inner =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

        // Current schema + current build version + matching head + a lone row
        // that is NOT on disk. If the heal wrongly fired, the rebuild would drop
        // it; because the stamp already matches, the fast path preserves it.
        let db_path = dir.path().join(".next.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            setup_schema(&conn).unwrap();
            set_meta(&conn, "build_version", BUILD_VERSION).unwrap();
            set_meta(&conn, "head_hash", "same-head").unwrap();
            let ghost = Task::new("Ghost");
            upsert_task_row(&conn, &ghost, "tasks/ghost.toml", None, 0).unwrap();
        }

        let store = CachedStore::open(inner, db_path, "same-head").unwrap();
        let all = store.list_tasks().unwrap();
        assert_eq!(all.len(), 1, "fast path must not rebuild when the stamp matches");
        assert_eq!(all[0].title, "Ghost");
    }
}
