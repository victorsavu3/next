use crate::core::{domain::filter, listing, scoring};
use chrono::Local;

use crate::AppContext;
use crate::{cli::render, core::FilterArgs};

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

    /// Show archived tasks (most recently completed first). The same filter
    /// grammar and pagination apply; scoring and the implicit gate do not.
    #[arg(long, conflicts_with_all = ["all", "closed", "future"])]
    pub archived: bool,

    /// Show tasks for all users, ignoring the active user filter.
    #[arg(long)]
    pub all_users: bool,

    /// Output as JSON. Alias for `--format json`.
    #[arg(long, conflicts_with = "format")]
    pub json: bool,

    /// Output format.
    #[arg(long, value_enum)]
    pub format: Option<crate::cli::commands::OutputFormat>,

    /// Print only how many tasks match. Under pagination this is the total
    /// across all pages, not the size of one page.
    #[arg(long)]
    pub count: bool,

    /// Show what the filter parsed to and how it will be run, instead of
    /// listing tasks. Use it when a query returns something unexpected.
    #[arg(long)]
    pub explain: bool,

    /// Maximum number of tasks to show. Overrides `list_limit` in config.
    /// Shorthand for `--page-size` (both cap the window on the result).
    #[arg(short = 'n', long)]
    pub limit: Option<usize>,

    /// Tasks per page (default 50, or `list_limit` from config).
    #[arg(long)]
    pub page_size: Option<u32>,

    /// 1-indexed page of results to show.
    #[arg(long, default_value_t = 1)]
    pub page: u32,

    /// Filter expression, e.g. `+@work -bug due<+7d` or a bare word to search.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tokens: Vec<String>,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    run_with_writer(args, ctx, &mut std::io::stdout())
}

/// The body, with output injectable so a test can read what was printed —
/// `--explain` in particular exists to be read, so its text is worth
/// asserting rather than eyeballing.
pub fn run_with_writer(
    args: Args,
    ctx: &AppContext,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let today = Local::now().date_naive();

    crate::core::reject_flag_like_tokens(&args.tokens, "next list --help")?;
    let format = crate::cli::commands::OutputFormat::resolve(args.format, args.json);
    // Kept for `--explain`, which shows the query as the user typed it —
    // after argv joining, which is where a shell-eaten filter goes missing.
    let raw_query = args.tokens.join(" ");
    let mut filter_args = FilterArgs::parse(args.tokens)?;
    filter_args.future = args.future;
    filter_args.all = args.all;
    filter_args.closed = args.closed;
    filter_args.all_users = args.all_users;
    filter_args.json = format.is_json();

    let filter_set = filter_args.to_filter_set()?;

    if args.archived {
        if args.explain {
            let store_filter = filter_set.to_store_filter(today)?;
            let (sql, exact) = crate::core::storage::explain_filter_pushdown(
                &store_filter.expr,
                store_filter.today,
            );
            let page = ctx.repo.store().query_tasks(&crate::core::TaskQuery {
                archived: true,
                filter: Some(store_filter),
                ..crate::core::TaskQuery::unpaginated()
            })?;
            write!(
                out,
                "{}",
                crate::cli::explain::render(
                    &raw_query,
                    &filter_set,
                    page.total as usize,
                    page.total as usize,
                    &crate::cli::explain::Execution::Sql { sql, exact },
                )
            )?;
            return Ok(());
        }
        let page = ctx.repo.store().query_tasks(&crate::core::TaskQuery {
            archived: true,
            filter: Some(filter_set.to_store_filter(today)?),
            page: args.page,
            page_size: args
                .page_size
                .or(args.limit.map(|n| n as u32))
                .unwrap_or(crate::core::store::DEFAULT_PAGE_SIZE),
            ..Default::default()
        })?;
        if args.count {
            // The total across every page, which is what someone asking "how
            // many?" means — not how many happened to fit on this one.
            writeln!(out, "{}", page.total)?;
            return Ok(());
        }
        if filter_args.json {
            writeln!(out, "{}", serde_json::to_string_pretty(&page)?)?;
        } else {
            if page.items.is_empty() {
                writeln!(out, "No archived tasks.")?;
            }
            for task in &page.items {
                let short = &task.id.to_string().replace('-', "")[..8];
                let when = task
                    .completed_at
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "-".into());
                writeln!(out, "[{short}] {when}  {}", task.title)?;
            }
            render::render_page_footer(&page);
        }
        return Ok(());
    }

    let state = ctx.repo.store().get_state()?;
    let candidates = listing::load_candidates(ctx.repo.store(), &filter_set)?;
    let tag_metas = ctx.repo.store().list_tag_metas()?;

    // Dates before filtering, not after: `created:` and `updated:` are query
    // terms now, and the pool they are read from is the same one scoring uses.
    let pool = listing::extend_with_parents(ctx.repo.store(), candidates.clone())?;
    let task_dates = ctx.repo.task_git_dates_for(&pool);
    let candidate_count = candidates.len();

    let filtered = filter::apply(candidates, &filter_set, &state, today, &task_dates);
    let scored = scoring::score_and_sort(
        filtered,
        &pool,
        today,
        &ctx.repo.scoring,
        &tag_metas,
        &task_dates,
    );

    if args.explain {
        // Reports the counts from the run just performed rather than
        // recomputing them — an explanation that could disagree with the
        // pipeline would be worse than none.
        write!(
            out,
            "{}",
            crate::cli::explain::render(
                &raw_query,
                &filter_set,
                candidate_count,
                scored.len(),
                // The active list pushes only the status gate; the expression
                // itself is evaluated in memory by `filter::apply`.
                &crate::cli::explain::Execution::InMemory,
            )
        )?;
        return Ok(());
    }

    if args.count {
        // Counted before pagination, so the answer does not depend on the
        // page size the caller happened to pass.
        writeln!(out, "{}", scored.len())?;
        return Ok(());
    }

    // Precedence: --page-size, then the legacy --limit / list_limit caps.
    let page_size = args
        .page_size
        .or(args.limit.map(|n| n as u32))
        .or(ctx.config.list_limit.map(|n| n as u32))
        .unwrap_or(crate::core::store::DEFAULT_PAGE_SIZE);
    let page = crate::core::store::paginate(scored, args.page, page_size);

    if filter_args.json {
        writeln!(out, "{}", serde_json::to_string_pretty(&page)?)?;
    } else {
        render::render_task_list(&page.items);
        render::render_page_footer(&page);
    }

    Ok(())
}
