use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task ID to edit.
    pub id: String,

    /// New title.
    #[arg(long)]
    pub title: Option<String>,

    /// Due date.
    #[arg(long)]
    pub due: Option<String>,

    /// Start date.
    #[arg(long)]
    pub start: Option<String>,

    /// Priority (low, medium, high).
    #[arg(long)]
    pub priority: Option<String>,

    /// Project path.
    #[arg(long)]
    pub project: Option<String>,

    /// Tags to attach (repeatable).
    #[arg(long = "tag", action = clap::ArgAction::Append)]
    pub tags: Vec<String>,

    /// Tags to remove (repeatable).
    #[arg(long = "remove-tag", action = clap::ArgAction::Append)]
    pub remove_tags: Vec<String>,

    /// Parent task ID.
    #[arg(long)]
    pub parent: Option<String>,

    /// IDs of tasks that block this one (repeatable).
    #[arg(long = "blocked-by", action = clap::ArgAction::Append)]
    pub blocked_by: Vec<String>,

    /// Additional notes.
    #[arg(long)]
    pub notes: Option<String>,

    /// Stage.
    #[arg(long)]
    pub stage: Option<String>,

    /// Identifier of a task or date to wait for.
    #[arg(long)]
    pub wait_for: Option<String>,

    /// Recurrence schedule rule.
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

    /// Clear the due date.
    #[arg(long)]
    pub clear_due: bool,

    /// Clear the start date.
    #[arg(long)]
    pub clear_start: bool,

    /// Clear the parent link.
    #[arg(long)]
    pub clear_parent: bool,

    /// Clear all blocked-by links.
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
