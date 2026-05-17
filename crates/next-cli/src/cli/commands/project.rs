use crate::AppContext;

/// Top-level `next project` subcommand.
/// Projects are plain tasks with stage=project; these commands are
/// convenience wrappers over the general task commands.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: ProjectSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum ProjectSubcommand {
    /// List tasks with stage=project in a tree view.
    List(ListArgs),
    /// Create a new project task (shorthand for `next add --stage project`).
    Add(AddArgs),
    /// Show a task and all its descendants (any stage).
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
    /// Project title.
    pub title: String,

    /// User-provided slug for stable referencing.
    #[arg(long)]
    pub slug: Option<String>,

    /// Project priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// Free-text notes (serves as project description).
    #[arg(long)]
    pub notes: Option<String>,

    /// Parent task: UUID, UUID prefix, or slug.
    #[arg(long)]
    pub parent: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct ShowArgs {
    /// Project task: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        ProjectSubcommand::List(_) => println!("not yet implemented: project list"),
        ProjectSubcommand::Add(a) => {
            println!("not yet implemented: project add (title={})", a.title)
        }
        ProjectSubcommand::Show(a) => {
            println!("not yet implemented: project show (id={})", a.id)
        }
    }
    Ok(())
}
