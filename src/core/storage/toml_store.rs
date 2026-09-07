use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use crate::core::{
    domain::{state::GlobalState, tag::TagMeta, task::Task},
    error::{Result, TaskError},
    store::Store,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Legacy on-disk format — only used during migration to read old single-field files.
#[derive(Debug, Serialize, Deserialize)]
struct LegacyTagEntry {
    pub description: String,
}

pub struct TomlStore {
    root: PathBuf,
    /// Absolute path to the state file (outside the repository).
    state_path: PathBuf,
    /// Lock file co-located with `state_path`.
    state_lock_path: PathBuf,
}

impl TomlStore {
    /// Opens (or initialises) a store rooted at `root`.
    ///
    /// `state_path` is the external path where `state.toml` will be kept
    /// (typically `$XDG_STATE_HOME/next/<hash>/state.toml`).
    ///
    /// Creates `tasks/`, `tags/`, and the state directory if they do not exist.
    /// Migrates old `<root>/state.toml` to `state_path` on first open.
    /// Migrates `tag_descriptions` from state into per-tag files.
    pub fn open(root: PathBuf, state_path: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("tasks"))?;
        fs::create_dir_all(root.join("tags"))?;
        if let Some(state_dir) = state_path.parent() {
            fs::create_dir_all(state_dir)?;
        }
        let state_lock_path = crate::core::storage::state_lock_path(&state_path);
        let mut this = Self {
            root,
            state_path,
            state_lock_path,
        };
        this.migrate_state_file()?;
        this.migrate_tag_descriptions()?;
        Ok(this)
    }

    /// If the old `<root>/state.toml` exists and the new `state_path` does not,
    /// move the old file to the new location. This is a one-time migration.
    fn migrate_state_file(&self) -> Result<()> {
        let old = self.root.join("state.toml");
        if old.exists() && !self.state_path.exists() {
            fs::copy(&old, &self.state_path)
                .map_err(|e| TaskError::Other(format!("migrate state.toml: {e}")))?;
            let _ = fs::remove_file(&old);
        }
        Ok(())
    }

    /// If `state.toml` contains a legacy `tag_descriptions` table, move each
    /// entry into its own `tags/<tag>.toml` file and re-save state without it.
    fn migrate_tag_descriptions(&mut self) -> Result<()> {
        let state_path = self.state_path();
        if !state_path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(state_path)?;
        let raw: toml::Value = toml::from_str(&content)
            .map_err(|e| TaskError::Other(format!("parse state.toml during migration: {e}")))?;

        let Some(toml::Value::Table(descs)) = raw.get("tag_descriptions") else {
            return Ok(());
        };
        if descs.is_empty() {
            return Ok(());
        }

        let pairs: Vec<(String, String)> = descs
            .iter()
            .filter_map(|(tag, val)| val.as_str().map(|s| (tag.clone(), s.to_owned())))
            .collect();

        for (tag, desc) in pairs {
            self.set_tag_description(&tag, &desc)?;
        }

        // Re-save state; GlobalState no longer has tag_descriptions so the
        // field is silently dropped on the next write.
        let state = self.get_state()?;
        self.save_state(&state)?;
        Ok(())
    }

    fn tasks_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }

    fn state_path(&self) -> &Path {
        &self.state_path
    }

    fn repo_lock_path(&self) -> PathBuf {
        self.root.join(".next.lock")
    }

    fn state_lock_path(&self) -> &Path {
        &self.state_lock_path
    }

    fn tags_dir(&self) -> PathBuf {
        self.root.join("tags")
    }

    /// Absolute path for the metadata file of `tag`.
    /// `@home/kitchen` → `<root>/tags/%40home/kitchen.toml`
    fn tag_file_path(&self, tag: &str) -> PathBuf {
        self.tags_dir().join(format!(
            "{}.toml",
            crate::core::storage::encode_tag_path(tag)
        ))
    }

    /// Acquires the exclusive repository-level lock.
    ///
    /// The lock is released when the returned guard is dropped.  Both
    /// `TomlStore` (task writes) and `GitBackend` (commit/pull/push) use the
    /// same `.next.lock` file via [`crate::core::storage::FileLock`], so they are
    /// mutually exclusive across processes and threads, and re-entrant within a
    /// thread (so a transaction may hold the lock across nested writes).
    pub(crate) fn acquire_repo_lock(&self) -> Result<crate::core::storage::FileLock> {
        crate::core::storage::FileLock::acquire(&self.repo_lock_path())
    }

    /// Finds the current on-disk path for `id`, or `None` if not found.
    ///
    /// Checks files whose names contain the UUID prefix first (fast path for
    /// non-slug tasks), then falls back to reading slug-named files.
    fn find_task_file(&self, id: Uuid) -> Result<Option<PathBuf>> {
        let uuid_hex = id.to_string().replace('-', "");
        let suffix = format!("-{}.toml", &uuid_hex[..8]);

        let mut priority: Vec<PathBuf> = Vec::new();
        let mut rest: Vec<PathBuf> = Vec::new();

        for entry in fs::read_dir(self.tasks_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.ends_with(&suffix) {
                priority.push(path);
            } else {
                rest.push(path);
            }
        }

        for path in priority.into_iter().chain(rest) {
            let content = match fs::read_to_string(path.as_path()) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Ok(task) = toml::from_str::<Task>(&content) {
                if task.id == id {
                    return Ok(Some(path));
                }
            }
        }

        Ok(None)
    }

    fn read_all_tasks(&self) -> Result<Vec<Task>> {
        Ok(self
            .list_tasks_with_paths()?
            .into_iter()
            .map(|(_, task)| task)
            .collect())
    }

    /// Returns the repository root this store was opened at.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Saves `task` at its canonical path, removing the file at
    /// `old_rel_path` when the task moved (slug or title change).
    ///
    /// The path-indexed fast path used by `CachedStore`: the caller supplies
    /// the previous location from its index instead of this store scanning
    /// the whole `tasks/` directory, and has already checked slug uniqueness
    /// against that index. The canonical slug file is still probed here to
    /// catch a not-yet-committed write by another process that the caller's
    /// index cannot know about.
    pub(crate) fn save_task_at(&mut self, task: &Task, old_rel_path: Option<&str>) -> Result<()> {
        let _lock = self.acquire_repo_lock()?;

        if let Some(ref slug) = task.slug {
            let candidate = self.tasks_dir().join(format!("{slug}.toml"));
            if let Ok(content) = fs::read_to_string(&candidate) {
                if let Ok(existing) = toml::from_str::<Task>(&content) {
                    if existing.id != task.id && existing.slug.as_deref() == Some(slug.as_str()) {
                        return Err(TaskError::SlugConflict(slug.clone()));
                    }
                }
            }
        }

        let new_path = self
            .tasks_dir()
            .join(crate::core::storage::filenames::generate_filename(task));
        if let Some(old_rel) = old_rel_path {
            let old_abs = self.root.join(old_rel);
            if old_abs != new_path {
                let _ = fs::remove_file(&old_abs);
            }
        }

        let content = toml::to_string_pretty(task)
            .map_err(|e| TaskError::Other(format!("TOML serialization error: {e}")))?;
        atomic_write(&new_path, &content)?;
        Ok(())
    }

    /// Deletes the task file at `rel_path`; falls back to the directory scan
    /// when the hint is stale (file already gone or moved).
    pub(crate) fn delete_task_at(&mut self, id: Uuid, rel_path: &str) -> Result<()> {
        let _lock = self.acquire_repo_lock()?;
        let abs = self.root.join(rel_path);
        if abs.exists() {
            fs::remove_file(&abs)?;
            Ok(())
        } else {
            self.delete_task(id)
        }
    }

    /// Reads every task file together with its repo-relative path
    /// (`tasks/<filename>`, forward slashes — the same form git reports).
    pub(crate) fn list_tasks_with_paths(&self) -> Result<Vec<(String, Task)>> {
        let mut tasks = Vec::new();
        for entry in fs::read_dir(self.tasks_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let content = fs::read_to_string(path.as_path())?;
            let task = toml::from_str::<Task>(&content)
                .map_err(|e| TaskError::Other(format!("parse error in {}: {e}", path.display())))?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            tasks.push((format!("tasks/{name}"), task));
        }
        Ok(tasks)
    }
}

