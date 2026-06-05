use crate::core::{sync, SyncOutcome};
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Only push; skip pulling from remote.
    #[arg(long)]
    pub push_only: bool,

    /// Only pull; skip pushing to remote.
    #[arg(long)]
    pub pull_only: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match sync(&mut ctx.repo, args.push_only, args.pull_only)? {
        SyncOutcome::Clean => {
            tracing::info!(cmd = "sync", "ok");
        }
        SyncOutcome::Conflicts(paths) => {
            let names: Vec<_> = paths.iter().map(|p| p.display().to_string()).collect();
            eprintln!("Merge conflicts — resolve manually: {}", names.join(", "));
            tracing::error!(cmd = "sync", "pull conflicts: {}", names.join(", "));
        }
    }
    Ok(())
}
