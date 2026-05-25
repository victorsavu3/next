use crate::{resolve::resolve_task_id, AppContext};

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
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let mut task = ctx.store.get_task(id)?;

    let value = parse_value(&args.value)?;
    task.data.insert(args.key.clone(), value.clone());
    task.touch();

    let task_path = next_storage::task_path(&ctx.repo_root, &task);
    ctx.store.save_task(&task)?;
    ctx.vcs.commit(
        &[task_path],
        &format!("next: data set {} {} on {}", args.key, value, task.title),
    )?;
    ctx.log.info(
        "data",
        &format!("[{}] set {}={}", &task.id.to_string()[..8], args.key, value),
    );
    Ok(())
}

fn unset(ctx: &mut AppContext, args: UnsetArgs) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let mut task = ctx.store.get_task(id)?;

    if !task.data.contains_key(&args.key) {
        anyhow::bail!("task [{}] has no data key {:?}", &task.id.to_string()[..8], args.key);
    }
    task.data.remove(&args.key);
    task.touch();

    let task_path = next_storage::task_path(&ctx.repo_root, &task);
    ctx.store.save_task(&task)?;
    ctx.vcs.commit(
        &[task_path],
        &format!("next: data unset {} on {}", args.key, task.title),
    )?;
    ctx.log.info(
        "data",
        &format!("[{}] unset {}", &task.id.to_string()[..8], args.key),
    );
    Ok(())
}

fn get(ctx: &mut AppContext, args: GetArgs) -> anyhow::Result<()> {
    let id = resolve_task_id(&*ctx.store, &args.id)?;
    let task = ctx.store.get_task(id)?;

    let value = task
        .data
        .get(&args.key)
        .ok_or_else(|| anyhow::anyhow!("task [{}] has no data key {:?}", &task.id.to_string()[..8], args.key))?;

    println!("{value}");
    Ok(())
}

/// Parse a value string as JSON; bare strings that are not valid JSON are stored as strings.
pub fn parse_value(raw: &str) -> anyhow::Result<serde_json::Value> {
    let value: serde_json::Value =
        serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()));
    if value.is_null() {
        anyhow::bail!("data value must not be null");
    }
    Ok(value)
}
