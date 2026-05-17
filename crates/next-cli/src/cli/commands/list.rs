use chrono::Local;
use next::domain::{filter, scoring};

use crate::cli::{filter::FilterArgs, render};
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Include tasks scheduled in the future.
    #[arg(long)]
    pub future: bool,

    /// Show all tasks regardless of status or stage.
    #[arg(long)]
    pub all: bool,

    /// Filter by stage (inbox, project, waiting, someday).
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
    let today = Local::now().date_naive();

    let mut filter_args = FilterArgs::parse(args.tokens);
    filter_args.future = args.future;
    filter_args.all = args.all;
    filter_args.stage = args.stage;
    filter_args.json = args.json;

    let filter_set = filter_args.to_filter_set()?;
    let state = ctx.store.get_state()?;
    let all_tasks = ctx.store.list_tasks()?;

    let filtered = filter::apply(all_tasks.clone(), &filter_set, &state, today);
    let scored = scoring::score_and_sort(filtered, &all_tasks, today, &ctx.config.scoring);

    if filter_args.json {
        println!("{}", serde_json::to_string_pretty(&scored)?);
    } else {
        render::render_task_list(&scored);
    }

    Ok(())
}
