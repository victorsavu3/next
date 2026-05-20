use next::store::PullResult;

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
    if !args.push_only {
        match ctx.vcs.pull()? {
            PullResult::Clean => {
                ctx.log.info("sync", "pull: clean");
            }
            PullResult::Conflicts(paths) => {
                let names: Vec<_> = paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                eprintln!("Merge conflicts — resolve manually: {}", names.join(", "));
                ctx.log
                    .error("sync", &format!("pull conflicts: {}", names.join(", ")));
                return Ok(());
            }
        }
    }

    if !args.pull_only {
        ctx.vcs.push()?;
        ctx.log.info("sync", "push: ok");
    }

    Ok(())
}
