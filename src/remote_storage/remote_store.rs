//! [`RemoteStore`] — [`Store`] implementation that calls a hosted `next-mcp` server.
//!
//! Every method is currently a stub returning "not yet implemented". Each stub
//! should be replaced with a `POST /mcp` call to the matching MCP tool using
//! Bearer authentication (see REQUIREMENTS.md §9.2 for the mapping table).

use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    domain::{state::GlobalState, tag::TagMeta, task::Task},
    error::{AppError, Result},
    store::Store,
};

/// [`Store`] backed by a hosted `next-mcp` server.
///
/// Construction never fails; errors are returned lazily when any method is
/// called, so the binary starts up cleanly even before the server is reachable.
pub struct RemoteStore {
    /// Base URL of the server, e.g. `https://tasks.example.com`.
    pub url: String,
    /// Optional bearer token for authentication.
    pub token: Option<String>,
}

impl RemoteStore {
    pub fn new(url: impl Into<String>, token: Option<impl Into<String>>) -> Self {
        Self {
            url: url.into(),
            token: token.map(Into::into),
        }
    }

    fn not_implemented(&self) -> AppError {
        AppError::Other(format!(
            "remote backend ({}) not yet implemented — \
             each method should call the matching MCP tool on the next-mcp server",
            self.url
        ))
    }
}

impl Store for RemoteStore {
    fn get_task(&self, _id: Uuid) -> Result<Task> {
        Err(self.not_implemented())
    }

    fn get_task_by_slug(&self, _slug: &str) -> Result<Option<Task>> {
        Err(self.not_implemented())
    }

    fn find_tasks_by_prefix(&self, _prefix: &str) -> Result<Vec<Task>> {
        Err(self.not_implemented())
    }

    fn list_tasks(&self) -> Result<Vec<Task>> {
        Err(self.not_implemented())
    }

    fn save_task(&mut self, _task: &Task) -> Result<()> {
        Err(self.not_implemented())
    }

    fn delete_task(&mut self, _id: Uuid) -> Result<()> {
        Err(self.not_implemented())
    }

    fn get_state(&self) -> Result<GlobalState> {
        Err(self.not_implemented())
    }

    fn save_state(&mut self, _state: &GlobalState) -> Result<()> {
        Err(self.not_implemented())
    }

    fn get_tag_meta(&self, _tag: &str) -> Result<Option<TagMeta>> {
        Err(self.not_implemented())
    }

    fn set_tag_meta(&mut self, _tag: &str, _meta: TagMeta) -> Result<()> {
        Err(self.not_implemented())
    }

    fn delete_tag_meta(&mut self, _tag: &str) -> Result<()> {
        Err(self.not_implemented())
    }

    fn list_tag_metas(&self) -> Result<HashMap<String, TagMeta>> {
        Err(self.not_implemented())
    }
}
