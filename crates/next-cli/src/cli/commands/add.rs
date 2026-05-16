use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task title.
    pub title: String,

    /// Due date (ISO 8601 or natural language).
    #[arg(long)]
    pub due: Option<String>,

    /// Start date — task is hidden until this date.
    #[arg(long)]
    pub start: Option<String>,

    /// Task priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// Project path to assign the task to.
    #[arg(long)]
    pub project: Option<String>,

    /// Tags to attach (repeatable).
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Parent task ID.
    #[arg(long)]
    pub parent: Option<String>,

    /// IDs of tasks that block this one (repeatable).
    #[arg(long = "blocked-by", action = clap::ArgAction::Append)]
    pub blocked_by: Vec<String>,

    /// Additional notes.
    #[arg(long)]
    pub notes: Option<String>,

    /// Initial stage (inbox, next, waiting, someday).
    #[arg(long)]
    pub stage: Option<String>,

    /// Identifier of a task or date to wait for.
    #[arg(long)]
    pub wait_for: Option<String>,

    /// Recurrence schedule rule, e.g. "every Monday".
    #[arg(long)]
    pub recur_schedule: Option<String>,

    /// Recurrence completion interval in days.
    #[arg(long)]
    pub recur_completion: Option<u32>,

    /// Mark task as long-term.
    #[arg(long)]
    pub long_term: bool,

    /// Adjust due/start dates relative to today.
    #[arg(long)]
    pub adjust: bool,

    /// Output result as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, _ctx: &mut AppContext) -> anyhow::Result<()> {
    println!("not yet implemented: add (title={})", args.title);
    Ok(())
}
