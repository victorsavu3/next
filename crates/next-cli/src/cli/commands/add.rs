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

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    println!("not yet implemented: add (title={})", args.title);
    Ok(())
}
