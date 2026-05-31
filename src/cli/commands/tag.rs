use std::collections::BTreeSet;

use crate::domain::tag::{self, TagKind, TagMeta};
use crate::domain::task::Priority;
use crate::AppContext;

/// Top-level `next tag` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: Option<TagSubcommand>,
}

#[derive(clap::Subcommand, Debug)]
pub enum TagSubcommand {
    /// Show all metadata for a tag.
    Show(ShowArgs),
    /// Set a human-readable description for a tag.
    Describe(DescribeArgs),
    /// Remove the description for a tag.
    #[command(name = "clear-description")]
    ClearDescription(ClearDescriptionArgs),
    /// Set a reference URL for a tag.
    #[command(name = "set-url")]
    SetUrl(SetUrlArgs),
    /// Remove the URL for a tag.
    #[command(name = "clear-url")]
    ClearUrl(ClearUrlArgs),
    /// Set the default priority for tasks tagged with this tag.
    #[command(name = "set-priority")]
    SetPriority(SetPriorityArgs),
    /// Remove the priority override for a tag.
    #[command(name = "clear-priority")]
    ClearPriority(ClearPriorityArgs),
    /// Disable time-based urgency (age + due-date factors) for tasks with this tag.
    #[command(name = "set-no-time-urgency")]
    SetNoTimeUrgency(NoTimeUrgencyArgs),
    /// Re-enable time-based urgency for tasks with this tag.
    #[command(name = "clear-no-time-urgency")]
    ClearNoTimeUrgency(NoTimeUrgencyArgs),
    /// Manage arbitrary key/value data for a tag.
    Data(DataArgs),
}

#[derive(clap::Args, Debug)]
pub struct ShowArgs {
    /// Tag to show (e.g. @work, #printer, python).
    pub tag: String,
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct DescribeArgs {
    /// Tag to describe (e.g. @work, #printer, python).
    pub tag: String,
    /// Description text.
    pub description: String,
}

#[derive(clap::Args, Debug)]
pub struct ClearDescriptionArgs {
    /// Tag whose description should be removed.
    pub tag: String,
}

#[derive(clap::Args, Debug)]
pub struct SetUrlArgs {
    /// Tag to update.
    pub tag: String,
    /// URL to associate with this tag.
    pub url: String,
}

#[derive(clap::Args, Debug)]
pub struct ClearUrlArgs {
    /// Tag whose URL should be removed.
    pub tag: String,
}

#[derive(clap::Args, Debug)]
pub struct SetPriorityArgs {
    /// Tag to update.
    pub tag: String,
    /// Priority: low, medium, or high.
    pub priority: String,
}

#[derive(clap::Args, Debug)]
pub struct ClearPriorityArgs {
    /// Tag whose priority override should be removed.
    pub tag: String,
}

#[derive(clap::Args, Debug)]
pub struct NoTimeUrgencyArgs {
    /// Tag to update (e.g. @wishlist, someday).
    pub tag: String,
}

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

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => list(ctx),
        Some(TagSubcommand::Show(a)) => show(ctx, a),
        Some(TagSubcommand::Describe(a)) => describe(ctx, a),
        Some(TagSubcommand::ClearDescription(a)) => clear_description(ctx, a),
        Some(TagSubcommand::SetUrl(a)) => set_url(ctx, a),
        Some(TagSubcommand::ClearUrl(a)) => clear_url(ctx, a),
        Some(TagSubcommand::SetPriority(a)) => set_priority(ctx, a),
        Some(TagSubcommand::ClearPriority(a)) => clear_priority(ctx, a),
        Some(TagSubcommand::SetNoTimeUrgency(a)) => set_no_time_urgency(ctx, a),
        Some(TagSubcommand::ClearNoTimeUrgency(a)) => clear_no_time_urgency(ctx, a),
        Some(TagSubcommand::Data(a)) => data(ctx, a),
    }
}

