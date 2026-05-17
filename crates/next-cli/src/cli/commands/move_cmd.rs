use crate::AppContext;

#[derive(clap::Args, Debug)]
#[command(name = "move")]
pub struct Args {
    /// Task to move: UUID, UUID prefix, or slug.
    pub id: String,

    /// New parent task: UUID, UUID prefix, or slug. Pass "none" to remove.
    #[arg(long)]
    pub parent: Option<String>,

    /// New GTD stage (inbox, project, waiting, someday).
    #[arg(long)]
    pub stage: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    println!("not yet implemented: move (id={})", args.id);
    Ok(())
}
