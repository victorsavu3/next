use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {}

pub fn run(_args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    println!("not yet implemented: sync");
    Ok(())
}
