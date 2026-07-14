use std::{collections::HashMap, path::PathBuf};

use uuid::Uuid;

use crate::core::{
    domain::{state::GlobalState, tag::TagMeta, task::{Status, Task}},
    error::Result,
};

/// Default number of items per [`Page`] when a query does not set one.
pub const DEFAULT_PAGE_SIZE: u32 = 1000;

/// A storage-level task query, compiled to SQL by the cached store.
///
/// Carries the *cheap* filter gates — the ones expressible as indexed column
/// and tag-table predicates. Relational gates (open blockers, parent with
/// open children, resource availability) stay in `domain::filter`, applied to
/// the already-reduced result.
#[derive(Debug, Clone)]
pub struct TaskQuery {
    /// Keep only tasks whose status is in the list. `None` = any status.
    pub statuses: Option<Vec<Status>>,
    /// `false` — only active-tier tasks; `true` — only archived tasks.
    pub archived: bool,
    /// Task must carry every listed tag (a parent segment matches descendants).
    pub required_tags: Vec<String>,
    /// Task must carry none of the listed tags (same descendant semantics).
    pub excluded_tags: Vec<String>,
    /// 1-indexed page to return; values below 1 are treated as 1.
    pub page: u32,
    /// Items per page; 0 falls back to [`DEFAULT_PAGE_SIZE`].
    pub page_size: u32,
}

impl Default for TaskQuery {
    fn default() -> Self {
        Self {
            statuses: None,
            archived: false,
            required_tags: Vec::new(),
            excluded_tags: Vec::new(),
            page: 1,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }
}

impl TaskQuery {
    /// Whether `task` (assumed active-tier) passes this query's filter gates.
    /// The reference semantics that SQL-backed implementations must match.
    pub fn matches(&self, task: &Task) -> bool {
        if self.archived {
            // A plain store has no archive tier.
            return false;
        }
        if let Some(statuses) = &self.statuses {
            if !statuses.contains(&task.status) {
                return false;
            }
        }
        if !self.required_tags.iter().all(|req| task.tags.iter().any(|t| tag_matches(t, req))) {
            return false;
        }
        if self.excluded_tags.iter().any(|exc| task.tags.iter().any(|t| tag_matches(t, exc))) {
            return false;
        }
        true
    }
}

/// Whether `task_tag` equals `filter` or is a descendant of it
/// (`@work/frontend` matches the filter `@work`).
pub fn tag_matches(task_tag: &str, filter: &str) -> bool {
    task_tag == filter
        || (task_tag.len() > filter.len()
            && task_tag.starts_with(filter)
            && task_tag.as_bytes()[filter.len()] == b'/')
}

/// One page of query results, with enough metadata for the caller to tell a
/// complete result from a truncated one.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// The 1-indexed page these items belong to.
    pub page: u32,
    pub page_size: u32,
    /// Matching items before pagination.
    pub total: u64,
}

/// Result of a `VcsBackend::pull` operation.
#[derive(Debug, Clone)]
pub enum PullResult {
    /// Pull completed without conflicts.
    Clean,
    /// Pull produced merge conflicts in these files; the user must resolve them.
    Conflicts(Vec<PathBuf>),
}

/// Persistent storage for tasks and global state.
///
/// A "project" is simply a task that has children via `parent_id`. There is no
/// separate project type or storage path — all tasks are stored and retrieved
/// through the same methods.
///
/// Reads use `&self`; writes use `&mut self`. Implementations may use interior
/// mutability internally (e.g. a SQLite connection behind a Mutex) but the trait
/// itself is explicit about mutability to keep the interface honest.
///
/// The canonical implementation in `next-storage` stores tasks as TOML files
/// (source of truth) backed by an SQLite cache for fast queries.
/// `next-test-utils` provides an in-memory implementation for unit tests.
pub trait Store: Send + Sync {
    // --- Tasks ---

    fn get_task(&self, id: Uuid) -> Result<Task>;

    /// Returns the task with the given user-provided slug, if any.
    /// Slugs are unique across all tasks.
    fn get_task_by_slug(&self, slug: &str) -> Result<Option<Task>>;

    /// Returns all tasks whose UUID string starts with `prefix`.
    /// Used for short-ID disambiguation in the CLI (minimum 4 hex chars).
    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>>;

    /// Returns all tasks regardless of status or stage.
    /// Filtering and scoring are applied in the domain layer.
    fn list_tasks(&self) -> Result<Vec<Task>>;

    fn save_task(&mut self, task: &Task) -> Result<()>;

    fn delete_task(&mut self, id: Uuid) -> Result<()>;

    // --- Global state ---

    fn get_state(&self) -> Result<GlobalState>;

    fn save_state(&mut self, state: &GlobalState) -> Result<()>;

    // --- Tag metadata ---
    //
    // Each tag's metadata is stored as an individual file (`tags/<tag>.toml`)
    // so that different tags can be committed and synced independently.
    // Slashes in tag names (e.g. `@home/kitchen`) map to actual subdirectories.

    /// Returns the full metadata for `tag`, or `None` if no file exists.
    fn get_tag_meta(&self, tag: &str) -> Result<Option<TagMeta>>;

    /// Writes (or replaces) the full metadata for `tag`.
    fn set_tag_meta(&mut self, tag: &str, meta: TagMeta) -> Result<()>;

    /// Deletes the metadata file for `tag`. Errors if no file exists.
    fn delete_tag_meta(&mut self, tag: &str) -> Result<()>;

    /// Returns all tag metadata as a map of tag → [`TagMeta`].
    fn list_tag_metas(&self) -> Result<HashMap<String, TagMeta>>;

