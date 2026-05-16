use crate::AppContext;
use crate::cli::filter::FilterArgs;

#[derive(clap::Args, Debug)]
#[command(name = "next")]
pub struct Args {
    /// Number of tasks to show (default: config value, usually 10).
    pub count: Option<usize>,

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

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let mut filter = FilterArgs::parse(args.tokens);
    filter.future = args.future;
    filter.all = args.all;
    filter.stage = args.stage;
    filter.json = args.json;
    let count = args.count.unwrap_or(ctx.config.next_count);
    println!("not yet implemented: next (count={count}, filter={filter:?})");
    Ok(())
}
