use crate::AppContext;
use crate::cli::filter::FilterArgs;

/// Top-level `next export` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: ExportSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum ExportSubcommand {
    /// Export tasks as an iCal file.
    Ical(IcalArgs),
}

#[derive(clap::Args, Debug)]
pub struct IcalArgs {
    /// Output file path (stdout if omitted).
    #[arg(long, short)]
    pub output: Option<String>,

    /// Include tasks scheduled in the future.
    #[arg(long)]
    pub future: bool,

    /// Show all tasks regardless of status or stage.
    #[arg(long)]
    pub all: bool,

    /// Filter by stage.
    #[arg(long)]
    pub stage: Option<String>,

    /// Filter tokens: +tag, -tag, project:path, context:@name.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tokens: Vec<String>,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        ExportSubcommand::Ical(a) => {
            let mut filter = FilterArgs::parse(a.tokens);
            filter.future = a.future;
            filter.all = a.all;
            filter.stage = a.stage;
            println!(
                "not yet implemented: export ical (output={:?}, filter={filter:?})",
                a.output
            );
        }
    }
    Ok(())
}
