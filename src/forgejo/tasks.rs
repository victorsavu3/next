//! Task access for the Forgejo plugin, backed by the `next` library directly
//! (the plugin is in-crate, so no subprocess).
//!
//! [`TaskStore`] is the interface the reconcile logic uses; [`LibTaskStore`]
//! wraps an [`TaskRepository`] and an in-memory fake is used in tests.

use std::path::Path;

use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

use crate::core::{
    domain::task::Task,
    error::TaskError,
    plugin::registry,
    service::{complete_task, create_task, CreateTaskParams},
    storage, TaskRepository,
};

use super::{issues::ForgejoIssue, keys, PLUGIN_NAME};

/// The task operations the reconcile logic needs.
pub trait TaskStore {
    /// All tasks whose `__forgejo-repo` equals `repo` (`owner/repo`).
    fn list_linked(&self, repo: &str) -> Result<Vec<Task>>;
    /// Creates a task from an issue (title, url, body, the mapped context tag,
    /// and the `__forgejo-*` link data), subscribes the plugin to it, and
    /// returns its id.
    fn create_from_issue(
        &mut self,
        issue: &ForgejoIssue,
        context: &str,
        repo: &str,
    ) -> Result<Uuid>;
    /// Marks a task done (and spawns any recurrence instance).
    fn mark_done(&mut self, id: Uuid) -> Result<()>;
    /// Fetches a task by id, or `None` if it no longer exists.
    fn show(&self, id: Uuid) -> Result<Option<Task>>;
}

/// Returns the `(repo, issue_number)` a task is linked to, if any.
pub fn forgejo_link(task: &Task) -> Option<(String, i64)> {
    let repo = task.data.get(keys::REPO)?.as_str()?.to_owned();
    let issue = task.data.get(keys::ISSUE)?.as_i64()?;
    Some((repo, issue))
}

/// Real [`TaskStore`] backed by an [`TaskRepository`] on the `next` repository.
pub struct LibTaskStore {
    ctx: TaskRepository,
}

impl LibTaskStore {
    /// Opens the store on `repo` (required — config-file resolution is CLI-only).
    pub fn open(repo: Option<&Path>) -> Result<Self> {
        let root = repo.ok_or_else(|| {
            anyhow::anyhow!(
                "no next repository configured — set next_repo in the plugin config or NEXT_REPO"
            )
        })?;
        Ok(Self {
            ctx: TaskRepository::open(root.to_path_buf())?,
        })
    }

    /// The repository root this store operates on.
    pub fn repo_root(&self) -> &Path {
        &self.ctx.repo_root
    }

    /// Pulls from the remote and reconciles the cache (best-effort).
    pub fn git_pull(&mut self) -> Result<()> {
        crate::core::sync(&mut self.ctx, false, true)?;
        Ok(())
    }

    /// Pushes local commits to the remote (best-effort).
    pub fn git_push(&mut self) -> Result<()> {
        crate::core::sync(&mut self.ctx, true, false)?;
        Ok(())
    }
}

impl TaskStore for LibTaskStore {
    fn list_linked(&self, repo: &str) -> Result<Vec<Task>> {
        Ok(self
            .ctx
            .store
            .list_tasks()?
            .into_iter()
            .filter(|t| t.data.get(keys::REPO).and_then(|v| v.as_str()) == Some(repo))
            .collect())
    }

    fn create_from_issue(
        &mut self,
        issue: &ForgejoIssue,
        context: &str,
        repo: &str,
    ) -> Result<Uuid> {
        let today = chrono::Local::now().date_naive();
        let params = CreateTaskParams {
            tags: vec![context.to_owned()],
            url: (!issue.html_url.is_empty()).then(|| issue.html_url.clone()),
            description: (!issue.body.is_empty()).then(|| issue.body.clone()),
            ..Default::default()
        };
        let repo_root = self.ctx.repo_root.clone();
        let task = create_task(
            issue.title.clone(),
            params,
            today,
            &repo_root,
            &mut *self.ctx.store,
            &*self.ctx.vcs,
        )?;
        let id = task.id;

        // Attach the link data in a single follow-up transaction.
        let repo = repo.to_owned();
        let number = issue.number;
        let url = issue.html_url.clone();
        let labels = issue.labels.clone();
        self.ctx.transaction(|store, vcs, root| {
            let mut t = store.get_task(id)?;
            t.data.insert(keys::REPO.into(), json!(repo));
            t.data.insert(keys::ISSUE.into(), json!(number));
            if !url.is_empty() {
                t.data.insert(keys::URL.into(), json!(url));
            }
            if !labels.is_empty() {
                t.data.insert(keys::LABELS.into(), json!(labels));
            }
            let path = storage::task_path(root, &t);
            store.save_task(&t)?;
            vcs.commit(&[path], &format!("next: forgejo link {repo}#{number}"))?;
            Ok(())
        })?;

        // Subscribe the plugin so local resolution of this task notifies us.
        registry::watch(&repo_root, PLUGIN_NAME, id)?;
        Ok(id)
    }

    fn mark_done(&mut self, id: Uuid) -> Result<()> {
        let today = chrono::Local::now().date_naive();
        let repo_root = self.ctx.repo_root.clone();
        complete_task(id, today, &repo_root, &mut *self.ctx.store, &*self.ctx.vcs)?;
        Ok(())
    }

    fn show(&self, id: Uuid) -> Result<Option<Task>> {
        match self.ctx.store.get_task(id) {
            Ok(t) => Ok(Some(t)),
            Err(TaskError::TaskNotFound(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgejo_link_reads_data() {
        let mut task = Task::new("t");
        assert_eq!(forgejo_link(&task), None);
        task.data
            .insert(keys::REPO.into(), json!("victor/task-manager"));
        task.data.insert(keys::ISSUE.into(), json!(42));
        assert_eq!(
            forgejo_link(&task),
            Some(("victor/task-manager".to_owned(), 42))
        );
    }

    #[test]
    fn forgejo_link_requires_both_keys() {
        let mut task = Task::new("t");
        task.data.insert(keys::REPO.into(), json!("victor/x"));
        assert_eq!(forgejo_link(&task), None, "needs the issue number too");
    }
}