/// Writes `content` to `path` atomically by first writing to `<path>.tmp`
/// then calling `rename`, which is atomic on POSIX systems.
///
/// Readers always see either the old complete file or the new complete file —
/// never a partially-written one.
pub(crate) fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);
    fs::write(&tmp_path, content)
        .map_err(|e| TaskError::Other(format!("write {}: {e}", tmp_path.display())))?;
    fs::rename(&tmp_path, path)
        .map_err(|e| TaskError::Other(format!("rename to {}: {e}", path.display())))?;
    Ok(())
}

/// Recursively walks `dir`, collecting tag metadata files into `result`.
/// Each file's path relative to `root` (without `.toml`) becomes the tag key.
fn walk_tags_dir(dir: &Path, root: &Path, result: &mut HashMap<String, TagMeta>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk_tags_dir(&path, root, result)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Ok(content) = fs::read_to_string(path.as_path()) {
                // Try full TagMeta first; fall back to legacy single-field format.
                let meta = toml::from_str::<TagMeta>(&content).unwrap_or_else(|_| {
                    toml::from_str::<LegacyTagEntry>(&content)
                        .map(|le| TagMeta {
                            description: Some(le.description),
                            ..Default::default()
                        })
                        .unwrap_or_default()
                });
                if let Ok(rel) = path.strip_prefix(root) {
                    if let Some(encoded) = rel.with_extension("").to_str() {
                        let tag = crate::core::storage::decode_tag_path(encoded);
                        result.insert(tag, meta);
                    }
                }
            }
        }
    }
    Ok(())
}

