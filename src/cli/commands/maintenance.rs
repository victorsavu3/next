use crate::cli::commands::archive;
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Drop and rebuild the local SQLite read cache (`.next.db`) from the
    /// committed TOML files, git history, and archive segments. Use this to
    /// recover from a corrupt or stale cache; it changes no committed data.
    RebuildCache(RebuildCacheArgs),

    /// Run the archive pass now: move old closed tasks into archive segments
    /// and prune sealed segments to the cold tier. Normally this runs
    /// automatically (at most once a day) during `next sync`.
    Archive(archive::Args),
}

#[derive(clap::Args, Debug)]
pub struct RebuildCacheArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.command {
        Command::RebuildCache(a) => rebuild_cache(a, ctx),
        Command::Archive(a) => archive::run(a, ctx),
    }
}

fn rebuild_cache(args: RebuildCacheArgs, ctx: &mut AppContext) -> anyhow::Result<()> {
    let head = ctx.repo.vcs.head_hash()?;
    ctx.repo.store.rebuild_cache(&head)?;
    let count = ctx.repo.store.list_tasks()?.len();

    if args.json {
        println!(
            "{}",
            serde_json::json!({ "rebuilt": true, "active_tasks": count })
        );
    } else {
        println!("Rebuilt the local cache ({count} active task(s)).");
    }
    Ok(())
}
