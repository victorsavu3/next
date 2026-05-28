/// The full tutorial text, embedded from `TUTORIAL.md` at compile time.
static TUTORIAL_TEXT: &str = include_str!("../../../TUTORIAL.md");

#[derive(clap::Args, Debug)]
pub struct Args;

pub fn run(_args: Args) -> anyhow::Result<()> {
    print!("{TUTORIAL_TEXT}");
    Ok(())
}