fn list(ctx: &mut AppContext) -> anyhow::Result<()> {
    let metas = ctx.store.list_tag_metas()?;
    let tasks = ctx.store.list_tasks()?;

    let mut all_tags: BTreeSet<String> = BTreeSet::new();
    for task in &tasks {
        for t in &task.tags {
            all_tags.insert(t.clone());
        }
    }
    for t in metas.keys() {
        all_tags.insert(t.clone());
    }

    if all_tags.is_empty() {
        println!("No tags found.");
        return Ok(());
    }

    let mut contexts: Vec<&str> = Vec::new();
    let mut resources: Vec<&str> = Vec::new();
    let mut freeform: Vec<&str> = Vec::new();

    for t in &all_tags {
        match tag::classify(t) {
            TagKind::Context => contexts.push(t.as_str()),
            TagKind::Resource => resources.push(t.as_str()),
            TagKind::Freeform => freeform.push(t.as_str()),
        }
    }

    print_group("Contexts (@)", &contexts, &metas);
    print_group("Resources (#)", &resources, &metas);
    print_group("Freeform", &freeform, &metas);

    Ok(())
}

fn print_group(header: &str, tags: &[&str], metas: &std::collections::HashMap<String, TagMeta>) {
    if tags.is_empty() {
        return;
    }
    println!("{header}");
    for &t in tags {
        match metas.get(t).and_then(|m| m.description.as_deref()) {
            Some(desc) => println!("  {t:<28}  {desc}"),
            None => println!("  {t}"),
        }
    }
    println!();
}

fn show(ctx: &mut AppContext, args: ShowArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let meta = ctx.store.get_tag_meta(&args.tag)?.unwrap_or_default();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&meta)?);
        return Ok(());
    }

    println!("Tag: {}", args.tag);
    if let Some(ref desc) = meta.description {
        println!("  description: {desc}");
    }
    if let Some(ref url) = meta.url {
        println!("  url:         {url}");
    }
    if let Some(ref p) = meta.priority {
        println!("  priority:    {}", priority_display(p));
    }
    if meta.no_time_urgency {
        println!("  no-time-urgency: true");
    }
    if !meta.data.is_empty() {
        println!("  data:");
        let mut keys: Vec<&String> = meta.data.keys().collect();
        keys.sort();
        for k in keys {
            println!("    {k} = {}", meta.data[k]);
        }
    }
    Ok(())
}

fn describe(ctx: &mut AppContext, args: DescribeArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.store.set_tag_description(&args.tag, &args.description)?;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    ctx.vcs
        .commit(&[tag_path], &format!("next: tag describe {}", args.tag))?;
    ctx.log
        .info("tag", &format!("described {} = {}", args.tag, args.description));
    Ok(())
}

fn clear_description(ctx: &mut AppContext, args: ClearDescriptionArgs) -> anyhow::Result<()> {
    ctx.store.delete_tag_description(&args.tag)?;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    ctx.vcs.commit(
        &[tag_path],
        &format!("next: tag clear-description {}", args.tag),
    )?;
    ctx.log
        .info("tag", &format!("cleared description for {}", args.tag));
    Ok(())
}

fn set_url(ctx: &mut AppContext, args: SetUrlArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut meta = ctx.store.get_tag_meta(&args.tag)?.unwrap_or_default();
    meta.url = Some(args.url.clone());
    ctx.store.set_tag_meta(&args.tag, meta)?;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    ctx.vcs
        .commit(&[tag_path], &format!("next: tag set-url {}", args.tag))?;
    ctx.log
        .info("tag", &format!("set url for {} = {}", args.tag, args.url));
    Ok(())
}

fn clear_url(ctx: &mut AppContext, args: ClearUrlArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut meta = ctx
        .store
        .get_tag_meta(&args.tag)?
        .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {:?}", args.tag))?;
    meta.url = None;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    if meta == TagMeta::default() {
        ctx.store.delete_tag_meta(&args.tag)?;
    } else {
        ctx.store.set_tag_meta(&args.tag, meta)?;
    }
    ctx.vcs
        .commit(&[tag_path], &format!("next: tag clear-url {}", args.tag))?;
    ctx.log
        .info("tag", &format!("cleared url for {}", args.tag));
    Ok(())
}

