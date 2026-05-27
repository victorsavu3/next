use std::collections::BTreeSet;

use next::domain::tag::{self, TagKind};

use crate::AppContext;

/// Top-level `next tag` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: Option<TagSubcommand>,
}

#[derive(clap::Subcommand, Debug)]
pub enum TagSubcommand {
    /// Set a human-readable description for a tag, context, or resource.
    Describe(DescribeArgs),
    /// Remove the description for a tag.
    #[command(name = "clear-description")]
    ClearDescription(ClearDescriptionArgs),
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

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => list(ctx),
        Some(TagSubcommand::Describe(a)) => describe(ctx, a),
        Some(TagSubcommand::ClearDescription(a)) => clear_description(ctx, a),
    }
}

fn list(ctx: &mut AppContext) -> anyhow::Result<()> {
    let descriptions = ctx.store.list_tag_descriptions()?;
    let tasks = ctx.store.list_tasks()?;

    let mut all_tags: BTreeSet<String> = BTreeSet::new();
    for task in &tasks {
        for t in &task.tags {
            all_tags.insert(t.clone());
        }
    }
    for t in descriptions.keys() {
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

    print_group("Contexts (@)", &contexts, &descriptions);
    print_group("Resources (#)", &resources, &descriptions);
    print_group("Freeform", &freeform, &descriptions);

    Ok(())
}

fn print_group(
    header: &str,
    tags: &[&str],
    descriptions: &std::collections::HashMap<String, String>,
) {
    if tags.is_empty() {
        return;
    }
    println!("{header}");
    for &t in tags {
        match descriptions.get(t) {
            Some(desc) => println!("  {t:<28}  {desc}"),
            None => println!("  {t}"),
        }
    }
    println!();
}

fn describe(ctx: &mut AppContext, args: DescribeArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.store.set_tag_description(&args.tag, &args.description)?;
    let tag_path = next_storage::tag_description_path(&ctx.repo_root, &args.tag);
    ctx.vcs
        .commit(&[tag_path], &format!("next: tag describe {}", args.tag))?;
    ctx.log
        .info("tag", &format!("described {} = {}", args.tag, args.description));
    Ok(())
}

fn clear_description(ctx: &mut AppContext, args: ClearDescriptionArgs) -> anyhow::Result<()> {
    ctx.store.delete_tag_description(&args.tag)?;
    let tag_path = next_storage::tag_description_path(&ctx.repo_root, &args.tag);
    ctx.vcs.commit(
        &[tag_path],
        &format!("next: tag clear-description {}", args.tag),
    )?;
    ctx.log
        .info("tag", &format!("cleared description for {}", args.tag));
    Ok(())
}
