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

    /// Return only these fields, e.g. `--fields id,title,due`. Requires
    /// `--json`; the table prints a fixed set of columns. A field that was not
    /// asked for is absent from the object rather than null.
    #[arg(long, value_delimiter = ',')]
    pub fields: Vec<String>,

    /// Filter expression, e.g. `+@work -bug due<+7d` or a bare word to search.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tokens: Vec<String>,
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    run_with_writer(args, ctx, &mut std::io::stdout())
}

/// The body, with output injectable so a test can read the JSON rather than
/// only assert that the command did not error — `--fields` is a claim about
/// what is printed, and printing to stdout leaves nothing to assert.
pub fn run_with_writer(
    args: Args,
    ctx: &AppContext,
    out: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let count = args.count.unwrap_or(ctx.config.next_count);

    crate::cli::commands::reject_misplaced_flags::<Args>(&args.tokens, "next next")?;
    let mut filter_args = FilterArgs::parse(args.tokens)?;
    filter_args.future = args.future;
    filter_args.all = args.all;
    filter_args.all_users = args.all_users;
    filter_args.json =
        crate::cli::commands::OutputFormat::resolve(args.format, args.json).is_json();

    crate::cli::commands::reject_fields_without_json(&args.fields, filter_args.json)?;
    // Parsed up front so an unknown field name fails before any work.
    let projection = crate::core::projection::Projection::parse(&args.fields)?;

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
        let mut json = serde_json::to_value(&scored)?;
        if let Some(items) = json.as_array_mut() {
            for item in items {
                projection.apply_to_scored(item);
            }
        }
        writeln!(out, "{}", serde_json::to_string_pretty(&json)?)?;
    } else {
        render::render_task_list(&scored);
    }
    Ok(())
}