fn set_priority(ctx: &mut AppContext, args: SetPriorityArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let priority = parse_priority(&args.priority)?;
    let mut meta = ctx.store.get_tag_meta(&args.tag)?.unwrap_or_default();
    meta.priority = Some(priority);
    ctx.store.set_tag_meta(&args.tag, meta)?;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    ctx.vcs
        .commit(&[tag_path], &format!("next: tag set-priority {}", args.tag))?;
    ctx.log.info(
        "tag",
        &format!("set priority for {} = {}", args.tag, args.priority),
    );
    Ok(())
}

fn clear_priority(ctx: &mut AppContext, args: ClearPriorityArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut meta = ctx
        .store
        .get_tag_meta(&args.tag)?
        .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {:?}", args.tag))?;
    meta.priority = None;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    if meta == TagMeta::default() {
        ctx.store.delete_tag_meta(&args.tag)?;
    } else {
        ctx.store.set_tag_meta(&args.tag, meta)?;
    }
    ctx.vcs
        .commit(&[tag_path], &format!("next: tag clear-priority {}", args.tag))?;
    ctx.log
        .info("tag", &format!("cleared priority for {}", args.tag));
    Ok(())
}

fn set_no_time_urgency(ctx: &mut AppContext, args: NoTimeUrgencyArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut meta = ctx.store.get_tag_meta(&args.tag)?.unwrap_or_default();
    meta.no_time_urgency = true;
    ctx.store.set_tag_meta(&args.tag, meta)?;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    ctx.vcs.commit(&[tag_path], &format!("next: tag set-no-time-urgency {}", args.tag))?;
    ctx.log.info("tag", &format!("set no-time-urgency for {}", args.tag));
    Ok(())
}

fn clear_no_time_urgency(ctx: &mut AppContext, args: NoTimeUrgencyArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut meta = ctx
        .store
        .get_tag_meta(&args.tag)?
        .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {:?}", args.tag))?;
    meta.no_time_urgency = false;
    let tag_path = crate::storage::tag_meta_path(&ctx.repo_root, &args.tag);
    if meta == TagMeta::default() {
        ctx.store.delete_tag_meta(&args.tag)?;
    } else {
        ctx.store.set_tag_meta(&args.tag, meta)?;
    }
    ctx.vcs.commit(&[tag_path], &format!("next: tag clear-no-time-urgency {}", args.tag))?;
    ctx.log.info("tag", &format!("cleared no-time-urgency for {}", args.tag));
    Ok(())
}

fn data(ctx: &mut AppContext, args: DataArgs) -> anyhow::Result<()> {
    match args.subcommand {
        DataSubcommand::Set(a) => data_set(ctx, a),
        DataSubcommand::Get(a) => data_get(ctx, a),
        DataSubcommand::Unset(a) => data_unset(ctx, a),
        DataSubcommand::List(a) => data_list(ctx, a),
    }
}

fn data_set(ctx: &mut AppContext, args: DataSetArgs) -> anyhow::Result<()> {
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

fn data_get(ctx: &mut AppContext, args: DataGetArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let meta = ctx.store.get_tag_meta(&args.tag)?.unwrap_or_default();
    match meta.data.get(&args.key) {
        Some(v) => println!("{v}"),
        None => anyhow::bail!("key {:?} not found for tag {:?}", args.key, args.tag),
    }
    Ok(())
}

fn data_unset(ctx: &mut AppContext, args: DataUnsetArgs) -> anyhow::Result<()> {
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

fn data_list(ctx: &mut AppContext, args: DataListArgs) -> anyhow::Result<()> {
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

fn parse_priority(s: &str) -> anyhow::Result<Priority> {
    match s.to_ascii_lowercase().as_str() {
        "low" => Ok(Priority::Low),
        "medium" => Ok(Priority::Medium),
        "high" => Ok(Priority::High),
        other => anyhow::bail!("unknown priority {other:?}: expected low, medium, or high"),
    }
}

fn priority_display(p: &Priority) -> &'static str {
    match p {
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
    }
}
