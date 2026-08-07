//! `next tag require|exclude|accept|clear-state` — the machine-local tag
//! state, which replaced the old `next context` and `next resource` commands.
//!
//! Every tag takes the same three states, so there is one set of commands
//! rather than one per sigil. `@` and `#` still say what a tag is *for*, and
//! the listing groups by them, but they no longer change what the state does.

use crate::core::domain::state::TagState;
use crate::core::domain::tag;
use crate::AppContext;

#[derive(clap::Args, Debug)]
pub struct TagStateArgs {
    /// One or more tags (e.g. @work #printer errand).
    #[arg(num_args(1..))]
    pub tags: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct ClearStateArgs {
    /// Tags to reset. With none given, every tag state is cleared.
    pub tags: Vec<String>,
}

/// Sets `state` on each tag.
pub fn set(ctx: &mut AppContext, args: TagStateArgs, state: TagState) -> anyhow::Result<()> {
    for t in &args.tags {
        tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let tags = args.tags.clone();
    ctx.repo.state_transaction(|store| {
        let mut global = store.get_state()?;
        for t in &tags {
            global.set_state(t, Some(state));
        }
        store.save_state(&global)?;
        Ok(())
    })?;

    let verb = match state {
        TagState::Required => "Required",
        TagState::Excluded => "Excluded",
        TagState::Accepted => "Accepted",
    };
    println!("{verb}: {}", args.tags.join(" "));
    if state == TagState::Accepted {
        println!("  (these tags are neither required nor hidden, and now ignore any state inherited from a parent tag)");
    }
    tracing::info!(cmd = "tag", "{verb} {}", args.tags.join(" "));
    Ok(())
}

/// Removes the entry for each tag, so it inherits from its parent again.
pub fn clear(ctx: &mut AppContext, args: ClearStateArgs) -> anyhow::Result<()> {
    for t in &args.tags {
        tag::validate_tag(t).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    let tags = args.tags.clone();
    ctx.repo.state_transaction(|store| {
        let mut global = store.get_state()?;
        if tags.is_empty() {
            global.tags.clear();
        } else {
            for t in &tags {
                global.set_state(t, None);
            }
        }
        store.save_state(&global)?;
        Ok(())
    })?;

    if args.tags.is_empty() {
        println!("All tag state cleared.");
    } else {
        println!("Cleared state for: {}", args.tags.join(" "));
    }
    tracing::info!(cmd = "tag", "cleared state");
    Ok(())
}

/// Prints the current state, for the top of `next tag`.
pub fn show(ctx: &AppContext) -> anyhow::Result<()> {
    let state = ctx.repo.store.get_state()?;
    if state.tags.is_empty() {
        println!("Tag state: none — every task is visible.");
        return Ok(());
    }

    println!("Tag state:");
    for (label, kind) in [
        ("required", TagState::Required),
        ("excluded", TagState::Excluded),
        ("accepted", TagState::Accepted),
    ] {
        let tags = state.tags_with(kind);
        if !tags.is_empty() {
            println!("  {label:<9} {}", tags.join(" "));
        }
    }
    if !state.any_required() {
        println!("  (nothing is required, so every tag that is not excluded is shown)");
    }
    Ok(())
}
