//! Repository sync (git pull/push) shared by the `sync` CLI command and the
//! Forgejo plugin.
//!
//! This is the core pull → cache-reconcile → push sequence with no I/O of its
//! own (no printing); callers decide how to report the outcome.

use std::path::PathBuf;

use crate::{store::PullResult, AppContext};

/// Result of a [`sync`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Pull (if any) was clean and push (if any) succeeded.
    Clean,
    /// Pull produced merge conflicts in these files; nothing was pushed.
    Conflicts(Vec<PathBuf>),
}

/// Pulls (unless `push_only`), reconciles the cache with the new HEAD, then
/// pushes (unless `pull_only`). On pull conflicts, returns early without
/// pushing and without rebuilding the cache (the working tree holds the
/// conflict markers for the user to resolve).
pub fn sync(ctx: &mut AppContext, push_only: bool, pull_only: bool) -> anyhow::Result<SyncOutcome> {
    if !push_only {
        match ctx.vcs.pull()? {
            PullResult::Clean => {
                let head = ctx.vcs.head_hash()?;
                ctx.store.after_pull(&head)?;
            }
            PullResult::Conflicts(paths) => return Ok(SyncOutcome::Conflicts(paths)),
        }
    }
    if !pull_only {
        ctx.vcs.push()?;
    }
    Ok(SyncOutcome::Clean)
}
