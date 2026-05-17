use std::{
    fs,
    path::PathBuf,
};

use next::{
    domain::{state::GlobalState, task::Task},
    error::{AppError, Result},
    store::Store,
};
use uuid::Uuid;

pub struct TomlStore {
    root: PathBuf,
}

impl TomlStore {
    /// Opens (or initialises) a store rooted at `root`.
    /// Creates `tasks/` if it does not exist.
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("tasks"))?;
        Ok(Self { root })
    }

    fn tasks_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.toml")
    }

    /// Canonical filename for a task (no directory prefix).
    pub(crate) fn task_filename(task: &Task) -> String {
        if let Some(ref slug) = task.slug {
            format!("{slug}.toml")
        } else {
            let title_part = title_to_slug(&task.title);
            let uuid_hex = task.id.to_string().replace('-', "");
            format!("{title_part}-{}.toml", &uuid_hex[..8])
        }
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
            // Skip unreadable or malformed files; only propagate IO errors.
            let content = match fs::read_to_string(&path) {
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
        let mut tasks = Vec::new();
        for entry in fs::read_dir(self.tasks_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let content = fs::read_to_string(&path)?;
            let task = toml::from_str::<Task>(&content).map_err(|e| {
                AppError::Other(format!("parse error in {}: {e}", path.display()))
            })?;
            tasks.push(task);
        }
        Ok(tasks)
    }
}

/// Converts a task title to a URL-safe slug component:
/// lowercase, runs of non-alphanumeric chars collapsed to a single `-`.
fn title_to_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = true; // suppress leading dash
    for c in title.chars() {
        if c.is_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    // strip trailing dash
    slug.trim_end_matches('-').to_owned()
}

impl Store for TomlStore {
    fn get_task(&self, id: Uuid) -> Result<Task> {
        match self.find_task_file(id)? {
            Some(path) => {
                let content = fs::read_to_string(&path)?;
                toml::from_str::<Task>(&content).map_err(|e| {
                    AppError::Other(format!("parse error in {}: {e}", path.display()))
                })
            }
            None => Err(AppError::TaskNotFound(id.to_string())),
        }
    }

    fn get_task_by_slug(&self, slug: &str) -> Result<Option<Task>> {
        // Fast path: slug-named files live at a predictable location.
        let candidate = self.tasks_dir().join(format!("{slug}.toml"));
        if candidate.exists() {
            let content = fs::read_to_string(&candidate)?;
            if let Ok(task) = toml::from_str::<Task>(&content) {
                if task.slug.as_deref() == Some(slug) {
                    return Ok(Some(task));
                }
            }
        }
        // Fallback: linear scan (handles stale or renamed files).
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
        // Reject slug conflicts with other tasks.
        if let Some(ref slug) = task.slug {
            if let Some(existing) = self.get_task_by_slug(slug)? {
                if existing.id != task.id {
                    return Err(AppError::SlugConflict(slug.clone()));
                }
            }
        }

        let new_filename = Self::task_filename(task);
        let new_path = self.tasks_dir().join(&new_filename);

        // Remove stale file when the name changes (slug added/changed, title changed).
        if let Some(old_path) = self.find_task_file(task.id)? {
            if old_path != new_path {
                fs::remove_file(&old_path)?;
            }
        }

        let content = toml::to_string_pretty(task)
            .map_err(|e| AppError::Other(format!("TOML serialization error: {e}")))?;
        fs::write(new_path, content)?;
        Ok(())
    }

    fn delete_task(&mut self, id: Uuid) -> Result<()> {
        match self.find_task_file(id)? {
            Some(path) => Ok(fs::remove_file(path)?),
            None => Err(AppError::TaskNotFound(id.to_string())),
        }
    }

    fn get_task_by_forgejo_issue(&self, url: &str) -> Result<Option<Task>> {
        Ok(self
            .read_all_tasks()?
            .into_iter()
            .find(|t| t.forgejo_issue.as_deref() == Some(url)))
    }

    fn get_task_by_webcal_uid(&self, uid: &str) -> Result<Option<Task>> {
        Ok(self
            .read_all_tasks()?
            .into_iter()
            .find(|t| t.webcal_uid.as_deref() == Some(uid)))
    }

    fn get_state(&self) -> Result<GlobalState> {
        let path = self.state_path();
        if !path.exists() {
            return Ok(GlobalState::default());
        }
        let content = fs::read_to_string(&path)?;
        toml::from_str::<GlobalState>(&content)
            .map_err(|e| AppError::Other(format!("parse error in state.toml: {e}")))
    }

    fn save_state(&mut self, state: &GlobalState) -> Result<()> {
        let content = toml::to_string_pretty(state)
            .map_err(|e| AppError::Other(format!("TOML serialization error: {e}")))?;
        fs::write(self.state_path(), content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use next::domain::task::{Priority, Recurrence, Stage, Status};

    use super::*;

    fn temp_store() -> (tempfile::TempDir, TomlStore) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = TomlStore::open(dir.path().to_path_buf()).unwrap();
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
        assert_eq!(loaded.stage, Stage::Inbox);
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
        task.stage = Stage::Project;
        task.tags = vec!["@work".into(), "$laptop".into()];
        task.notes = Some("Some notes".into());
        task.score_adjustment = 1.5;
        task.long_term = true;
        task.recurrence = Some(Recurrence::Completion { interval_days: 7 });

        store.save_task(&task).unwrap();
        let loaded = store.get_task(task.id).unwrap();

        assert_eq!(loaded.slug, task.slug);
        assert_eq!(loaded.due, task.due);
        assert_eq!(loaded.start, task.start);
        assert_eq!(loaded.priority, task.priority);
        assert_eq!(loaded.stage, task.stage);
        assert_eq!(loaded.tags, task.tags);
        assert_eq!(loaded.notes, task.notes);
        assert_eq!(loaded.score_adjustment, task.score_adjustment);
        assert!(loaded.long_term);
        assert!(matches!(
            loaded.recurrence,
            Some(Recurrence::Completion { interval_days: 7 })
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
        // Use all zeroes — astronomically unlikely to match a real UUID.
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
        assert!(matches!(err, AppError::TaskNotFound(_)));
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
        assert!(matches!(err, AppError::SlugConflict(_)));
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

        // Default state when file is absent.
        let default = store.get_state().unwrap();
        assert!(default.active_contexts.is_empty());

        let mut state = GlobalState::default();
        state.active_contexts = vec!["@work".into(), "@home".into()];
        state.resources = HashMap::from([("printer".into(), false)]);
        store.save_state(&state).unwrap();

        let loaded = store.get_state().unwrap();
        assert_eq!(loaded.active_contexts, state.active_contexts);
        assert_eq!(loaded.resources["printer"], false);
    }

    #[test]
    fn title_to_slug_basic() {
        assert_eq!(title_to_slug("Buy milk"), "buy-milk");
        assert_eq!(title_to_slug("  leading spaces"), "leading-spaces");
        assert_eq!(title_to_slug("Hello, World!"), "hello-world");
        assert_eq!(title_to_slug("abc123"), "abc123");
        assert_eq!(title_to_slug("a--b"), "a-b");
    }

    #[test]
    fn schedule_recurrence_round_trips() {
        let (_dir, mut store) = temp_store();
        let mut task = Task::new("Weekly review");
        task.recurrence = Some(Recurrence::Schedule {
            rule: "every Monday".into(),
        });
        store.save_task(&task).unwrap();
        let loaded = store.get_task(task.id).unwrap();
        assert!(matches!(
            loaded.recurrence,
            Some(Recurrence::Schedule { rule }) if rule == "every Monday"
        ));
    }
}
