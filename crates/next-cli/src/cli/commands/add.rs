use chrono::Local;
use next::domain::{
    date_parse::parse_date,
    task::{Priority, Recurrence, Stage, Task},
};

use crate::{resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task title.
    pub title: String,

    /// Due date (ISO 8601 or natural language, e.g. "in two weeks").
    #[arg(long)]
    pub due: Option<String>,

    /// Start date — task is hidden until this date.
    #[arg(long)]
    pub start: Option<String>,

    /// Task priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// User-provided slug for stable referencing (e.g. "water-plants").
    #[arg(long)]
    pub slug: Option<String>,

    /// Tags to attach (repeatable). Use @ for context, $ for resource.
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Parent task: UUID, UUID prefix, or slug.
    #[arg(long)]
    pub parent: Option<String>,

    /// Explicit blocker task: UUID, UUID prefix, or slug (repeatable).
    #[arg(long = "blocked-by", action = clap::ArgAction::Append)]
    pub blocked_by: Vec<String>,

    /// Free-text notes.
    #[arg(long)]
    pub notes: Option<String>,

    /// GTD stage (inbox, project, waiting, someday). Default: inbox.
    #[arg(long)]
    pub stage: Option<String>,

    /// Who this task is waiting on (sets stage to waiting).
    #[arg(long)]
    pub wait_for: Option<String>,

    /// Schedule-based recurrence rule, e.g. "every Monday".
    #[arg(long)]
    pub recur_schedule: Option<String>,

    /// Completion-based recurrence interval in days.
    #[arg(long)]
    pub recur_completion: Option<u32>,

    /// Suppress age-based scoring (suitable for long-running tasks).
    #[arg(long)]
    pub long_term: bool,

    /// Manual urgency score adjustment (positive boosts, negative penalises).
    #[arg(long)]
    pub adjust: Option<f64>,

    /// Output result as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let today = Local::now().date_naive();

    let mut task = Task::new(args.title);

    if let Some(expr) = args.due {
        task.due = Some(parse_date(&expr, today)?);
    }
    if let Some(expr) = args.start {
        task.start = Some(parse_date(&expr, today)?);
    }

    if let Some(p) = args.priority {
        task.priority = parse_priority(&p)?;
    }

    task.slug = args.slug;
    task.tags = args.tags;
    task.notes = args.notes;
    task.long_term = args.long_term;

    if let Some(adj) = args.adjust {
        task.score_adjustment = adj;
    }

    if let Some(ref stage) = args.stage {
        task.stage = parse_stage(stage)?;
    }

    if let Some(wait) = args.wait_for {
        task.waiting_for = Some(wait);
        task.stage = Stage::Waiting;
    }

    if let Some(rule) = args.recur_schedule {
        task.recurrence = Some(Recurrence::Schedule { rule });
    } else if let Some(interval) = args.recur_completion {
        task.recurrence = Some(Recurrence::Completion {
            interval_days: interval,
        });
    }

    if let Some(ref parent_ref) = args.parent {
        task.parent_id = Some(resolve_task_id(&*ctx.store, parent_ref)?);
    }

    for blocker_ref in &args.blocked_by {
        task.blocked_by
            .push(resolve_task_id(&*ctx.store, blocker_ref)?);
    }

    let short_id = task.id.to_string().replace('-', "")[..8].to_owned();
    let task_path = next_storage::task_path(&ctx.repo_root, &task);

    ctx.store.save_task(&task)?;
    ctx.vcs
        .commit(&[task_path], &format!("next: add {}", task.title))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        ctx.log.info("add", &format!("added [{}] {}", short_id, task.title));
    }

    Ok(())
}

fn parse_priority(s: &str) -> anyhow::Result<Priority> {
    match s.to_lowercase().as_str() {
        "low" => Ok(Priority::Low),
        "medium" | "med" => Ok(Priority::Medium),
        "high" => Ok(Priority::High),
        _ => anyhow::bail!("unknown priority {s:?} — expected low, medium, or high"),
    }
}

fn parse_stage(s: &str) -> anyhow::Result<Stage> {
    match s.to_lowercase().as_str() {
        "inbox" => Ok(Stage::Inbox),
        "project" => Ok(Stage::Project),
        "waiting" => Ok(Stage::Waiting),
        "someday" => Ok(Stage::Someday),
        _ => anyhow::bail!("unknown stage {s:?} — expected inbox, project, waiting, or someday"),
    }
}