impl Store for TomlStore {
    fn get_task(&self, id: Uuid) -> Result<Task> {
        match self.find_task_file(id)? {
            Some(path) => {
                let content = fs::read_to_string(path.as_path())?;
                toml::from_str::<Task>(&content).map_err(|e| {
                    TaskError::Other(format!("parse error in {}: {e}", path.display()))
                })
            }
            None => Err(TaskError::TaskNotFound(id.to_string())),
        }
    }

    fn get_task_by_slug(&self, slug: &str) -> Result<Option<Task>> {
        let candidate = self.tasks_dir().join(format!("{slug}.toml"));
        if candidate.exists() {
            let content = fs::read_to_string(&candidate)?;
            if let Ok(task) = toml::from_str::<Task>(&content) {
                if task.slug.as_deref() == Some(slug) {
                    return Ok(Some(task));
                }
            }
        }
        Ok(self
            .read_all_tasks()?
            .into_iter()
            .find(|t| t.slug.as_deref() == Some(slug)))
    }

    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>> {
        Ok(self
            .read_all_tasks()?
            .into_iter()
            .filter(|t| t.id.to_string().starts_with(prefix))
            .collect())
    }

    fn list_tasks(&self) -> Result<Vec<Task>> {
        self.read_all_tasks()
    }

    fn save_task(&mut self, task: &Task) -> Result<()> {
        let _lock = self.acquire_repo_lock()?;

        if let Some(ref slug) = task.slug {
            if let Some(existing) = self.get_task_by_slug(slug)? {
                if existing.id != task.id {
                    return Err(TaskError::SlugConflict(slug.clone()));
                }
            }
        }

        let new_filename = crate::core::storage::filenames::generate_filename(task);
        let new_path = self.tasks_dir().join(&new_filename);

        if let Some(old_path) = self.find_task_file(task.id)? {
            if old_path != new_path {
                fs::remove_file(&old_path)?;
            }
        }

        let content = toml::to_string_pretty(task)
            .map_err(|e| TaskError::Other(format!("TOML serialization error: {e}")))?;
        atomic_write(&new_path, &content)?;
        Ok(())
    }

    fn delete_task(&mut self, id: Uuid) -> Result<()> {
        let _lock = self.acquire_repo_lock()?;
        match self.find_task_file(id)? {
            Some(path) => Ok(fs::remove_file(path)?),
            None => Err(TaskError::TaskNotFound(id.to_string())),
        }
    }

    fn get_state(&self) -> Result<GlobalState> {
        // Reads the global section of the combined machine-local `state.toml`.
        // Takes the (re-entrant) state lock so this read is consistent with
        // concurrent writers and does not deadlock inside a held state
        // transaction. Keyed off this store's own `state_path`.
        Ok(
            crate::core::storage::load_machine_state_at(self.state_path(), self.state_lock_path())?
                .global,
        )
    }

