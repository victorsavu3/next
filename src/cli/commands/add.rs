use chrono::Local;
use crate::cli::recurrence_parse::parse_recurrence;
use crate::domain::{
    date_parse::parse_date,
    service::{create_task, CreateTaskParams},
};

use crate::AppContext;

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

    /// Calendar snap applied to the computed next occurrence date.
    /// Values: next-workday, monday … sunday, dom:N (day-of-month).
    #[arg(long)]
    pub recur_snap: Option<String>,

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

    let due = args.due.map(|expr| parse_date(&expr, today)).transpose()?;
    let start = args.start.map(|expr| parse_date(&expr, today)).transpose()?;

    let anchor = start.or(due).unwrap_or(today);
    let recurrence = parse_recurrence(
        args.recur_schedule,
        args.recur_completion,
        args.recur_snap.as_deref(),
        anchor,
    )?;

    let params = CreateTaskParams {
        due,
        start,
        priority: args.priority,
        slug: args.slug,
        assignee: args.assignee,
        tags: args.tags,
        parent: args.parent,
        blocked_by: args.blocked_by,
        description: args.description,
        url: args.url,
        notes: args.notes,
        long_term: args.long_term,
        score_adjustment: args.adjust,
        recurrence,
    };

    let task = create_task(
        args.title,
        params,
        today,
        &ctx.repo_root.clone(),
        &mut *ctx.store,
        &*ctx.vcs,
    )?;

    let short_id = task.id.to_string().replace('-', "")[..8].to_owned();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        ctx.log.info("add", &format!("added [{}] {}", short_id, task.title));
    }

    Ok(())
}
