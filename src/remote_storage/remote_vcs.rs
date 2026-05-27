//! [`RemoteVcs`] — no-op [`VcsBackend`] for the remote storage backend.
//!
//! When tasks are stored on a server, the server owns persistence. There is no
//! local git repository to commit to, and push/pull are handled by the server
//! itself. All VCS operations therefore succeed silently.

use std::path::PathBuf;

use crate::{
    error::Result,
    store::{PullResult, VcsBackend},
};

/// No-op VCS backend used with [`super::RemoteStore`].
///
/// `commit` and `push` are silent no-ops. `pull` always returns
/// [`PullResult::Clean`]. `head_hash` returns `"remote"` as a stable sentinel.
pub struct RemoteVcs {
    /// Base URL of the server (stored for future use, e.g. server-side history).
    pub url: String,
    /// Optional bearer token.
    pub token: Option<String>,
}

impl RemoteVcs {
    pub fn new(url: impl Into<String>, token: Option<impl Into<String>>) -> Self {
        Self {
            url: url.into(),
            token: token.map(Into::into),
        }
    }
}

impl VcsBackend for RemoteVcs {
    fn commit(&self, _paths: &[PathBuf], _message: &str) -> Result<()> {
        Ok(())
    }

    fn pull(&self) -> Result<PullResult> {
        Ok(PullResult::Clean)
    }

    fn push(&self) -> Result<()> {
        Ok(())
    }

    fn head_hash(&self) -> Result<String> {
        Ok("remote".to_owned())
    }
}
