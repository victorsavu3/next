use chrono::Local;
use crate::core::{domain::filter, scoring};

use crate::{cli::render, core::FilterArgs};
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Include tasks scheduled in the future.
    #[arg(long)]
    pub future: bool,

    /// Show all tasks regardless of implicit filtering.
    #[arg(long)]
    pub all: bool,

    /// Show tasks for all users, ignoring the active user filter.
    #[arg(long)]
    pub all_users: bool,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Maximum number of tasks to show. Overrides `list_limit` in config.
    /// Without this flag (and with no config default), all matching tasks are shown.
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,

    /// Filter tokens: +tag, -tag, parent:slug (project scope), context:@name, user:name.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tokens: Vec<String>,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();

    let mut filter_args = FilterArgs::parse(args.tokens);
    filter_args.future = args.future;
    filter_args.all = args.all;
    filter_args.all_users = args.all_users;
    filter_args.json = args.json;

    let filter_set = filter_args.to_filter_set()?;
    let state = ctx.store().get_state()?;
    let all_tasks = ctx.store().list_tasks()?;
    let tag_metas = ctx.store().list_tag_metas()?;

    let filtered = filter::apply(all_tasks.clone(), &filter_set, &state, today);
    let mut scored = scoring::score_and_sort(filtered, &all_tasks, today, &ctx.config.scoring, &tag_metas);

    let limit = args.limit.or(ctx.config.list_limit);
    if let Some(n) = limit {
        scored.truncate(n);
    }

    if filter_args.json {
        println!("{}", serde_json::to_string_pretty(&scored)?);
    } else {
        render::render_task_list(&scored);
    }

    Ok(())
}
