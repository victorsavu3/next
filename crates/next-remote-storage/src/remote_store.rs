//! [`RemoteStore`] — stub implementation of [`Store`] backed by a remote server.
//!
//! Every method currently returns [`AppError::Other`] with a "not yet
//! implemented" message. When the server protocol is finalised the stubs will
//! be replaced with HTTP calls.

use uuid::Uuid;

use next::{
    domain::{state::GlobalState, task::Task},
    error::{AppError, Result},
    store::Store,
};

/// Stub [`Store`] that will talk to a hosted `next-server` over HTTP.
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
            "remote store ({}) not yet implemented",
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

    fn get_task_by_forgejo_issue(&self, _url: &str) -> Result<Option<Task>> {
        Err(self.not_implemented())
    }

    fn get_task_by_webcal_uid(&self, _uid: &str) -> Result<Option<Task>> {
        Err(self.not_implemented())
    }

    fn get_state(&self) -> Result<GlobalState> {
        Err(self.not_implemented())
    }

    fn save_state(&mut self, _state: &GlobalState) -> Result<()> {
        Err(self.not_implemented())
    }
}
