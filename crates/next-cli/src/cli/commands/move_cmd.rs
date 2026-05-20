use next::domain::task::Stage;

use crate::{resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
#[command(name = "move")]
pub struct Args {
    /// Task to move: UUID, UUID prefix, or slug.
    pub id: String,

    /// New parent task: UUID, UUID prefix, or slug. Pass "none" to remove.
    #[arg(long)]
    pub parent: Option<String>,

    /// New GTD stage (inbox, project, waiting, someday).
    #[arg(long)]
    pub stage: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let mut task = ctx.store.get_task(id)?;

    if let Some(ref stage_str) = args.stage {
        task.stage = parse_stage(stage_str)?;
    }

    match args.parent.as_deref() {
        Some("none") => task.parent_id = None,
        Some(parent_ref) => {
            task.parent_id = Some(resolve_task_id(&*ctx.store, parent_ref)?);
        }
        None => {}
    }

    task.touch();
    ctx.store.save_task(&task)?;

    let task_path = next_storage::task_path(&ctx.repo_root, &task);
    ctx.vcs
        .commit(&[task_path], &format!("next: move {}", task.title))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        ctx.log
            .info("move", &format!("[{}] {}", &task.id.to_string()[..8], task.title));
    }
    Ok(())
}

fn parse_stage(s: &str) -> anyhow::Result<Stage> {
    match s.to_lowercase().as_str() {
        "inbox" => Ok(Stage::Inbox),
        "project" => Ok(Stage::Project),
        "waiting" => Ok(Stage::Waiting),
        "someday" => Ok(Stage::Someday),
        _ => anyhow::bail!("unknown stage {s:?} — expected inbox, project, waiting, or someday"),
    }
}
