use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task ID to cancel.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    println!("not yet implemented: cancel (id={})", args.id);
    Ok(())
}
