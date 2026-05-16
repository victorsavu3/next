use crate::AppContext;

/// Top-level `next project` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: ProjectSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum ProjectSubcommand {
    /// List all projects.
    List(ListArgs),
    /// Add a new project.
    Add(AddArgs),
    /// Show details for a project.
    Show(ShowArgs),
}

#[derive(clap::Args, Debug)]
pub struct ListArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct AddArgs {
    /// Project path (e.g. "work/backend").
    pub path: String,

    /// Project priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// Short description.
    #[arg(long)]
    pub description: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct ShowArgs {
    /// Project path.
    pub path: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        ProjectSubcommand::List(_) => println!("not yet implemented: project list"),
        ProjectSubcommand::Add(a) => {
            println!("not yet implemented: project add (path={})", a.path)
        }
        ProjectSubcommand::Show(a) => {
            println!("not yet implemented: project show (path={})", a.path)
        }
    }
    Ok(())
}
