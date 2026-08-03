use crate::{core::resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
#[command(name = "move")]
pub struct Args {
    /// Task to move: UUID, UUID prefix, or slug.
    pub id: String,

    /// New parent task: UUID, UUID prefix, or slug. Pass "none" to remove.
    #[arg(long)]
    pub parent: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.repo.store, &args.id)?;
    let parent = args.parent.clone();
    let task = ctx.repo.transaction(|store, vcs, root| {
        let mut task = store.get_task(id)?;

        match parent.as_deref() {
            Some("none") => task.parent_id = None,
            Some(parent_ref) => {
                task.parent_id = Some(resolve_task_id(&*store, parent_ref)?);
            }
            None => {}
        }

        store.save_task(&task)?;

        let task_path = crate::core::storage::task_path(root, &task);
        vcs.commit(&[task_path], &format!("next: move {}", task.title))?;
        Ok(task)
    })?;
    ctx.repo.record_task_event("move", task.id);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        println!("moved [{}] {}", &task.id.to_string()[..8], task.title);
        tracing::info!(
            cmd = "move",
            "[{}] {}",
            &task.id.to_string()[..8],
            task.title
        );
    }
    Ok(())
}
