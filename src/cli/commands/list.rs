use chrono::Local;
use crate::core::{domain::filter, listing, scoring};

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

    /// Show only closed tasks (done or cancelled). Can be combined with --all.
    #[arg(long)]
    pub closed: bool,

    /// Show tasks for all users, ignoring the active user filter.
    #[arg(long)]
    pub all_users: bool,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Maximum number of tasks to show. Overrides `list_limit` in config.
    /// Shorthand for `--page-size` (both cap the window on the result).
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,

    /// Tasks per page (default 1000, or `list_limit` from config).
    #[arg(long)]
    pub page_size: Option<u32>,

    /// 1-indexed page of results to show.
    #[arg(long, default_value_t = 1)]
    pub page: u32,

    /// Filter tokens: +tag, -tag, parent:slug (project scope), context:@name, user:name.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tokens: Vec<String>,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();

    let mut filter_args = FilterArgs::parse(args.tokens);
    filter_args.future = args.future;
    filter_args.all = args.all;
    filter_args.closed = args.closed;
    filter_args.all_users = args.all_users;
    filter_args.json = args.json;

    let filter_set = filter_args.to_filter_set()?;
    let state = ctx.repo.store().get_state()?;
    let candidates = listing::load_candidates(ctx.repo.store(), &filter_set)?;
    let tag_metas = ctx.repo.store().list_tag_metas()?;

    let filtered = filter::apply(candidates.clone(), &filter_set, &state, today);
    let pool = listing::extend_with_parents(ctx.repo.store(), candidates)?;
    let task_dates = ctx.repo.task_git_dates_for(&pool);
    let scored = scoring::score_and_sort(filtered, &pool, today, &ctx.repo.scoring, &tag_metas, &task_dates);

    // Precedence: --page-size, then the legacy --limit / list_limit caps.
    let page_size = args
        .page_size
        .or(args.limit.map(|n| n as u32))
        .or(ctx.config.list_limit.map(|n| n as u32))
        .unwrap_or(crate::core::store::DEFAULT_PAGE_SIZE);
    let page = crate::core::store::paginate(scored, args.page, page_size);

    if filter_args.json {
        println!("{}", serde_json::to_string_pretty(&page)?);
    } else {
        render::render_task_list(&page.items);
        render::render_page_footer(&page);
    }

    Ok(())
}
