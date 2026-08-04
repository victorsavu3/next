//! Shared business-logic layer.
//!
//! These functions implement the core mutations (create, complete, edit) in
//! terms of the [`Store`] and [`VcsBackend`] traits so that both the CLI
//! command handlers and the MCP tool handlers can call exactly the same code.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use uuid::Uuid;

use crate::{
    core::{
        domain::{
            tag,
            task::{Recurrence, Status, Task},
        },
        error::TaskError,
        recurrence::spawn_next,
        resolve::resolve_task_id,
        storage::{self, FileLock},
    },
    Store, VcsBackend,
};

// ── Mutation transaction ──────────────────────────────────────────────────────

/// Opens a mutation transaction over the repository.
///
/// Acquires the re-entrant repository lock and reconciles the store's cache
/// with the on-disk git HEAD, so the read that follows reflects commits made by
/// other processes (closing the lost-update window).  The returned lock guard
/// MUST stay alive for the whole read-modify-write-commit sequence; the nested
/// `save_task` / `commit` calls re-acquire the same lock harmlessly.
///
/// Pair with [`end_mutation`] after the commit succeeds.
pub(crate) fn begin_mutation(
    repo_root: &Path,
    store: &mut dyn Store,
    vcs: &dyn VcsBackend,
) -> anyhow::Result<FileLock> {
    let lock = storage::lock_repo(repo_root)?;
    let head = vcs.head_hash()?;
    store.after_pull(&head)?;
    Ok(lock)
}

/// Closes a mutation transaction: records the post-commit HEAD on the store's
/// cache so the next [`begin_mutation`] does not rebuild unnecessarily.  Must be
/// called while the transaction lock is still held.
pub(crate) fn end_mutation(store: &mut dyn Store, vcs: &dyn VcsBackend) -> anyhow::Result<()> {
    let head = vcs.head_hash()?;
    store.note_head(&head)?;
    Ok(())
}

// ── Validation helpers ────────────────────────────────────────────────────────

/// Validates a user-supplied URL.
///
/// Only `http://` and `https://` schemes are accepted.
pub fn validate_url(u: &str) -> anyhow::Result<()> {
    if u.starts_with("http://") || u.starts_with("https://") {
        Ok(())
    } else {
        anyhow::bail!("url must start with http:// or https://")
    }
}

