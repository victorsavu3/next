use chrono::{Local, NaiveDate};
use serde::Serialize;

use crate::core::{domain::filter, recurrence, scoring};

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

/// A single occurrence in the forecast: either a concrete existing task or a
/// projected (not-yet-spawned) future instance of a schedule-type series.
#[derive(Debug, Clone, Serialize)]
pub struct ForecastEntry {
    /// The forecast date (the task's `due`, or the projected occurrence date).
    pub date: NaiveDate,
    /// Short 8-char id of the originating task (the open instance for projected ones).
    pub id: String,
    pub title: String,
    /// Urgency score of the originating task.
    pub score: f64,
    /// `true` for projected future occurrences that do not yet exist as tasks.
    pub projected: bool,
}

/// Build the chronologically-ordered forecast entries for the given horizon:
/// concrete existing tasks due within the window plus projected future
/// occurrences of active schedule-type recurrence series. The horizon (in days)
/// is returned alongside so callers can render the section headings.
pub fn build_entries(args: &Args, ctx: &AppContext) -> anyhow::Result<(Vec<ForecastEntry>, u32)> {
    let today = Local::now().date_naive();
    let horizon = args.days.unwrap_or(ctx.config.forecast_horizon_days);

    let mut filter_args = FilterArgs::parse(args.tokens.clone());
    filter_args.future = args.future;
    filter_args.all = args.all;
    filter_args.all_users = args.all_users;
    filter_args.json = args.json;

    let filter_set = filter_args.to_filter_set()?;
    let state = ctx.repo.store().get_state()?;
    let all_tasks = ctx.repo.store().list_tasks()?;
    let tag_metas = ctx.repo.store().list_tag_metas()?;

    let filtered = filter::apply(all_tasks.clone(), &filter_set, &state, today);
    let scored = scoring::score_and_sort(filtered, &all_tasks, today, &ctx.repo.scoring, &tag_metas);

    let cutoff = today + chrono::Duration::days(horizon as i64);

    let mut entries: Vec<ForecastEntry> = Vec::new();
    for st in &scored {
        let short = st.task.id.to_string().replace('-', "")[..8].to_string();

        // Concrete existing task whose stored due falls within the horizon.
        if st.task.due.is_some_and(|d| d <= cutoff) {
            entries.push(ForecastEntry {
                date: st.task.due.unwrap(),
                id: short.clone(),
                title: st.task.title.clone(),
                score: st.score,
                projected: false,
            });
        }

        // Project the recurrence series forward. Only schedule-type series have
        // deterministic future dates; completion-type series depend on unknown
        // future completion dates and cannot be projected, so they are skipped.
        for date in recurrence::project_series(&st.task, today, cutoff) {
            entries.push(ForecastEntry {
                date,
                id: short.clone(),
                title: st.task.title.clone(),
                score: st.score,
                projected: true,
            });
        }
    }

    // Order chronologically; concrete tasks sort before projected ones on the
    // same date so the current instance is shown ahead of its projections.
    entries.sort_by(|a, b| a.date.cmp(&b.date).then(a.projected.cmp(&b.projected)));

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
            truncate(&e.title, 45)
        );
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((byte_pos, _)) => &s[..byte_pos],
        None => s,
    }
}
