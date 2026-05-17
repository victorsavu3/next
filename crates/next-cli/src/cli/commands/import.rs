use crate::AppContext;

/// Top-level `next import` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: ImportSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum ImportSubcommand {
    /// Import issues from a Forgejo repository.
    Forgejo(ForgejoArgs),
    /// Import tasks from an iCal file or URL.
    Ical(IcalArgs),
}

#[derive(clap::Args, Debug)]
pub struct ForgejoArgs {
    /// Repository in "owner/repo" format.
    pub repo: String,

    /// Parent task for imported tasks: UUID, UUID prefix, or slug.
    #[arg(long)]
    pub parent: Option<String>,

    /// Extra tags to attach to all imported tasks (repeatable).
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct IcalArgs {
    /// Path to a local iCal file or an http(s)://  / webcal:// URL.
    pub source: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        ImportSubcommand::Forgejo(a) => {
            println!("not yet implemented: import forgejo (repo={})", a.repo)
        }
        ImportSubcommand::Ical(a) => {
            println!("not yet implemented: import ical (source={})", a.source)
        }
    }
    Ok(())
}
