use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to edit: UUID, UUID prefix, or slug.
    pub id: String,

    /// New title.
    #[arg(long)]
    pub title: Option<String>,

    /// Due date (ISO 8601 or natural language).
    #[arg(long)]
    pub due: Option<String>,

    /// Start date.
    #[arg(long)]
    pub start: Option<String>,

    /// Priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// User-provided slug.
    #[arg(long)]
    pub slug: Option<String>,

    /// Tags to attach (repeatable).
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Tags to remove (repeatable).
    #[arg(long = "remove-tag", action = clap::ArgAction::Append)]
    pub remove_tags: Vec<String>,

    /// Parent task: UUID, UUID prefix, or slug.
    #[arg(long)]
    pub parent: Option<String>,

    /// Explicit blocker task (repeatable).
    #[arg(long = "blocked-by", action = clap::ArgAction::Append)]
    pub blocked_by: Vec<String>,

    /// Free-text notes.
    #[arg(long)]
    pub notes: Option<String>,

    /// GTD stage (inbox, project, waiting, someday).
    #[arg(long)]
    pub stage: Option<String>,

    /// Who this task is waiting on (sets stage to waiting).
    #[arg(long)]
    pub wait_for: Option<String>,

    /// Schedule-based recurrence rule.
    #[arg(long)]
    pub recur_schedule: Option<String>,

    /// Completion-based recurrence interval in days.
    #[arg(long)]
    pub recur_completion: Option<u32>,

    /// Suppress age-based scoring.
    #[arg(long)]
    pub long_term: bool,

    /// Manual urgency score adjustment.
    #[arg(long)]
    pub adjust: Option<f64>,

    /// Remove the due date.
    #[arg(long)]
    pub clear_due: bool,

    /// Remove the start date.
    #[arg(long)]
    pub clear_start: bool,

    /// Remove the parent link.
    #[arg(long)]
    pub clear_parent: bool,

    /// Remove all explicit blockers.
    #[arg(long)]
    pub clear_blocked_by: bool,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    println!("not yet implemented: edit (id={})", args.id);
    Ok(())
}
