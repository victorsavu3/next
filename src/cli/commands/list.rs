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

    /// Return only these fields, e.g. `--fields id,title,due`. Requires
    /// `--json`; the table prints a fixed set of columns. A field that was not
    /// asked for is absent from the object rather than null.
    #[arg(long, value_delimiter = ',')]
    pub fields: Vec<String>,

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

impl Args {
    /// How many tasks fit on one page: `--page-size`, then the `-n`/`--limit`
    /// spelling of the same thing, then `list_limit` from config, then the
    /// built-in default.
    ///
    /// One function because there is one answer. The archived path used to
    /// stop at `--limit` and fall through to the default, so the same config
    /// gave `next list` and `next list --archived` different page sizes.
    fn page_size(&self, list_limit: Option<usize>) -> u32 {
        self.page_size
            .or(self.limit.map(|n| n as u32))
            .or(list_limit.map(|n| n as u32))
            .unwrap_or(crate::core::store::DEFAULT_PAGE_SIZE)
    }
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    run_with_writer(args, ctx, &mut std::io::stdout())
}

/// What to say when the status gate was always going to swallow the query.
///
/// It names the flag that lifts the gate rather than lifting it: which tasks
/// `next list` returns is not something a query term gets to change.
fn gate_hint(gate: listing::ContradictedGate) -> &'static str {
    match gate {
        listing::ContradictedGate::Active => {
            "No matches: this listing shows only open tasks, and the query asks \
             for a status it excludes. Repeat it with `--all` to search every \
             status."
        }
        listing::ContradictedGate::Closed => {
            "No matches: `--closed` shows only closed tasks, and the query asks \
             for a status it excludes. Drop `--closed`, or use `--all`, to \
             search every status."
        }
    }
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

    crate::cli::commands::reject_misplaced_flags::<Args>(&args.tokens, "next list")?;
    let format = crate::cli::commands::OutputFormat::resolve(args.format, args.json);
    crate::cli::commands::reject_fields_without_json(&args.fields, format.is_json())?;
    // Resolved before the tokens are moved out of `args`, and once for both
    // the archived and the active path.
    let page_size = args.page_size(ctx.config.list_limit);
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
    // Parsed up front so an unknown field name fails before any work, rather
    // than after a full scan.
    let projection = crate::core::projection::Projection::parse(&args.fields)?;

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
            page_size,
            ..Default::default()
        })?;
        if args.count {
            // The total across every page, which is what someone asking "how
            // many?" means — not how many happened to fit on this one.
            writeln!(out, "{}", page.total)?;
            return Ok(());
        }
        if filter_args.json {
            let mut json = serde_json::to_value(&page)?;
            projection.apply_to_page(&mut json);
            writeln!(out, "{}", serde_json::to_string_pretty(&json)?)?;
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

    // Read before `scored` is consumed by pagination; the hint below is about
    // the whole result, not about this page being past the end.
    let nothing_matched = scored.is_empty();
    let page = crate::core::store::paginate(scored, args.page, page_size);

    if filter_args.json {
        let mut json = serde_json::to_value(&page)?;
        projection.apply_to_page(&mut json);
        writeln!(out, "{}", serde_json::to_string_pretty(&json)?)?;
    } else {
        // `--fields` cannot reach here: it is refused without `--json` above,
        // rather than parsed and quietly ignored.
        render::render_task_list(&page.items);
        render::render_page_footer(&page);
        // An empty result the implicit gate was always going to produce says
        // "no such tasks" when it means "not in this view". Naming the flag
        // that widens it is the whole fix — what matched does not change.
        if nothing_matched {
            if let Some(gate) = listing::contradicted_status_gate(&filter_set) {
                writeln!(out, "{}", gate_hint(gate))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults clap would produce, with only the paging fields set.
    fn paging(page_size: Option<u32>, limit: Option<usize>) -> Args {
        Args {
            future: false,
            all: false,
            closed: false,
            archived: false,
            all_users: false,
            json: false,
            format: None,
            count: false,
            explain: false,
            fields: vec![],
            limit,
            page_size,
            page: 1,
            tokens: vec![],
        }
    }

    #[test]
    fn the_page_size_precedence_is_one_ladder() {
        let default = crate::core::store::DEFAULT_PAGE_SIZE;
        assert_eq!(
            paging(Some(7), Some(5)).page_size(Some(3)),
            7,
            "--page-size"
        );
        assert_eq!(paging(None, Some(5)).page_size(Some(3)), 5, "then --limit");
        assert_eq!(paging(None, None).page_size(Some(3)), 3, "then list_limit");
        assert_eq!(
            paging(None, None).page_size(None),
            default,
            "then the default"
        );
    }
}
