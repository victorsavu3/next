use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task ID to delete.
    pub id: String,

    /// Skip confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    println!("not yet implemented: delete (id={})", args.id);
    Ok(())
}
