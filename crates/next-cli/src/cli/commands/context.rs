use crate::AppContext;

/// Top-level `next context` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: Option<ContextSubcommand>,
}

#[derive(clap::Subcommand, Debug)]
pub enum ContextSubcommand {
    /// Set the active context tags (replaces any previously active contexts).
    Set(SetArgs),
    /// Clear all active context tags (return to context-agnostic mode).
    Clear,
    /// Set a human-readable description for a context tag.
    Describe(DescribeArgs),
    /// Remove the description for a context tag.
    #[command(name = "clear-description")]
    ClearDescription(ClearDescriptionArgs),
}

#[derive(clap::Args, Debug)]
pub struct SetArgs {
    /// One or more @-prefixed context tags to activate (e.g. @work @work/frontend).
    #[arg(num_args(1..))]
    pub tags: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct DescribeArgs {
    /// Context tag to describe (must start with @, e.g. @work).
    pub tag: String,
    /// Description text.
    pub description: String,
}

#[derive(clap::Args, Debug)]
pub struct ClearDescriptionArgs {
    /// Context tag whose description should be removed.
    pub tag: String,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => show(ctx),
        Some(ContextSubcommand::Set(a)) => set(ctx, a.tags),
        Some(ContextSubcommand::Clear) => clear(ctx),
        Some(ContextSubcommand::Describe(a)) => describe(ctx, a),
        Some(ContextSubcommand::ClearDescription(a)) => clear_description(ctx, a),
    }
}

fn show(ctx: &mut AppContext) -> anyhow::Result<()> {
    let state = ctx.store.get_state()?;
    if state.active_contexts.is_empty() {
        println!("No active context (all tasks visible).");
    } else {
        println!("Active contexts:");
        for c in &state.active_contexts {
            match ctx.store.get_tag_description(c)? {
                Some(desc) => println!("  {c:<28}  {desc}"),
                None => println!("  {c}"),
            }
        }
    }
    Ok(())
}

fn set(ctx: &mut AppContext, tags: Vec<String>) -> anyhow::Result<()> {
    for tag in &tags {
        if !tag.starts_with('@') {
            anyhow::bail!("context tags must start with '@', got: {tag}");
        }
    }
    let mut state = ctx.store.get_state()?;
    state.active_contexts = tags.clone();
    ctx.store.save_state(&state)?;
    let state_path = ctx.repo_root.join("state.toml");
    ctx.vcs.commit(
        &[state_path],
        &format!("next: context set {}", tags.join(" ")),
    )?;
    ctx.log.info("context", &format!("set {}", tags.join(" ")));
    Ok(())
}

fn clear(ctx: &mut AppContext) -> anyhow::Result<()> {
    let mut state = ctx.store.get_state()?;
    state.active_contexts.clear();
    ctx.store.save_state(&state)?;
    let state_path = ctx.repo_root.join("state.toml");
    ctx.vcs.commit(&[state_path], "next: context clear")?;
    ctx.log.info("context", "cleared");
    Ok(())
}

fn describe(ctx: &mut AppContext, args: DescribeArgs) -> anyhow::Result<()> {
    if !args.tag.starts_with('@') {
        anyhow::bail!("context tags must start with '@', got: {}", args.tag);
    }
    ctx.store.set_tag_description(&args.tag, &args.description)?;
    let tag_path = next_storage::tag_description_path(&ctx.repo_root, &args.tag);
    ctx.vcs.commit(
        &[tag_path],
        &format!("next: context describe {}", args.tag),
    )?;
    ctx.log
        .info("context", &format!("described {} = {}", args.tag, args.description));
    Ok(())
}

fn clear_description(ctx: &mut AppContext, args: ClearDescriptionArgs) -> anyhow::Result<()> {
    if !args.tag.starts_with('@') {
        anyhow::bail!("context tags must start with '@', got: {}", args.tag);
    }
    ctx.store.delete_tag_description(&args.tag)?;
    let tag_path = next_storage::tag_description_path(&ctx.repo_root, &args.tag);
    ctx.vcs.commit(
        &[tag_path],
        &format!("next: context clear-description {}", args.tag),
    )?;
    ctx.log
        .info("context", &format!("cleared description for {}", args.tag));
    Ok(())
}
