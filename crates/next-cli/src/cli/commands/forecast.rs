use crate::AppContext;
use crate::cli::filter::FilterArgs;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Number of days to forecast (default from config, usually 90).
    #[arg(long, default_value_t = 90)]
    pub days: u32,

    /// Include tasks scheduled in the future.
    #[arg(long)]
    pub future: bool,

    /// Show all tasks regardless of status or stage.
    #[arg(long)]
    pub all: bool,

    /// Filter by stage.
    #[arg(long)]
    pub stage: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Filter tokens: +tag, -tag, project:path, context:@name.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tokens: Vec<String>,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    let mut filter = FilterArgs::parse(args.tokens);
    filter.future = args.future;
    filter.all = args.all;
    filter.stage = args.stage;
    filter.json = args.json;
    println!(
        "not yet implemented: forecast (days={}, filter={filter:?})",
        args.days
    );
    Ok(())
}
