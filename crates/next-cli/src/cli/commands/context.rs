use crate::AppContext;

/// Top-level `next context` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: Option<ContextSubcommand>,
}

#[derive(clap::Subcommand, Debug)]
pub enum ContextSubcommand {
    /// Set active context tags.
    Set(SetArgs),
    /// Clear all active context tags.
    Clear,
}

#[derive(clap::Args, Debug)]
pub struct SetArgs {
    /// One or more @-prefixed context tags to activate.
    #[arg(num_args(1..))]
    pub tags: Vec<String>,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => println!("not yet implemented: context (show active contexts)"),
        Some(ContextSubcommand::Set(a)) => {
            println!("not yet implemented: context set (tags={:?})", a.tags)
        }
        Some(ContextSubcommand::Clear) => println!("not yet implemented: context clear"),
    }
    Ok(())
}
