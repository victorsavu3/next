use chrono::{Local, NaiveDate};
use next::domain::{filter, scoring};

use crate::cli::filter::FilterArgs;
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Number of days to forecast (default from config, usually 90).
    #[arg(long)]
    pub days: Option<u32>,

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
    let today = Local::now().date_naive();
    let horizon = args.days.unwrap_or(ctx.config.forecast_horizon_days);

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

    let cutoff = today + chrono::Duration::days(horizon as i64);
    let due_tasks: Vec<_> = scored
        .iter()
        .filter(|st| st.task.due.is_some_and(|d| d <= cutoff))
        .collect();

    if filter_args.json {
        println!("{}", serde_json::to_string_pretty(&due_tasks)?);
        return Ok(());
    }

    if due_tasks.is_empty() {
        println!("No tasks due in the next {horizon} days.");
        return Ok(());
    }

    let (mut overdue, mut today_vec, mut week_vec, mut month_vec, mut later_vec) =
        (vec![], vec![], vec![], vec![], vec![]);

    for st in &due_tasks {
        let days = (st.task.due.unwrap() - today).num_days();
        match days {
            d if d < 0 => overdue.push(st),
            0 => today_vec.push(st),
            1..=7 => week_vec.push(st),
            8..=30 => month_vec.push(st),
            _ => later_vec.push(st),
        }
    }

    print_section("Overdue", &overdue, today);
    print_section("Today", &today_vec, today);
    print_section("This week", &week_vec, today);
    print_section("This month", &month_vec, today);
    print_section(&format!("Next {horizon} days"), &later_vec, today);

    Ok(())
}

fn print_section(
    label: &str,
    tasks: &[&&next::domain::scoring::ScoredTask],
    today: NaiveDate,
) {
    if tasks.is_empty() {
        return;
    }
    println!("\n{label}");
    println!("{}", "─".repeat(label.len()));
    for st in tasks {
        let short = &st.task.id.to_string().replace('-', "")[..8];
        let due = st.task.due.unwrap();
        let days = (due - today).num_days();
        let due_label = match days {
            d if d < 0 => format!("{due} ({} days overdue)", -d),
            0 => format!("{due} (today)"),
            1 => format!("{due} (tomorrow)"),
            d => format!("{due} (in {d} days)"),
        };
        println!(
            "  [{short}] {:<45}  {due_label}",
            truncate(&st.task.title, 45)
        );
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}
