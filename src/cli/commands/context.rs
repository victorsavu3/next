use crate::core::domain::tag;
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
    Set(ContextTagArgs),
    /// Clear all active context tags (return to context-agnostic mode).
    Clear,
    /// Set excluded context tags — tasks with these contexts are always hidden.
    Exclude(ContextTagArgs),
    /// Clear all excluded context tags.
    #[command(name = "clear-excluded")]
    ClearExcluded,
}

#[derive(clap::Args, Debug)]
pub struct ContextTagArgs {
    /// One or more @-prefixed context tags (e.g. @work @home/kitchen).
    #[arg(num_args(1..))]
    pub tags: Vec<String>,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => show(ctx),
        Some(ContextSubcommand::Set(a)) => set(ctx, a.tags),
        Some(ContextSubcommand::Clear) => clear(ctx),
        Some(ContextSubcommand::Exclude(a)) => exclude(ctx, a.tags),
        Some(ContextSubcommand::ClearExcluded) => clear_excluded(ctx),
    }
}

fn show(ctx: &mut AppContext) -> anyhow::Result<()> {
    let state = ctx.store.get_state()?;
    if state.active_contexts.is_empty() {
        println!("Active context: (none — all tasks visible)");
    } else {
        println!("Active contexts:");
        for c in &state.active_contexts {
            match ctx.store.get_tag_description(c)? {
                Some(desc) => println!("  {c:<28}  {desc}"),
                None => println!("  {c}"),
            }
        }
    }
    if !state.excluded_contexts.is_empty() {
        println!("Excluded contexts:");
        for c in &state.excluded_contexts {
            match ctx.store.get_tag_description(c)? {
                Some(desc) => println!("  {c:<28}  {desc}"),
                None => println!("  {c}"),
            }
        }
    }
    Ok(())
}

fn validate_context_tags(tags: &[String]) -> anyhow::Result<()> {
    for t in tags {
        tag::validate_context_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    Ok(())
}

fn set(ctx: &mut AppContext, tags: Vec<String>) -> anyhow::Result<()> {
    validate_context_tags(&tags)?;
    ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        state.active_contexts = tags.clone();
        store.save_state(&state)?;
        Ok(())
    })?;
    tracing::info!(cmd = "context", "set {}", tags.join(" "));
    Ok(())
}

fn clear(ctx: &mut AppContext) -> anyhow::Result<()> {
    ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        state.active_contexts.clear();
        store.save_state(&state)?;
        Ok(())
    })?;
    tracing::info!(cmd = "context", "cleared");
    Ok(())
}

fn exclude(ctx: &mut AppContext, tags: Vec<String>) -> anyhow::Result<()> {
    validate_context_tags(&tags)?;
    ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        state.excluded_contexts = tags.clone();
        store.save_state(&state)?;
        Ok(())
    })?;
    tracing::info!(cmd = "context", "exclude {}", tags.join(" "));
    Ok(())
}

fn clear_excluded(ctx: &mut AppContext) -> anyhow::Result<()> {
    ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        state.excluded_contexts.clear();
        store.save_state(&state)?;
        Ok(())
    })?;
    tracing::info!(cmd = "context", "cleared excluded");
    Ok(())
}
