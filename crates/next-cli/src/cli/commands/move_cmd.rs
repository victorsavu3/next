use crate::AppContext;

#[derive(clap::Args, Debug)]
#[command(name = "move")]
pub struct Args {
    /// Task ID to move.
    pub id: String,

    /// Target project path.
    #[arg(long)]
    pub project: Option<String>,

    /// Target stage (inbox, next, waiting, someday).
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
