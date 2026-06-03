use crate::{resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Task to stop: UUID, UUID prefix, or slug.
    pub id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let task = ctx.transaction(|store, vcs, root| {
        let mut task = store.get_task(id)?;
        task.mark_stopped();
        store.save_task(&task)?;
        let task_path = crate::storage::task_path(root, &task);
        vcs.commit(&[task_path], &format!("next: stop {}", task.title))?;
        Ok(task)
    })?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        tracing::info!(cmd = "stop", "[{}] {}", &task.id.to_string()[..8], task.title);
    }
    Ok(())
}
