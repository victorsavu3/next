use crate::core::parse_value;
use crate::core::domain::task::validate_key;
use crate::{core::resolve::resolve_task_id, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: DataSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum DataSubcommand {
    /// Set a single data key to a value.
    Set(SetArgs),
    /// Remove a single data key.
    Unset(UnsetArgs),
    /// Print the value of a single data key.
    Get(GetArgs),
}

#[derive(clap::Args, Debug)]
pub struct SetArgs {
    /// Task: UUID, UUID prefix, or slug.
    pub id: String,
    /// Key to set.
    pub key: String,
    /// Value — parsed as JSON (number, bool, array, object); bare strings are stored as-is.
    pub value: String,
}

#[derive(clap::Args, Debug)]
pub struct UnsetArgs {
    /// Task: UUID, UUID prefix, or slug.
    pub id: String,
    /// Key to remove.
    pub key: String,
}

#[derive(clap::Args, Debug)]
pub struct GetArgs {
    /// Task: UUID, UUID prefix, or slug.
    pub id: String,
    /// Key to retrieve.
    pub key: String,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        DataSubcommand::Set(a) => set(ctx, a),
        DataSubcommand::Unset(a) => unset(ctx, a),
        DataSubcommand::Get(a) => get(ctx, a),
    }
}

fn set(ctx: &mut AppContext, args: SetArgs) -> anyhow::Result<()> {
    validate_key(&args.key)?;
    let id = resolve_task_id(&*ctx.repo.store, &args.id)?;
    let value = parse_value(&args.value)?;

    ctx.repo.transaction(|store, vcs, root| {
        let mut task = store.get_task(id)?;
        task.data.insert(args.key.clone(), value.clone());

        let task_path = crate::core::storage::task_path(root, &task);
        store.save_task(&task)?;
        vcs.commit(
            &[task_path],
            &format!("next: data set {} {} on {}", args.key, value, task.title),
        )?;
        tracing::info!(cmd = "data", "[{}] set {}={}", &task.id.to_string()[..8], args.key, value);
        Ok(())
    })?;
    ctx.repo.record_task_event("data", id);
    Ok(())
}

fn unset(ctx: &mut AppContext, args: UnsetArgs) -> anyhow::Result<()> {
    validate_key(&args.key)?;
    let id = resolve_task_id(&*ctx.repo.store, &args.id)?;

    ctx.repo.transaction(|store, vcs, root| {
        let mut task = store.get_task(id)?;

        if !task.data.contains_key(&args.key) {
            anyhow::bail!("task [{}] has no data key {:?}", &task.id.to_string()[..8], args.key);
        }
        task.data.remove(&args.key);

        let task_path = crate::core::storage::task_path(root, &task);
        store.save_task(&task)?;
        vcs.commit(
            &[task_path],
            &format!("next: data unset {} on {}", args.key, task.title),
        )?;
        tracing::info!(cmd = "data", "[{}] unset {}", &task.id.to_string()[..8], args.key);
        Ok(())
    })?;
    ctx.repo.record_task_event("data", id);
    Ok(())
}

fn get(ctx: &mut AppContext, args: GetArgs) -> anyhow::Result<()> {
    validate_key(&args.key)?;
    let id = resolve_task_id(&*ctx.repo.store, &args.id)?;
    let task = ctx.repo.store.get_task(id)?;

    let value = task
        .data
        .get(&args.key)
        .ok_or_else(|| anyhow::anyhow!("task [{}] has no data key {:?}", &task.id.to_string()[..8], args.key))?;

    println!("{value}");
    Ok(())
}
