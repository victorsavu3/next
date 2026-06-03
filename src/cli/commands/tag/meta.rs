use crate::domain::tag::{self, TagMeta};
use crate::domain::task::Priority;
use crate::AppContext;

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

pub fn show(ctx: &mut AppContext, args: ShowArgs) -> anyhow::Result<()> {
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

pub fn describe(ctx: &mut AppContext, args: DescribeArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        store.set_tag_description(&args.tag, &args.description)?;
        let tag_path = crate::storage::tag_meta_path(root, &args.tag);
        vcs.commit(&[tag_path], &format!("next: tag describe {}", args.tag))?;
        Ok(())
    })?;
    tracing::info!(cmd = "tag", "described {} = {}", args.tag, args.description);
    Ok(())
}

pub fn clear_description(ctx: &mut AppContext, args: ClearDescriptionArgs) -> anyhow::Result<()> {
    ctx.transaction(|store, vcs, root| {
        store.delete_tag_description(&args.tag)?;
        let tag_path = crate::storage::tag_meta_path(root, &args.tag);
        vcs.commit(
            &[tag_path],
            &format!("next: tag clear-description {}", args.tag),
        )?;
        Ok(())
    })?;
    tracing::info!(cmd = "tag", "cleared description for {}", args.tag);
    Ok(())
}

pub fn set_url(ctx: &mut AppContext, args: SetUrlArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store.get_tag_meta(&args.tag)?.unwrap_or_default();
        meta.url = Some(args.url.clone());
        store.set_tag_meta(&args.tag, meta)?;
        let tag_path = crate::storage::tag_meta_path(root, &args.tag);
        vcs.commit(&[tag_path], &format!("next: tag set-url {}", args.tag))?;
        Ok(())
    })?;
    tracing::info!(cmd = "tag", "set url for {} = {}", args.tag, args.url);
    Ok(())
}

pub fn clear_url(ctx: &mut AppContext, args: ClearUrlArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store
            .get_tag_meta(&args.tag)?
            .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {:?}", args.tag))?;
        meta.url = None;
        let tag_path = crate::storage::tag_meta_path(root, &args.tag);
        if meta == TagMeta::default() {
            store.delete_tag_meta(&args.tag)?;
        } else {
            store.set_tag_meta(&args.tag, meta)?;
        }
        vcs.commit(&[tag_path], &format!("next: tag clear-url {}", args.tag))?;
        Ok(())
    })?;
    tracing::info!(cmd = "tag", "cleared url for {}", args.tag);
    Ok(())
}

pub fn set_priority(ctx: &mut AppContext, args: SetPriorityArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    let priority: Priority = args.priority.parse()?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store.get_tag_meta(&args.tag)?.unwrap_or_default();
        meta.priority = Some(priority);
        store.set_tag_meta(&args.tag, meta)?;
        let tag_path = crate::storage::tag_meta_path(root, &args.tag);
        vcs.commit(&[tag_path], &format!("next: tag set-priority {}", args.tag))?;
        Ok(())
    })?;
    tracing::info!(cmd = "tag", "set priority for {} = {}", args.tag, args.priority);
    Ok(())
}

pub fn clear_priority(ctx: &mut AppContext, args: ClearPriorityArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store
            .get_tag_meta(&args.tag)?
            .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {:?}", args.tag))?;
        meta.priority = None;
        let tag_path = crate::storage::tag_meta_path(root, &args.tag);
        if meta == TagMeta::default() {
            store.delete_tag_meta(&args.tag)?;
        } else {
            store.set_tag_meta(&args.tag, meta)?;
        }
        vcs.commit(&[tag_path], &format!("next: tag clear-priority {}", args.tag))?;
        Ok(())
    })?;
    tracing::info!(cmd = "tag", "cleared priority for {}", args.tag);
    Ok(())
}

pub fn set_no_time_urgency(ctx: &mut AppContext, args: NoTimeUrgencyArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store.get_tag_meta(&args.tag)?.unwrap_or_default();
        meta.no_time_urgency = true;
        store.set_tag_meta(&args.tag, meta)?;
        let tag_path = crate::storage::tag_meta_path(root, &args.tag);
        vcs.commit(&[tag_path], &format!("next: tag set-no-time-urgency {}", args.tag))?;
        Ok(())
    })?;
    tracing::info!(cmd = "tag", "set no-time-urgency for {}", args.tag);
    Ok(())
}

pub fn clear_no_time_urgency(ctx: &mut AppContext, args: NoTimeUrgencyArgs) -> anyhow::Result<()> {
    tag::validate_tag(&args.tag).map_err(|e| anyhow::anyhow!("{e}"))?;
    ctx.transaction(|store, vcs, root| {
        let mut meta = store
            .get_tag_meta(&args.tag)?
            .ok_or_else(|| anyhow::anyhow!("no metadata set for tag {:?}", args.tag))?;
        meta.no_time_urgency = false;
        let tag_path = crate::storage::tag_meta_path(root, &args.tag);
        if meta == TagMeta::default() {
            store.delete_tag_meta(&args.tag)?;
        } else {
            store.set_tag_meta(&args.tag, meta)?;
        }
        vcs.commit(&[tag_path], &format!("next: tag clear-no-time-urgency {}", args.tag))?;
        Ok(())
    })?;
    tracing::info!(cmd = "tag", "cleared no-time-urgency for {}", args.tag);
    Ok(())
}

fn priority_display(p: &Priority) -> &'static str {
    match p {
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
    }
}
