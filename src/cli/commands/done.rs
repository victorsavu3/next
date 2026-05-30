use crate::{domain::recurrence::spawn_next, resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to mark done: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let mut task = ctx.store.get_task(id)?;
    task.mark_done();
    ctx.store.save_task(&task)?;

    let task_path = crate::storage::task_path(&ctx.repo_root, &task);
    let mut paths = vec![task_path];

    let today = chrono::Local::now().date_naive();
    if let Some(next) = spawn_next(&task, today) {
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
