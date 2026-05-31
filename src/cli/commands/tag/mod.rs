mod data;
mod meta;

use std::collections::BTreeSet;

use crate::domain::tag::{self, TagKind, TagMeta};
use crate::AppContext;

pub use data::{DataArgs, DataGetArgs, DataListArgs, DataSetArgs, DataSubcommand, DataUnsetArgs};
pub use meta::{
    ClearDescriptionArgs, ClearPriorityArgs, ClearUrlArgs, DescribeArgs, NoTimeUrgencyArgs,
    SetPriorityArgs, SetUrlArgs, ShowArgs,
};

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

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => list(ctx),
        Some(TagSubcommand::Show(a)) => meta::show(ctx, a),
        Some(TagSubcommand::Describe(a)) => meta::describe(ctx, a),
        Some(TagSubcommand::ClearDescription(a)) => meta::clear_description(ctx, a),
        Some(TagSubcommand::SetUrl(a)) => meta::set_url(ctx, a),
        Some(TagSubcommand::ClearUrl(a)) => meta::clear_url(ctx, a),
        Some(TagSubcommand::SetPriority(a)) => meta::set_priority(ctx, a),
        Some(TagSubcommand::ClearPriority(a)) => meta::clear_priority(ctx, a),
        Some(TagSubcommand::SetNoTimeUrgency(a)) => meta::set_no_time_urgency(ctx, a),
        Some(TagSubcommand::ClearNoTimeUrgency(a)) => meta::clear_no_time_urgency(ctx, a),
        Some(TagSubcommand::Data(a)) => data::run(ctx, a),
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
