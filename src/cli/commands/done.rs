use crate::{domain::{date_parse::parse_date, service::complete_task}, resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to mark done: UUID, UUID prefix, or slug.
    pub id: String,

    /// Date to treat as the completion date for recurrence scheduling.
    /// Accepts ISO 8601 or natural language ("yesterday", "2026-05-30").
    /// Defaults to today.
    #[arg(long)]
    pub completed_at: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let today = chrono::Local::now().date_naive();
    let completion_date = match args.completed_at {
        Some(ref expr) => parse_date(expr, today).map_err(anyhow::Error::from)?,
        None => today,
    };

    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let task = complete_task(
        id,
        completion_date,
        &ctx.repo_root.clone(),
        &mut *ctx.store,
        &*ctx.vcs,
    )?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        tracing::info!(cmd = "done", "[{}] {}", &task.id.to_string()[..8], task.title);
    }
    Ok(())
}