    fn save_state(&mut self, state: &GlobalState) -> Result<()> {
        // Rewrites only the global section, preserving the plugin and sync
        // sections, under the single (re-entrant, exclusive) state lock.
        crate::core::storage::update_machine_state_at(
            self.state_path(),
            self.state_lock_path(),
            |machine| {
                machine.global = state.clone();
                Ok(())
            },
        )
    }

    fn get_tag_meta(&self, tag: &str) -> Result<Option<TagMeta>> {
        let path = self.tag_file_path(tag);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path.as_path())?;
        let meta = toml::from_str::<TagMeta>(&content).unwrap_or_else(|_| {
            toml::from_str::<LegacyTagEntry>(&content)
                .map(|le| TagMeta {
                    description: Some(le.description),
                    ..Default::default()
                })
                .unwrap_or_default()
        });
        Ok(Some(meta))
    }

    fn set_tag_meta(&mut self, tag: &str, meta: TagMeta) -> Result<()> {
        let _lock = self.acquire_repo_lock()?;
        let path = self.tag_file_path(tag);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&meta)
            .map_err(|e| TaskError::Other(format!("TOML serialization error: {e}")))?;
        atomic_write(&path, &content)?;
        Ok(())
    }

    fn delete_tag_meta(&mut self, tag: &str) -> Result<()> {
        let _lock = self.acquire_repo_lock()?;
        let path = self.tag_file_path(tag);
        if !path.exists() {
            return Err(TaskError::Other(format!("no metadata set for tag {tag:?}")));
        }
        fs::remove_file(&path)?;
        // Remove empty parent directories up to (but not including) tags/.
        let tags_dir = self.tags_dir();
        let mut parent = path.parent();
        while let Some(dir) = parent {
            if dir == tags_dir {
                break;
            }
            if fs::read_dir(dir)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
            {
                let _ = fs::remove_dir(dir);
            }
            parent = dir.parent();
        }
        Ok(())
    }

    fn list_tag_metas(&self) -> Result<HashMap<String, TagMeta>> {
        let tags_dir = self.tags_dir();
        if !tags_dir.exists() {
            return Ok(HashMap::new());
        }
        let mut result = HashMap::new();
        walk_tags_dir(&tags_dir, &tags_dir, &mut result)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {

    use crate::core::domain::task::{Priority, Recurrence, Snap, SnapLeeway, Status};

    use super::*;

    fn temp_store() -> (tempfile::TempDir, TomlStore) {
        let dir = tempfile::TempDir::new().unwrap();
        let state_path = dir.path().join("state.toml");
        let store = TomlStore::open(dir.path().to_path_buf(), state_path).unwrap();
        (dir, store)
    }

    #[test]
    fn round_trip_basic_task() {
        let (_dir, mut store) = temp_store();
        let task = Task::new("Buy milk");
        store.save_task(&task).unwrap();
        let loaded = store.get_task(task.id).unwrap();
        assert_eq!(loaded.title, task.title);
        assert_eq!(loaded.id, task.id);
        assert_eq!(loaded.status, Status::Open);
        assert_eq!(loaded.priority, Priority::Medium);
    }

    #[test]
    fn round_trip_full_task() {
        use chrono::NaiveDate;
        let (_dir, mut store) = temp_store();
        let mut task = Task::new("Complex task");
        task.slug = Some("complex-task".into());
        task.due = Some(NaiveDate::from_ymd_opt(2026, 12, 31).unwrap());
        task.start = Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap());
        task.priority = Priority::High;
        task.tags = vec!["@work".into(), "$laptop".into()];
        task.notes = Some("Some notes".into());
        task.score_adjustment = 1.5;
        task.long_term = true;
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
            snap_leeway: None,
        });

        store.save_task(&task).unwrap();
        let loaded = store.get_task(task.id).unwrap();

        assert_eq!(loaded.slug, task.slug);
        assert_eq!(loaded.due, task.due);
        assert_eq!(loaded.start, task.start);
        assert_eq!(loaded.priority, task.priority);
        assert_eq!(loaded.tags, task.tags);
        assert_eq!(loaded.notes, task.notes);
        assert_eq!(loaded.score_adjustment, task.score_adjustment);
        assert!(loaded.long_term);
        assert!(matches!(
            loaded.recurrence,
            Some(Recurrence::Completion {
                interval_days: 7,
                snap: None,
                snap_leeway: None
            })
        ));
    }

    #[test]
    fn get_task_by_slug() {
        let (_dir, mut store) = temp_store();
        let mut task = Task::new("Water plants");
        task.slug = Some("water-plants".into());
        store.save_task(&task).unwrap();

        let found = store.get_task_by_slug("water-plants").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, "Water plants");

        let missing = store.get_task_by_slug("nonexistent").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn list_tasks_returns_all() {
        let (_dir, mut store) = temp_store();
        store.save_task(&Task::new("Task one")).unwrap();
        store.save_task(&Task::new("Task two")).unwrap();
        store.save_task(&Task::new("Task three")).unwrap();
        let all = store.list_tasks().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn find_tasks_by_prefix() {
        let (_dir, mut store) = temp_store();
        let task = Task::new("Prefixed task");
        store.save_task(&task).unwrap();

        let prefix = &task.id.to_string()[..8];
        let found = store.find_tasks_by_prefix(prefix).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, task.id);
    }

    #[test]
    fn find_tasks_by_prefix_no_match() {
        let (_dir, mut store) = temp_store();
        store.save_task(&Task::new("Some task")).unwrap();
        let found = store.find_tasks_by_prefix("00000000").unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn delete_task() {
        let (_dir, mut store) = temp_store();
        let task = Task::new("Temporary");
        store.save_task(&task).unwrap();
        assert!(store.get_task(task.id).is_ok());

        store.delete_task(task.id).unwrap();
        assert!(store.get_task(task.id).is_err());
    }

    #[test]
    fn delete_nonexistent_task_errors() {
        let (_dir, mut store) = temp_store();
        let err = store.delete_task(Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, TaskError::TaskNotFound(_)));
    }

    #[test]
    fn slug_conflict_rejected() {
        let (_dir, mut store) = temp_store();
        let mut t1 = Task::new("First");
        t1.slug = Some("shared".into());
        let mut t2 = Task::new("Second");
        t2.slug = Some("shared".into());

        store.save_task(&t1).unwrap();
        let err = store.save_task(&t2).unwrap_err();
        assert!(matches!(err, TaskError::SlugConflict(_)));
    }

    #[test]
    fn update_task_overwrites_in_place() {
        let (_dir, mut store) = temp_store();
        let mut task = Task::new("Original title");
        store.save_task(&task).unwrap();

        task.title = "Updated title".into();
        store.save_task(&task).unwrap();

        let all = store.list_tasks().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "Updated title");
    }

    #[test]
    fn rename_slug_removes_old_file() {
        let (_dir, mut store) = temp_store();
        let mut task = Task::new("My task");
        task.slug = Some("old-slug".into());
        store.save_task(&task).unwrap();
        assert!(store.get_task_by_slug("old-slug").unwrap().is_some());

        task.slug = Some("new-slug".into());
        store.save_task(&task).unwrap();

        assert!(store.get_task_by_slug("old-slug").unwrap().is_none());
        assert!(store.get_task_by_slug("new-slug").unwrap().is_some());
        assert_eq!(store.list_tasks().unwrap().len(), 1);
    }

    #[test]
    fn state_round_trip() {
        let (_dir, mut store) = temp_store();

        use crate::core::domain::state::TagState;

        let default = store.get_state().unwrap();
        assert!(default.tags.is_empty());

        let mut state = GlobalState::default();
        state.set_state("@work", Some(TagState::Required));
        state.set_state("#printer", Some(TagState::Excluded));
        store.save_state(&state).unwrap();

        let loaded = store.get_state().unwrap();
        assert_eq!(loaded.tags, state.tags);
    }

    // -----------------------------------------------------------------------
    // Migration tests
    // -----------------------------------------------------------------------

    #[test]
    fn migrate_no_state_file_is_noop() {
        let (_dir, store) = temp_store();
        // No state.toml exists; migration must be a silent no-op.
        assert!(store.list_tag_descriptions().unwrap().is_empty());
    }

    #[test]
    fn migrate_state_without_tag_descriptions_is_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("state.toml"),
            "active_users = [\"alice\"]\n",
        )
        .unwrap();
        let store =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        // Existing state fields must be untouched.
        let state = store.get_state().unwrap();
        assert_eq!(state.active_users, vec!["alice"]);
        assert!(store.list_tag_descriptions().unwrap().is_empty());
    }

    #[test]
    fn migrate_empty_tag_descriptions_is_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("state.toml"), "[tag_descriptions]\n").unwrap();
        let store =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        assert!(store.list_tag_descriptions().unwrap().is_empty());
    }

    #[test]
    fn migrate_moves_descriptions_to_tag_files() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("state.toml"),
            "[tag_descriptions]\n\
             \"@work\" = \"Tasks at the office\"\n\
             \"#printer\" = \"Laser printer\"\n\
             \"python\" = \"Python work\"\n",
        )
        .unwrap();

        let store =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

        let descs = store.list_tag_descriptions().unwrap();
        assert_eq!(
            descs.get("@work").map(String::as_str),
            Some("Tasks at the office")
        );
        assert_eq!(
            descs.get("#printer").map(String::as_str),
            Some("Laser printer")
        );
        assert_eq!(descs.get("python").map(String::as_str), Some("Python work"));
        assert_eq!(descs.len(), 3);

        // Per-tag files created on disk (@ → __context__, # → __resource__).
        assert!(dir
            .path()
            .join("tags")
            .join("__context__work.toml")
            .exists());
        assert!(dir
            .path()
            .join("tags")
            .join("__resource__printer.toml")
            .exists());
        assert!(dir.path().join("tags").join("python.toml").exists());

        // Legacy field stripped from state.toml.
        let raw = fs::read_to_string(dir.path().join("state.toml")).unwrap();
        assert!(
            !raw.contains("tag_descriptions"),
            "tag_descriptions must be stripped from state.toml after migration"
        );
    }

    #[test]
    fn migrate_hierarchical_tags_create_subdirs() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("state.toml"),
            "[tag_descriptions]\n\
             \"@home/kitchen\" = \"Kitchen tasks\"\n\
             \"#laptop/personal\" = \"Personal laptop\"\n",
        )
        .unwrap();

        let store =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

        let descs = store.list_tag_descriptions().unwrap();
        assert_eq!(
            descs.get("@home/kitchen").map(String::as_str),
            Some("Kitchen tasks")
        );
        assert_eq!(
            descs.get("#laptop/personal").map(String::as_str),
            Some("Personal laptop")
        );

        // Hierarchical paths turn into real subdirectories (@ and # encoded).
        assert!(dir
            .path()
            .join("tags/__context__home/kitchen.toml")
            .exists());
        assert!(dir
            .path()
            .join("tags/__resource__laptop/personal.toml")
            .exists());
    }

    #[test]
    fn migrate_preserves_other_state_fields() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("state.toml"),
            "active_users = [\"alice\"]\n\
             \n\
             [tags]\n\
             \"@work\" = \"included\"\n\
             \n\
             [tag_descriptions]\n\
             \"@work\" = \"Work context\"\n",
        )
        .unwrap();

        let store =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();

        let state = store.get_state().unwrap();
        assert_eq!(state.active_users, vec!["alice"]);
        assert_eq!(
            state.state_of("@work"),
            Some(crate::core::domain::state::TagState::Required)
        );
    }

    #[test]
    fn migrate_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("state.toml"),
            "[tag_descriptions]\n\"@work\" = \"Work tasks\"\n",
        )
        .unwrap();

        // First open migrates.
        drop(TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap());

        // Second open: no tag_descriptions in state.toml anymore; migration is a no-op.
        let store =
            TomlStore::open(dir.path().to_path_buf(), dir.path().join("state.toml")).unwrap();
        let descs = store.list_tag_descriptions().unwrap();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs.get("@work").map(String::as_str), Some("Work tasks"));
    }

    #[test]
    fn schedule_recurrence_round_trips() {
        use chrono::NaiveDate;
        let (_dir, mut store) = temp_store();
        let mut task = Task::new("Weekly review");
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
            anchor: NaiveDate::from_ymd_opt(2026, 1, 5).unwrap(),
            snap: None,
            snap_leeway: None,
        });
        store.save_task(&task).unwrap();
        let loaded = store.get_task(task.id).unwrap();
        assert!(matches!(
            loaded.recurrence,
            Some(Recurrence::Schedule { ref rrule, .. }) if rrule == "FREQ=WEEKLY;BYDAY=MO"
        ));
    }

    #[test]
    fn schedule_recurrence_rule_alias_deserializes() {
        // Old TOML files used `rule` instead of `rrule`. The alias must allow loading them.
        let toml = r#"
id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
title = "Legacy"
status = "open"
priority = "medium"
created_at = "2026-01-01T00:00:00Z"
updated_at = "2026-01-01T00:00:00Z"

[recurrence]
type = "schedule"
rule = "FREQ=MONTHLY;BYMONTHDAY=1"
anchor = "2026-06-01"
"#;
        let task: Task = toml::from_str(toml).expect("should parse with `rule` alias");
        assert!(matches!(
            task.recurrence,
            Some(Recurrence::Schedule { ref rrule, .. }) if rrule == "FREQ=MONTHLY;BYMONTHDAY=1"
        ));
    }

    // ── snap leeway persistence (T9, T10) ───────────────────────────────────

    /// Saves `task` and returns the bytes actually written to its file.
    ///
    /// The round trip through `Task` proves the value survives; only the file
    /// proves what a *different* binary — or a `git diff` — will see.
    fn saved_file(store: &mut TomlStore, task: &Task) -> String {
        store.save_task(task).unwrap();
        let path = store
            .find_task_file(task.id)
            .unwrap()
            .expect("the task file must exist after save");
        fs::read_to_string(path).unwrap()
    }

    fn rent_task() -> Task {
        let mut task = Task::new("Pay rent");
        task.slug = Some("rent".into());
        task.due = Some(chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap());
        task
    }

    #[test]
    fn a_completion_leeway_is_written_as_its_own_table() {
        let (_dir, mut store) = temp_store();
        let mut task = rent_task();
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: Some(SnapLeeway {
                back: 3,
                forward: Some(3),
            }),
        });

        // The file from the proposal's data model, verbatim.
        let expected = format!(
            "id = \"{}\"\n\
             title = \"Pay rent\"\n\
             status = \"open\"\n\
             priority = \"medium\"\n\
             due = \"2026-06-01\"\n\
             slug = \"rent\"\n\
             \n\
             [recurrence]\n\
             type = \"completion\"\n\
             interval_days = 30\n\
             \n\
             [recurrence.snap]\n\
             type = \"day_of_month\"\n\
             day = 1\n\
             \n\
             [recurrence.snap_leeway]\n\
             back = 3\n\
             forward = 3\n",
            task.id
        );
        assert_eq!(saved_file(&mut store, &task), expected);

        assert_eq!(
            store.get_task(task.id).unwrap().recurrence,
            task.recurrence,
            "the rule must round-trip unchanged"
        );
    }

    /// The claim D4 rests on: a task that never set a leeway is byte-identical
    /// to what it was before the field existed, so an older binary reading a
    /// synced repo sees nothing new.
    #[test]
    fn an_absent_leeway_emits_no_table_at_all() {
        let (_dir, mut store) = temp_store();
        let mut task = rent_task();
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: None,
        });

        let content = saved_file(&mut store, &task);
        assert!(
            !content.contains("snap_leeway"),
            "an unset leeway must not appear in the file:\n{content}"
        );
        assert!(content.contains("[recurrence.snap]"));
        assert_eq!(store.get_task(task.id).unwrap().recurrence, task.recurrence);
    }

    /// `forward` omitted is the only spelling of "unbounded", so it has to
    /// survive as an omission rather than being filled in on write.
    #[test]
    fn a_leeway_without_a_forward_bound_round_trips_as_an_omission() {
        let (_dir, mut store) = temp_store();
        let mut task = rent_task();
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: Some(SnapLeeway {
                back: 3,
                forward: None,
            }),
        });

        let content = saved_file(&mut store, &task);
        assert!(
            content.contains("[recurrence.snap_leeway]\nback = 3\n"),
            "{content}"
        );
        assert!(
            !content.contains("forward"),
            "an unbounded forward must stay absent:\n{content}"
        );
        assert_eq!(store.get_task(task.id).unwrap().recurrence, task.recurrence);
    }

    /// `back = 0` is the default too, so it is skipped — leaving a table that
    /// carries only the forward bound.
    #[test]
    fn a_zero_backward_leeway_is_skipped() {
        let (_dir, mut store) = temp_store();
        let mut task = rent_task();
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: Some(Snap::DayOfMonth { day: 1 }),
            snap_leeway: Some(SnapLeeway {
                back: 0,
                forward: Some(2),
            }),
        });

        let content = saved_file(&mut store, &task);
        assert!(
            content.contains("[recurrence.snap_leeway]\nforward = 2\n"),
            "{content}"
        );
        assert_eq!(store.get_task(task.id).unwrap().recurrence, task.recurrence);
    }

    #[test]
    fn a_schedule_leeway_round_trips_beside_its_anchor() {
        let (_dir, mut store) = temp_store();
        let mut task = Task::new("Weekly review");
        task.recurrence = Some(Recurrence::Schedule {
            rrule: "FREQ=WEEKLY;BYDAY=MO".into(),
            anchor: chrono::NaiveDate::from_ymd_opt(2026, 5, 4).unwrap(),
            snap: Some(Snap::NextWorkday),
            snap_leeway: Some(SnapLeeway {
                back: 2,
                forward: Some(2),
            }),
        });

        let content = saved_file(&mut store, &task);
        assert!(content.contains("[recurrence.snap_leeway]"), "{content}");
        assert_eq!(store.get_task(task.id).unwrap().recurrence, task.recurrence);
    }

    /// T10: a file written by a *newer* binary still loads here.
    ///
    /// Nothing uses `deny_unknown_fields`, so an unrecognised key inside
    /// `snap_leeway` is ignored rather than failing the parse — which is what
    /// keeps one task file from taking down `list` on every older machine in a
    /// synced repo. (Refusing to *write* such a file is a separate guard and
    /// deliberately not part of this change.)
    #[test]
    fn a_leeway_carrying_an_unknown_key_still_loads() {
        let toml = r#"
id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
title = "From a newer binary"
status = "open"
priority = "medium"

[recurrence]
type = "completion"
interval_days = 30

[recurrence.snap]
type = "day_of_month"
day = 1

[recurrence.snap_leeway]
back = 3
forward = 3
sideways = 7
"#;
        let task: Task = toml::from_str(toml).expect("an unknown key must not fail the parse");
        assert_eq!(
            task.recurrence.as_ref().and_then(Recurrence::snap_leeway),
            Some(&SnapLeeway {
                back: 3,
                forward: Some(3)
            })
        );
    }

    /// The whole no-migration claim in one test: a file written before the
    /// field existed is read, written back, and comes out byte-identical.
    ///
    /// Nothing about this feature requires a rewrite pass, a `doctor` step or
    /// a version bump on any existing repository — and if that ever stops
    /// being true, it stops here rather than in someone's git history.
    #[test]
    fn a_pre_leeway_task_file_survives_a_write_unchanged() {
        let (_dir, mut store) = temp_store();
        // Exactly what the previous release emitted for this task.
        let before = "\
id = \"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee\"
title = \"Pay rent\"
status = \"open\"
priority = \"medium\"
due = \"2026-06-01\"
slug = \"rent\"

[recurrence]
type = \"completion\"
interval_days = 30

[recurrence.snap]
type = \"day_of_month\"
day = 1
";
        let path = store.tasks_dir().join("rent.toml");
        fs::write(&path, before).unwrap();

        // Read it through the real store, then save it straight back.
        let task: Task = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            task.recurrence.as_ref().unwrap().snap_leeway().is_none(),
            "an absent table must read as absent, not as a default value"
        );
        store.save_task(&task).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            before,
            "rewriting a pre-leeway task must not change one byte of it"
        );
    }

    /// And the dates it produces are unchanged too — the serialisation being
    /// stable would be cold comfort if the schedule moved.
    #[test]
    fn a_pre_leeway_rule_spawns_the_same_dates_it_always_did() {
        use crate::core::recurrence::{apply_snap, spawn_next};

        let mut task = rent_task();
        let snap = Snap::DayOfMonth { day: 1 };
        task.recurrence = Some(Recurrence::Completion {
            interval_days: 30,
            snap: Some(snap.clone()),
            snap_leeway: None,
        });

        let mut completed = chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        for _ in 0..24 {
            let spawned = spawn_next(&task, completed).unwrap().unwrap();
            let raw = completed + chrono::Duration::days(30);
            assert_eq!(
                spawned.due,
                Some(apply_snap(raw, &snap)),
                "an absent leeway must still be the old forward-only snap"
            );
            completed = spawned.due.unwrap();
        }
    }
}
