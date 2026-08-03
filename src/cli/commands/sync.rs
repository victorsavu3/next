use std::path::PathBuf;

use crate::core::{sync, SyncOutcome};
use crate::AppContext;

/// Error returned by [`run`] when the pull left merge conflicts.
///
/// REQUIREMENTS.md §2.2: an explicit `next sync` MUST exit with code 2 on
/// merge conflicts so scripts can detect them. `main` downcasts to this type
/// to map it to `std::process::exit(2)`; every other error keeps exit code 1.
#[derive(Debug)]
pub struct ConflictsError(pub Vec<PathBuf>);

impl std::fmt::Display for ConflictsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<_> = self.0.iter().map(|p| p.display().to_string()).collect();
        write!(
            f,
            "Merge conflicts — resolve manually: {}",
            names.join(", ")
        )
    }
}

impl std::error::Error for ConflictsError {}

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Only push; skip pulling from remote.
    #[arg(long)]
    pub push_only: bool,

    /// Only pull; skip pushing to remote.
    #[arg(long)]
    pub pull_only: bool,

    /// Suppress the success confirmation (set by autosync, not a CLI flag).
    #[arg(skip)]
    pub quiet: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match sync(&mut ctx.repo, args.push_only, args.pull_only)? {
        SyncOutcome::Clean => {
            if !args.quiet {
                println!("Synced with remote.");
            }
            tracing::info!(cmd = "sync", "ok");
            // Trigger any registered plugin's periodic sync that is now due.
            // Best-effort; runs after the sync (no repo lock held).
            crate::core::plugin::run_due_syncs(
                &ctx.repo.repo_root,
                ctx.config.sync.plugin_sync_default_secs,
                chrono::Local::now().to_utc(),
            );
        }
        SyncOutcome::Conflicts(paths) => {
            let err = ConflictsError(paths);
            eprintln!("{err}");
            tracing::error!(cmd = "sync", "pull conflicts: {err}");
            return Err(err.into());
        }
    }
    Ok(())
}
