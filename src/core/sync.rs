//! Repository sync (git pull/push) shared by the `sync` CLI command and the
//! Forgejo plugin.
//!
//! This is the core pull → cache-reconcile → push sequence with no I/O of its
//! own (no printing); callers decide how to report the outcome.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::core::{store::PullResult, sync_state, TaskRepository};

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
pub fn sync(ctx: &mut TaskRepository, push_only: bool, pull_only: bool) -> anyhow::Result<SyncOutcome> {
    if !push_only {
        match ctx.vcs.pull()? {
            PullResult::Clean => {
                let head = ctx.vcs.head_hash()?;
                ctx.store.after_pull(&head)?;
                // A clean pull means the local copy is fresh — reset the
                // staleness clock so pull-before-query (Req A) won't re-pull.
                // Best-effort: a metadata write failure must not fail the sync.
                if let Err(e) =
                    crate::core::sync_state::record_pull(&ctx.repo_root, chrono::Utc::now())
                {
                    tracing::warn!("failed to record last_pull: {e}");
                }
            }
            PullResult::Conflicts(paths) => return Ok(SyncOutcome::Conflicts(paths)),
        }
    }
    if !pull_only {
        ctx.vcs.push()?;
    }
    Ok(SyncOutcome::Clean)
}

// ── Pull-before-query staleness (Req A) ────────────────────────────────────────

/// Outcome of a [`pull_if_stale`] attempt.  Never an error — the caller proceeds
/// with the command regardless of which arm is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullStatus {
    /// Pull-before-query is disabled (config off or `--offline`); no I/O done.
    Disabled,
    /// The local copy is within the staleness window; no pull was attempted.
    Fresh,
    /// A pull ran cleanly and the cache + `last_pull` were updated.
    Pulled,
    /// The local copy was stale but the pull could not be completed.  The reason
    /// is human-readable; the command still proceeds (on possibly stale data).
    Failed(String),
}

/// Options controlling [`pull_if_stale`].
#[derive(Debug, Clone)]
pub struct StaleOpts {
    /// Whether pull-before-query is active for this invocation.
    pub enabled: bool,
    /// How long a local copy stays "fresh" after a pull.
    pub staleness: Duration,
    /// The current instant (injected for testability).
    pub now: DateTime<Utc>,
}

/// Pulls the latest changes before a read/write *if* the local copy is stale.
///
/// This is the "pull-before-query" entry point shared by every front-end (CLI
/// now; MCP/TUI later).  It is deliberately infallible — it returns a
/// [`PullStatus`] so the caller always proceeds with the command, even when the
/// pull fails (e.g. the network is down).  Callers should surface
/// [`PullStatus::Pulled`] / [`PullStatus::Failed`] as a stderr note/warning.
///
/// Note: `pull_timeout_secs` is *not* enforced here yet (deferred to a later
/// task); the pull is invoked directly.
pub fn pull_if_stale(ctx: &mut TaskRepository, opts: &StaleOpts) -> PullStatus {
    if !opts.enabled {
        return PullStatus::Disabled;
    }

    let root = ctx.repo_root.clone();
    if let Ok(state) = sync_state::load(&root) {
        if let Some(last) = state.last_pull {
            if opts.now.signed_duration_since(last).to_std().is_ok_and(|elapsed| elapsed < opts.staleness) {
                return PullStatus::Fresh;
            }
        }
    }

    match ctx.vcs.pull() {
        Ok(PullResult::Clean) => {
            // Reconcile the cache with the new HEAD, then record the pull.
            // Any failure here downgrades to Failed without propagating.
            let reconcile = ctx
                .vcs
                .head_hash()
                .and_then(|head| ctx.store.after_pull(&head));
            if let Err(e) = reconcile {
                return PullStatus::Failed(e.to_string());
            }
            if let Err(e) = sync_state::record_pull(&root, opts.now) {
                return PullStatus::Failed(e.to_string());
            }
            PullStatus::Pulled
        }
        Ok(PullResult::Conflicts(_)) => {
            PullStatus::Failed("merge conflicts; resolve manually".to_owned())
        }
        Err(e) => PullStatus::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod stale_tests {
    use super::*;
    use crate::core::TaskRepository;

    /// Builds a TaskRepository over a fresh tempdir git repo (no remote).
    fn temp_repo() -> (tempfile::TempDir, TaskRepository) {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Test").unwrap();
            cfg.set_str("user.email", "test@test.com").unwrap();
        }
        let ctx = TaskRepository::open(dir.path().to_path_buf()).unwrap();
        (dir, ctx)
    }

    #[test]
    fn disabled_does_no_io() {
        let (_dir, mut ctx) = temp_repo();
        let opts = StaleOpts {
            enabled: false,
            staleness: Duration::from_secs(3600),
            now: Utc::now(),
        };
        assert_eq!(pull_if_stale(&mut ctx, &opts), PullStatus::Disabled);
        // No sync_state file should have been written.
        let path = crate::core::storage::sync_state_path_for_repo(&ctx.repo_root);
        assert!(!path.exists(), "disabled must not touch sync_state");
    }

    #[test]
    fn fresh_when_recently_pulled() {
        let (_dir, mut ctx) = temp_repo();
        let now = Utc::now();
        // Record a pull "now", then query with a large staleness window.
        sync_state::record_pull(&ctx.repo_root, now).unwrap();
        let opts = StaleOpts {
            enabled: true,
            staleness: Duration::from_secs(3600),
            now: now + chrono::Duration::seconds(10),
        };
        assert_eq!(pull_if_stale(&mut ctx, &opts), PullStatus::Fresh);
    }

    #[test]
    fn stale_without_remote_fails_without_recording() {
        let (_dir, mut ctx) = temp_repo();
        // No last_pull recorded → stale. No remote configured → pull fails.
        let opts = StaleOpts {
            enabled: true,
            staleness: Duration::from_secs(3600),
            now: Utc::now(),
        };
        match pull_if_stale(&mut ctx, &opts) {
            PullStatus::Failed(_) => {}
            other => panic!("expected Failed for no-remote stale repo, got {other:?}"),
        }
        // Must NOT have recorded a pull on failure.
        let state = sync_state::load(&ctx.repo_root).unwrap();
        assert!(state.last_pull.is_none(), "failed pull must not record last_pull");
    }
}
