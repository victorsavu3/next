use crate::core::{domain::filter, listing, scoring};
use chrono::Local;

use crate::AppContext;
use crate::{cli::render, core::FilterArgs};

#[derive(clap::Args, Debug)]
#[command(name = "next")]
pub struct Args {
    /// Number of tasks to show (default: config value, usually 10).
    pub count: Option<usize>,

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

    /// Output format.
    ///
    /// No `--count` here, unlike `list`: `next --count N` already means "show
    /// me N tasks", and a command whose whole purpose is the top of the list
    /// has nothing useful to say about how many matched in total.
    #[arg(long, value_enum, conflicts_with = "json")]
    pub format: Option<crate::cli::commands::OutputFormat>,

    /// Filter expression, e.g. `+@work -bug due<+7d` or a bare word to search.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tokens: Vec<String>,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let count = args.count.unwrap_or(ctx.config.next_count);

    crate::core::reject_flag_like_tokens(&args.tokens, "next next --help")?;
    let mut filter_args = FilterArgs::parse(args.tokens)?;
    filter_args.future = args.future;
    filter_args.all = args.all;
    filter_args.all_users = args.all_users;
    filter_args.json =
        crate::cli::commands::OutputFormat::resolve(args.format, args.json).is_json();

    let filter_set = filter_args.to_filter_set()?;
    let state = ctx.repo.store().get_state()?;
    let candidates = listing::load_candidates(ctx.repo.store(), &filter_set)?;
    let tag_metas = ctx.repo.store().list_tag_metas()?;

    // Dates before filtering: `created:` and `updated:` are query terms.
    let pool = listing::extend_with_parents(ctx.repo.store(), candidates.clone())?;
    let task_dates = ctx.repo.task_git_dates_for(&pool);

    let filtered = filter::apply(candidates, &filter_set, &state, today, &task_dates);
    let mut scored = scoring::score_and_sort(
        filtered,
        &pool,
        today,
        &ctx.repo.scoring,
        &tag_metas,
        &task_dates,
    );
    scored.truncate(count);

    if filter_args.json {
        println!("{}", serde_json::to_string_pretty(&scored)?);
    } else {
        render::render_task_list(&scored);
    }
    Ok(())
}