/// Validates a user-supplied slug.
///
/// Allowed characters: ASCII letters (`a-z`, `A-Z`), digits (`0-9`), hyphen
/// (`-`), and underscore (`_`).
pub fn validate_slug(slug: &str) -> anyhow::Result<()> {
    if slug.is_empty() {
        anyhow::bail!("slug must not be empty");
    }
    if let Some(bad) = slug
        .chars()
        .find(|c| !matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_'))
    {
        anyhow::bail!(
            "slug contains invalid character {bad:?} — only letters, digits, '-' and '_' are allowed"
        );
    }
    Ok(())
}

// ── CreateTaskParams ──────────────────────────────────────────────────────────

/// All optional fields for creating a new task.
///
/// The only required input is the title; everything else is `None` / default.
#[derive(Debug, Default)]
pub struct CreateTaskParams {
    pub due: Option<NaiveDate>,
    pub start: Option<NaiveDate>,
    pub priority: Option<String>,
    pub slug: Option<String>,
    pub assignee: Option<String>,
    /// Tags to attach.  Each must already be validated by the caller, **or**
    /// pass them through [`create_task`] which validates them.
    pub tags: Vec<String>,
    /// Parent task reference (UUID string, slug, or prefix).
    pub parent: Option<String>,
    /// Blocker task references.
    pub blocked_by: Vec<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub notes: Option<String>,
    pub long_term: bool,
    pub score_adjustment: Option<f64>,
    pub recurrence: Option<Recurrence>,
}

// ── EditTaskParams ────────────────────────────────────────────────────────────

/// Describes the field edits to apply to an existing task.
///
/// All fields are optional; only the `Some` / `true` variants are applied.
#[derive(Debug, Default)]
pub struct EditTaskParams {
    pub title: Option<String>,

    pub due: Option<NaiveDate>,
    pub clear_due: bool,

    pub start: Option<NaiveDate>,
    pub clear_start: bool,

    pub priority: Option<String>,
    pub slug: Option<String>,

    pub assignee: Option<String>,
    pub clear_assignee: bool,

    /// Tags to add (already validated).
    pub add_tags: Vec<String>,
    /// Tags to remove.
    pub remove_tags: Vec<String>,

    /// Parent reference (UUID string, slug, or prefix).
    pub parent: Option<String>,
    pub clear_parent: bool,

    /// Blocker references.
    pub blocked_by: Vec<String>,
    pub clear_blocked_by: bool,

    pub description: Option<String>,
    pub clear_description: bool,

    pub url: Option<String>,
    pub clear_url: bool,

    pub notes: Option<String>,

    pub recurrence: Option<Recurrence>,
    pub clear_recurrence: bool,

    pub long_term: Option<bool>,
    pub score_adjustment: Option<f64>,
}

// ── create_task ───────────────────────────────────────────────────────────────

/// Creates a new task, saves it to `store`, and commits it via `vcs`.
///
/// Returns the saved task.
///
/// # Context-tag injection
///
/// When `params.tags` contains no context tag (one starting with `@`), the
/// currently active contexts read from `store` are automatically appended.
pub fn create_task(
    title: String,
    params: CreateTaskParams,
    today: NaiveDate,
    repo_root: &Path,
    store: &mut dyn Store,
    vcs: &dyn VcsBackend,
) -> anyhow::Result<Task> {
    let _txn = begin_mutation(repo_root, store, vcs)?;

    let mut task = Task::new(title);

    task.due = params.due;
    task.start = params.start;

    if let Some(p) = params.priority {
        task.priority = p.parse()?;
    }

    if let Some(ref s) = params.slug {
        validate_slug(s)?;
        task.slug = params.slug.clone();
    }

    task.assignee = params.assignee;
    task.description = params.description;
    task.notes = params.notes;
    task.long_term = params.long_term;

    if let Some(ref u) = params.url {
        validate_url(u)?;
    }
    task.url = params.url;

    if let Some(adj) = params.score_adjustment {
        task.score_adjustment = adj;
    }

    for t in &params.tags {
        tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
    }
    task.tags = params.tags;

    if let Some(parent_ref) = params.parent {
        task.parent_id = Some(resolve_task_id(store, &parent_ref)?);
    }

    for blocker_ref in &params.blocked_by {
        task.blocked_by.push(resolve_task_id(store, blocker_ref)?);
    }

    if let Some(recurrence) = params.recurrence {
        task.recurrence = Some(recurrence);
        task.recurrence_id = Some(task.id);
    }

    // Auto-apply the included context tags when the task has none of its own,
    // so a task captured while working in a context lands in it. Only `@` tags:
    // inheriting an included `#resource` or a freeform label would attach
    // something the user never asked for, and unlike a context that is not
    // recoverable from where they were.
    if !task.tags.iter().any(|t| tag::is_context(t)) {
        let state = store.get_state()?;
        for ctx_tag in state.tags_with(crate::core::domain::state::TagState::Included) {
            if tag::is_context(ctx_tag) {
                task.tags.push(ctx_tag.to_owned());
            }
        }
    }

    let _ = today; // held for future use (e.g. relative date validation)

    let task_path = storage::task_path(repo_root, &task);
    store.save_task(&task)?;
    vcs.commit(&[task_path], &format!("next: add {}", task.title))?;

    end_mutation(store, vcs)?;
    Ok(task)
}

// ── complete_task ─────────────────────────────────────────────────────────────

/// Marks task `id` as done, spawns a recurrence instance when applicable,
/// saves both to `store`, and commits via `vcs`.
///
/// Returns the completed task (not the spawned instance, if any).
pub fn complete_task(
    id: Uuid,
    completion_date: NaiveDate,
    repo_root: &Path,
    store: &mut dyn Store,
    vcs: &dyn VcsBackend,
) -> anyhow::Result<Task> {
    let _txn = begin_mutation(repo_root, store, vcs)?;

    let mut task = store.get_task(id)?;

    // Only active (Open/Started) tasks may be completed. Re-completing an
    // already-resolved task would re-mark it done and spawn a duplicate
    // recurrence instance, so reject it with a clear error instead.
    if !task.is_active() {
        let msg = match task.status {
            Status::Done => format!("task {id} is already done"),
            Status::Cancelled => format!("cannot complete task {id}: it is cancelled"),
            // Unreachable: is_active() is true for Open/Started.
            Status::Open | Status::Started => unreachable!(),
        };
        return Err(TaskError::Other(msg).into());
    }

    task.mark_done(completion_date);

    let task_path = storage::task_path(repo_root, &task);
    let mut paths: Vec<PathBuf> = vec![task_path];

    if let Some(next) = spawn_next(&task, completion_date)? {
        let next_path = storage::task_path(repo_root, &next);
        store.save_task(&next)?;
        paths.push(next_path);
    }

    store.save_task(&task)?;
    vcs.commit(&paths, &format!("next: done {}", task.title))?;

    end_mutation(store, vcs)?;
    Ok(task)
}

// ── apply_edits ───────────────────────────────────────────────────────────────

/// Applies `edits` to task `id`, saves the result to `store`, and commits via
/// `vcs`.
///
/// Returns the updated task.
pub fn apply_edits(
    id: Uuid,
    edits: EditTaskParams,
    today: NaiveDate,
    repo_root: &Path,
    store: &mut dyn Store,
    vcs: &dyn VcsBackend,
) -> anyhow::Result<Task> {
    let _txn = begin_mutation(repo_root, store, vcs)?;

    // Editing an archived task pulls it back into the active tier first;
    // the touched segment joins this edit's commit.
    let resurrected_segment = crate::core::archiver::resurrect_if_archived(store, repo_root, id)?;

    let mut task = store.get_task(id)?;

    // A title or slug edit renames the file on disk; the old path must be
    // committed as a deletion or it stays tracked in git and resurfaces as a
    // duplicate task on every other machine's next pull.
    let path_before_edits = storage::task_path(repo_root, &task);

    if let Some(title) = edits.title {
        task.title = title;
    }

    if edits.clear_due {
        task.due = None;
    } else if let Some(d) = edits.due {
        task.due = Some(d);
    }

    if edits.clear_start {
        task.start = None;
    } else if let Some(s) = edits.start {
        task.start = Some(s);
    }

    if let Some(p) = edits.priority {
        task.priority = p.parse()?;
    }

    if let Some(slug) = edits.slug {
        validate_slug(&slug)?;
        task.slug = Some(slug);
    }

    if edits.clear_assignee {
        task.assignee = None;
    } else if let Some(a) = edits.assignee {
        task.assignee = Some(a);
    }

    if edits.clear_description {
        task.description = None;
    } else if let Some(d) = edits.description {
        task.description = Some(d);
    }

    if edits.clear_url {
        task.url = None;
    } else if let Some(ref u) = edits.url {
        validate_url(u)?;
        task.url = edits.url;
    }

    if let Some(notes) = edits.notes {
        task.notes = Some(notes);
    }

    if let Some(adj) = edits.score_adjustment {
        task.score_adjustment = adj;
    }

    if let Some(lt) = edits.long_term {
        task.long_term = lt;
    }

    // ── Tags ──────────────────────────────────────────────────────────────────
    for t in &edits.add_tags {
        tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
        if !task.tags.contains(t) {
            task.tags.push(t.clone());
        }
    }
    for t in &edits.remove_tags {
        task.tags.retain(|existing| existing != t);
    }

    // ── Parent ────────────────────────────────────────────────────────────────
    if edits.clear_parent {
        task.parent_id = None;
    } else if let Some(parent_ref) = edits.parent {
        task.parent_id = Some(resolve_task_id(store, &parent_ref)?);
    }

    // ── Blocked-by ────────────────────────────────────────────────────────────
    if edits.clear_blocked_by {
        task.blocked_by.clear();
    } else {
        for blocker_ref in &edits.blocked_by {
            let bid = resolve_task_id(store, blocker_ref)?;
            if !task.blocked_by.contains(&bid) {
                task.blocked_by.push(bid);
            }
        }
    }

    // ── Recurrence ────────────────────────────────────────────────────────────
    if edits.clear_recurrence {
        task.recurrence = None;
        task.recurrence_id = None;
    } else if let Some(recurrence) = edits.recurrence {
        task.recurrence = Some(recurrence);
        task.recurrence_id.get_or_insert(task.id);
    }

    let _ = today; // held for future use

    let new_path = storage::task_path(repo_root, &task);
    let mut paths = vec![new_path.clone()];
    if path_before_edits != new_path {
        paths.push(path_before_edits);
    }
    paths.extend(resurrected_segment);
    store.save_task(&task)?;
    vcs.commit(&paths, &format!("next: edit {}", task.title))?;

    end_mutation(store, vcs)?;
    Ok(task)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskRepository;
    use tempfile::TempDir;

    fn make_ctx() -> (TempDir, TaskRepository) {
        let dir = tempfile::tempdir().unwrap();
        crate::core::test_git::init_test_repo(dir.path());
        let (store, vcs) = crate::core::storage::open(dir.path().to_path_buf()).unwrap();
        let ctx =
            TaskRepository::with_parts(Box::new(store), Box::new(vcs), dir.path().to_path_buf());
        (dir, ctx)
    }

    fn today() -> NaiveDate {
        chrono::Local::now().date_naive()
    }

    // ── create_task ───────────────────────────────────────────────────────────

    #[test]
    fn create_task_basic() {
        let (_dir, mut ctx) = make_ctx();
        let task = create_task(
            "Buy milk".into(),
            CreateTaskParams::default(),
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();
        assert_eq!(task.title, "Buy milk");
        // Round-trip: should be retrievable from the store.
        let loaded = ctx.store.get_task(task.id).unwrap();
        assert_eq!(loaded.title, "Buy milk");
    }

    #[test]
    fn create_task_validates_url() {
        let (_dir, mut ctx) = make_ctx();
        let params = CreateTaskParams {
            url: Some("ftp://not-allowed".into()),
            ..Default::default()
        };
        let err = create_task(
            "X".into(),
            params,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap_err();
        assert!(err.to_string().contains("http"));
    }

    #[test]
    fn create_task_validates_slug() {
        let (_dir, mut ctx) = make_ctx();
        let params = CreateTaskParams {
            slug: Some("bad slug!".into()),
            ..Default::default()
        };
        let err = create_task(
            "X".into(),
            params,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid character"));
    }

    #[test]
    fn create_task_injects_active_context() {
        use crate::core::domain::state::TagState;

        let (_dir, mut ctx) = make_ctx();
        // Include a context tag, plus tags of the other two kinds that must
        // NOT be inherited: only a context says "where I am working".
        let mut state = ctx.store.get_state().unwrap();
        state.set_state("@work", Some(TagState::Included));
        state.set_state("#printer", Some(TagState::Included));
        state.set_state("urgent", Some(TagState::Included));
        ctx.store.save_state(&state).unwrap();

        let task = create_task(
            "Meeting".into(),
            CreateTaskParams::default(),
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();
        assert!(
            task.tags.contains(&"@work".to_owned()),
            "context tag should be injected"
        );
        assert!(
            !task.tags.iter().any(|t| t == "#printer" || t == "urgent"),
            "only contexts are inherited: {:?}",
            task.tags
        );
    }

    #[test]
    fn create_task_no_injection_when_context_tag_present() {
        use crate::core::domain::state::TagState;

        let (_dir, mut ctx) = make_ctx();
        let mut state = ctx.store.get_state().unwrap();
        state.set_state("@work", Some(TagState::Included));
        ctx.store.save_state(&state).unwrap();

        let params = CreateTaskParams {
            tags: vec!["@home".into()],
            ..Default::default()
        };
        let task = create_task(
            "Personal task".into(),
            params,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();
        // @work should NOT be injected because @home is already a context tag.
        assert!(!task.tags.contains(&"@work".to_owned()));
        assert!(task.tags.contains(&"@home".to_owned()));
    }

    // ── complete_task ─────────────────────────────────────────────────────────

    #[test]
    fn complete_task_marks_done() {
        let (_dir, mut ctx) = make_ctx();
        let task = create_task(
            "Write report".into(),
            CreateTaskParams::default(),
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        let completed = complete_task(
            task.id,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        use crate::core::domain::task::Status;
        assert_eq!(completed.status, Status::Done);
        assert_eq!(completed.completed_at, Some(today()));
        // The recorded date must survive a round-trip through the store.
        assert_eq!(
            ctx.store.get_task(task.id).unwrap().completed_at,
            Some(today())
        );
    }

    #[test]
    fn complete_task_records_backdated_completion_date() {
        let (_dir, mut ctx) = make_ctx();
        let task = create_task(
            "Backdated".into(),
            CreateTaskParams::default(),
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        let when = today() - chrono::Duration::days(3);
        let completed = complete_task(
            task.id,
            when,
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        assert_eq!(completed.completed_at, Some(when));
    }

    #[test]
    fn complete_task_spawns_recurrence_instance() {
        use crate::core::domain::task::Recurrence;

        let (_dir, mut ctx) = make_ctx();
        let params = CreateTaskParams {
            recurrence: Some(Recurrence::Completion {
                interval_days: 7,
                snap: None,
            }),
            ..Default::default()
        };
        let task = create_task(
            "Weekly review".into(),
            params,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        complete_task(
            task.id,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        let all = ctx.store.list_tasks().unwrap();
        assert_eq!(all.len(), 2, "original + spawned next instance");
    }

    // ── apply_edits ───────────────────────────────────────────────────────────

    #[test]
    fn apply_edits_title() {
        let (_dir, mut ctx) = make_ctx();
        let task = create_task(
            "Old title".into(),
            CreateTaskParams::default(),
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        let edits = EditTaskParams {
            title: Some("New title".into()),
            ..Default::default()
        };
        let updated = apply_edits(
            task.id,
            edits,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        assert_eq!(updated.title, "New title");
    }

    #[test]
    fn apply_edits_add_and_remove_tags() {
        let (_dir, mut ctx) = make_ctx();
        let params = CreateTaskParams {
            tags: vec!["@work".into(), "#laptop".into()],
            ..Default::default()
        };
        let task = create_task(
            "Tag test".into(),
            params,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        let edits = EditTaskParams {
            add_tags: vec!["urgent".into()],
            remove_tags: vec!["#laptop".into()],
            ..Default::default()
        };
        let updated = apply_edits(
            task.id,
            edits,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        assert!(updated.tags.contains(&"urgent".to_owned()));
        assert!(!updated.tags.contains(&"#laptop".to_owned()));
        assert!(updated.tags.contains(&"@work".to_owned()));
    }

    #[test]
    fn apply_edits_clear_fields() {
        use chrono::NaiveDate;
        let (_dir, mut ctx) = make_ctx();
        let params = CreateTaskParams {
            due: Some(NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()),
            description: Some("desc".into()),
            url: Some("https://example.com".into()),
            assignee: Some("alice".into()),
            ..Default::default()
        };
        let task = create_task(
            "Clear test".into(),
            params,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        let edits = EditTaskParams {
            clear_due: true,
            clear_description: true,
            clear_url: true,
            clear_assignee: true,
            ..Default::default()
        };
        let updated = apply_edits(
            task.id,
            edits,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        assert!(updated.due.is_none());
        assert!(updated.description.is_none());
        assert!(updated.url.is_none());
        assert!(updated.assignee.is_none());
    }

    #[test]
    fn apply_edits_validates_url() {
        let (_dir, mut ctx) = make_ctx();
        let task = create_task(
            "URL check".into(),
            CreateTaskParams::default(),
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap();

        let edits = EditTaskParams {
            url: Some("ftp://bad".into()),
            ..Default::default()
        };
        let err = apply_edits(
            task.id,
            edits,
            today(),
            &ctx.repo_root.clone(),
            &mut *ctx.store,
            &*ctx.vcs,
        )
        .unwrap_err();
        assert!(err.to_string().contains("http"));
    }
}
