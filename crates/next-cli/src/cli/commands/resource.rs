use crate::AppContext;

/// Availability toggle for `next resource set`.
#[derive(clap::ValueEnum, Clone, Debug)]
pub enum Availability {
    On,
    Off,
}

/// Top-level `next resource` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    #[command(subcommand)]
    pub subcommand: Option<ResourceSubcommand>,
}

#[derive(clap::Subcommand, Debug)]
pub enum ResourceSubcommand {
    /// Set availability of a $-prefixed resource tag.
    Set(SetArgs),
}

#[derive(clap::Args, Debug)]
pub struct SetArgs {
    /// Resource tag (e.g. "$computer").
    pub resource: String,

    /// Whether the resource is available.
    pub availability: Availability,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => println!("not yet implemented: resource (list resources)"),
        Some(ResourceSubcommand::Set(a)) => println!(
            "not yet implemented: resource set (resource={}, availability={:?})",
            a.resource, a.availability
        ),
    }
    Ok(())
}
