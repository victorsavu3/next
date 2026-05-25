use chrono::Local;
use next::domain::{
    date_parse::parse_date,
    tag,
    task::{Priority, Recurrence, Task},
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

    /// Assign to a specific user.
    #[arg(long)]
    pub assignee: Option<String>,

    /// Tags to attach (repeatable). Use @ for context, # for resource.
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Parent task: UUID, UUID prefix, or slug.
    #[arg(long)]
    pub parent: Option<String>,

    /// Explicit blocker task: UUID, UUID prefix, or slug (repeatable).
    #[arg(long = "blocked-by", action = clap::ArgAction::Append)]
    pub blocked_by: Vec<String>,

    /// Longer description (multi-line context or detail).
    #[arg(long)]
    pub description: Option<String>,

    /// URL associated with this task (ticket, doc, reference link).
    #[arg(long)]
    pub url: Option<String>,

    /// Free-text notes.
    #[arg(long)]
    pub notes: Option<String>,

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
    task.assignee = args.assignee;
    for t in &args.tags {
        tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
    }
    task.tags = args.tags;
    task.description = args.description;
    if let Some(ref u) = args.url {
        validate_url(u)?;
    }
    task.url = args.url;
    task.notes = args.notes;
    task.long_term = args.long_term;

    if let Some(adj) = args.adjust {
        task.score_adjustment = adj;
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

pub fn validate_url(u: &str) -> anyhow::Result<()> {
    if u.starts_with("http://") || u.starts_with("https://") {
        Ok(())
    } else {
        anyhow::bail!("url must start with http:// or https://")
    }
}

fn parse_priority(s: &str) -> anyhow::Result<Priority> {
    match s.to_lowercase().as_str() {
        "low" => Ok(Priority::Low),
        "medium" | "med" => Ok(Priority::Medium),
        "high" => Ok(Priority::High),
        _ => anyhow::bail!("unknown priority {s:?} — expected low, medium, or high"),
    }
}

