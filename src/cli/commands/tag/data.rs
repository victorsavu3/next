use crate::domain::tag;
use crate::domain::tag::TagMeta;
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct DataArgs {
    #[command(subcommand)]
    pub subcommand: DataSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum DataSubcommand {
    /// Store an arbitrary value under a key for a tag.
    Set(DataSetArgs),
    /// Print the value stored under a key for a tag.
    Get(DataGetArgs),
    /// Remove a key from a tag's data.
    Unset(DataUnsetArgs),
    /// List all data keys and values for a tag.
    List(DataListArgs),
}

#[derive(clap::Args, Debug)]
pub struct DataSetArgs {
    /// Tag to update.
    pub tag: String,
    /// Key name.
    pub key: String,
    /// Value (any valid JSON literal, or a bare string).
    pub value: String,
}

#[derive(clap::Args, Debug)]
pub struct DataGetArgs {
    /// Tag to query.
    pub tag: String,
    /// Key name.
    pub key: String,
}

#[derive(clap::Args, Debug)]
pub struct DataUnsetArgs {
    /// Tag to update.
    pub tag: String,
    /// Key name to remove.
    pub key: String,
}

#[derive(clap::Args, Debug)]
pub struct DataListArgs {
    /// Tag to query.
    pub tag: String,
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub fn run(ctx: &mut AppContext, args: DataArgs) -> anyhow::Result<()> {
    match args.subcommand {
        DataSubcommand::Set(a) => set(ctx, a),
        DataSubcommand::Get(a) => get(ctx, a),
        DataSubcommand::Unset(a) => unset(ctx, a),
        DataSubcommand::List(a) => list(ctx, a),
    }
}

fn set(ctx: &mut AppContext, args: DataSetArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let value: serde_json::Value = serde_json::from_str(&args.value)
        .unwrap_or_else(|_| serde_json::Value::String(args.value.clone()));
    let mut meta = ctx.store.get_tag_meta(&args.tag)?.unwrap_or_default();
    meta.data.insert(args.key.clone(), value);
    ctx.store.set_tag_meta(&args.tag, meta)?;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    ctx.vcs.commit(
        &[tag_path],
        &format!("next: tag data set {} {}", args.tag, args.key),
    )?;
    ctx.log
        .info("tag", &format!("set data {}.{} = {}", args.tag, args.key, args.value));
    Ok(())
}

fn get(ctx: &mut AppContext, args: DataGetArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let meta = ctx.store.get_tag_meta(&args.tag)?.unwrap_or_default();
    match meta.data.get(&args.key) {
        Some(v) => println!("{v}"),
        None => anyhow::bail!("key {:?} not found for tag {:?}", args.key, args.tag),
    }
    Ok(())
}

fn unset(ctx: &mut AppContext, args: DataUnsetArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut meta = ctx
        .store
        .get_tag_meta(&args.tag)?
        .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {:?}", args.tag))?;
    if meta.data.remove(&args.key).is_none() {
        anyhow::bail!("key {:?} not found for tag {:?}", args.key, args.tag);
    }
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    if meta == TagMeta::default() {
        ctx.store.delete_tag_meta(&args.tag)?;
    } else {
        ctx.store.set_tag_meta(&args.tag, meta)?;
    }
    ctx.vcs.commit(
        &[tag_path],
        &format!("next: tag data unset {} {}", args.tag, args.key),
    )?;
    ctx.log
        .info("tag", &format!("unset data {}.{}", args.tag, args.key));
    Ok(())
}

fn list(ctx: &mut AppContext, args: DataListArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let meta = ctx.store.get_tag_meta(&args.tag)?.unwrap_or_default();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&meta.data)?);
        return Ok(());
    }

    if meta.data.is_empty() {
        println!("No data set for {}.", args.tag);
        return Ok(());
    }

    let mut keys: Vec<&String> = meta.data.keys().collect();
    keys.sort();
    for k in keys {
        println!("{k} = {}", meta.data[k]);
    }
    Ok(())
}
