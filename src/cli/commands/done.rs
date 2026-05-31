use crate::{domain::{date_parse::parse_date, recurrence::spawn_next}, resolve::resolve_task_id, AppContext};

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
    let mut task = ctx.store.get_task(id)?;
    task.mark_done();
    ctx.store.save_task(&task)?;

    let task_path = crate::storage::task_path(&ctx.repo_root, &task);
    let mut paths = vec![task_path];

    if let Some(next) = spawn_next(&task, completion_date)? {
        let next_path = crate::storage::task_path(&ctx.repo_root, &next);
        ctx.store.save_task(&next)?;
        paths.push(next_path);
    }

    ctx.vcs
        .commit(&paths, &format!("next: done {}", task.title))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        ctx.log
            .info("done", &format!("[{}] {}", &task.id.to_string()[..8], task.title));
    }
    Ok(())
}