    // --- Convenience wrappers for description-only operations ---

    /// Returns the description for `tag`, or `None` if not set.
    fn get_tag_description(&self, tag: &str) -> Result<Option<String>> {
        Ok(self.get_tag_meta(tag)?.and_then(|m| m.description))
    }

    /// Sets or replaces the description for `tag`, preserving other metadata.
    fn set_tag_description(&mut self, tag: &str, description: &str) -> Result<()> {
        let mut meta = self.get_tag_meta(tag)?.unwrap_or_default();
        meta.description = Some(description.to_owned());
        self.set_tag_meta(tag, meta)
    }

    /// Removes the description field for `tag`. Errors if no metadata file exists.
    fn delete_tag_description(&mut self, tag: &str) -> Result<()> {
        let mut meta = self
            .get_tag_meta(tag)?
            .ok_or_else(|| crate::core::error::TaskError::Other(format!("no description set for tag {tag:?}")))?;
        meta.description = None;
        if meta == TagMeta::default() {
            self.delete_tag_meta(tag)
        } else {
            self.set_tag_meta(tag, meta)
        }
    }

    /// Returns all tag descriptions as a map of tag → description.
    fn list_tag_descriptions(&self) -> Result<HashMap<String, String>> {
        Ok(self
            .list_tag_metas()?
            .into_iter()
            .filter_map(|(tag, meta)| meta.description.map(|d| (tag, d)))
            .collect())
    }

    /// Reconciles any HEAD-keyed cache with `new_head`, rebuilding when it
    /// differs from the stored hash.
    ///
    /// Called after a VCS pull and at the start of a mutation transaction so the
    /// subsequent read reflects commits made by other processes.  The default
    /// implementation is a no-op for stores that need no special handling.
    fn after_pull(&mut self, _new_head: &str) -> Result<()> {
        Ok(())
    }

    /// Records `new_head` as the cache's current HEAD *without* rebuilding.
    ///
    /// Called at the end of a mutation transaction: the writes were already
    /// applied to the cache in-place by `save_task` / `save_state`, so the cache
    /// is current and a rebuild would be wasted work — but the stored HEAD hash
    /// must advance to match the new commit, otherwise the next [`after_pull`]
    /// would see a mismatch and rebuild unnecessarily.  Default: no-op.
    fn note_head(&mut self, _new_head: &str) -> Result<()> {
        Ok(())
    }

    /// Git-derived creation/update timestamps for all cached tasks, keyed by
    /// task id. Maintained by the storage layer from git history and used by
    /// scoring (age factor). Default: empty, for stores without a date index.
    fn task_dates(&self) -> Result<HashMap<Uuid, crate::core::scoring::TaskDates>> {
        Ok(HashMap::new())
    }

    /// Returns the tasks matching `q`, paginated.
    ///
    /// The default implementation filters `list_tasks()` in memory and is the
    /// reference semantics; indexed stores (the SQLite cache) override it with
    /// a pushed-down query. Result order is stable across implementations and
    /// pages: unresolved tasks first (by id), then by most recent completion.
    fn query_tasks(&self, q: &TaskQuery) -> Result<Page<Task>> {
        let mut items: Vec<Task> = self
            .list_tasks()?
            .into_iter()
            .filter(|t| q.matches(t))
            .collect();
        sort_for_query(&mut items);
        let total = items.len() as u64;
        let page_size = if q.page_size == 0 { DEFAULT_PAGE_SIZE } else { q.page_size };
        let page = q.page.max(1);
        let offset = (page as usize - 1).saturating_mul(page_size as usize);
        let items: Vec<Task> = items.into_iter().skip(offset).take(page_size as usize).collect();
        Ok(Page { items, page, page_size, total })
    }
}

/// Sorts tasks into the canonical query order: unresolved tasks first (by
/// id), then resolved tasks by most recent completion, ties broken by id.
/// Deterministic, so consecutive pages never overlap or skip.
pub fn sort_for_query(tasks: &mut [Task]) {
    use std::cmp::Ordering;
    tasks.sort_by(|a, b| match (a.completed_at, b.completed_at) {
        (None, None) => a.id.cmp(&b.id),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => y.cmp(&x).then_with(|| a.id.cmp(&b.id)),
    });
}

/// Version-control backend.
///
/// Separated from `Store` so command handlers that do not perform git operations
/// (scoring, filtering, etc.) can be tested without a VCS stub.
pub trait VcsBackend: Send + Sync {
    /// Stages `paths` and creates a commit with the given message.
    fn commit(&self, paths: &[PathBuf], message: &str) -> Result<()>;

    fn pull(&self) -> Result<PullResult>;

    fn push(&self) -> Result<()>;

    /// Returns the SHA-1 hex string of the current HEAD commit.
    /// Used by the cache layer to detect whether a rebuild is needed.
    fn head_hash(&self) -> Result<String>;

    /// Returns the working-tree diff relative to HEAD.
    ///
    /// The string contains `git status --short` output followed by the full
    /// `git diff HEAD` patch. Conflicted files appear in both sections.
    /// Default: returns an error (not supported by non-git backends).
    fn diff(&self) -> Result<String> {
        Err(crate::core::error::TaskError::Other("diff not supported by this backend".into()))
    }

    /// Fetches from the default remote and hard-resets the working tree to
    /// `FETCH_HEAD`, discarding all local changes and resolving any conflicts.
    ///
    /// Returns the new HEAD SHA-1 hex string so the caller can update caches.
    /// Default: returns an error (not supported by non-git backends).
    fn force_pull(&self) -> Result<String> {
        Err(crate::core::error::TaskError::Other("force_pull not supported by this backend".into()))
    }
}
