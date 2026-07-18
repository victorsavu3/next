use chrono::{Local, NaiveDate};

use crate::core::forecast::{self, ForecastEntry};
use crate::core::FilterArgs;
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Number of days to forecast (default from config, usually 90).
    #[arg(long)]
    pub days: Option<u32>,

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

    /// Filter tokens: +tag, -tag, parent:slug, context:@name, user:name.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub tokens: Vec<String>,
}

/// Build the chronologically-ordered forecast entries for the given horizon by
/// delegating to [`crate::core::forecast::build_entries`]. The horizon (in days)
/// is returned alongside so callers can render the section headings.
pub fn build_entries(args: &Args, ctx: &AppContext) -> anyhow::Result<(Vec<ForecastEntry>, u32)> {
    let today = Local::now().date_naive();
    let horizon = args.days.unwrap_or(ctx.config.forecast_horizon_days);

    crate::core::reject_flag_like_tokens(&args.tokens, "next forecast --help")?;
    let mut filter_args = FilterArgs::parse(args.tokens.clone());
    filter_args.future = true; // forecast always shows future-start tasks
    filter_args.all = args.all;
    filter_args.all_users = args.all_users;
    filter_args.json = args.json;

    let filter_set = filter_args.to_filter_set()?;
    let state = ctx.repo.store().get_state()?;
    let candidates = crate::core::listing::load_candidates(ctx.repo.store(), &filter_set)?;
    let pool = crate::core::listing::extend_with_parents(ctx.repo.store(), candidates)?;
    let tag_metas = ctx.repo.store().list_tag_metas()?;
    let task_dates = ctx.repo.task_git_dates_for(&pool);

    let entries = forecast::build_entries(
        &pool,
        &state,
        &ctx.repo.scoring,
        &tag_metas,
        &filter_set,
        today,
        horizon,
        &task_dates,
    );

    Ok((entries, horizon))
}

pub fn run(args: Args, ctx: &AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();
    let (entries, horizon) = build_entries(&args, ctx)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No tasks due in the next {horizon} days.");
        return Ok(());
    }

    let (mut overdue, mut today_vec, mut week_vec, mut month_vec, mut later_vec) =
        (vec![], vec![], vec![], vec![], vec![]);

    for e in &entries {
        let days = (e.date - today).num_days();
        match days {
            d if d < 0 => overdue.push(e),
            0 => today_vec.push(e),
            1..=7 => week_vec.push(e),
            8..=30 => month_vec.push(e),
            _ => later_vec.push(e),
        }
    }

    print_section("Overdue", &overdue, today);
    print_section("Today", &today_vec, today);
    print_section("This week", &week_vec, today);
    print_section("This month", &month_vec, today);
    print_section(&format!("Next {horizon} days"), &later_vec, today);

    Ok(())
}

fn print_section(label: &str, entries: &[&ForecastEntry], today: NaiveDate) {
    if entries.is_empty() {
        return;
    }
    println!("\n{label}");
    println!("{}", "─".repeat(label.len()));
    for e in entries {
        let date = e.date;
        let days = (date - today).num_days();
        let due_label = match days {
            d if d < 0 => format!("{date} ({} days overdue)", -d),
            0 => format!("{date} (today)"),
            1 => format!("{date} (tomorrow)"),
            d => format!("{date} (in {d} days)"),
        };
        let marker = if e.projected { " (projected)" } else { "" };
        println!(
            "  [{}] {:<45}  {due_label}{marker}",
            e.id,
            crate::cli::render::truncate(&e.title, 45)
        );
    }
}
