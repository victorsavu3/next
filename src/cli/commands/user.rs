use crate::AppContext;

/// Top-level `next user` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: Option<UserSubcommand>,
}

#[derive(clap::Subcommand, Debug)]
pub enum UserSubcommand {
    /// Set the active user filter (replaces any previously active users).
    Set(SetArgs),
    /// Clear all active users (return to showing all tasks).
    Clear,
    /// List all users referenced across all tasks.
    List,
}

#[derive(clap::Args, Debug)]
pub struct SetArgs {
    /// One or more usernames to activate as the current user filter.
    #[arg(num_args(1..))]
    pub users: Vec<String>,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => show(ctx),
        Some(UserSubcommand::Set(a)) => set(ctx, a.users),
        Some(UserSubcommand::Clear) => clear(ctx),
        Some(UserSubcommand::List) => list(ctx),
    }
}

fn show(ctx: &mut AppContext) -> anyhow::Result<()> {
    let state = ctx.store.get_state()?;
    if state.active_users.is_empty() {
        println!("No active user filter (all tasks visible).");
    } else {
        println!("Active users:");
        for u in &state.active_users {
            println!("  {u}");
        }
    }
    Ok(())
}

fn set(ctx: &mut AppContext, users: Vec<String>) -> anyhow::Result<()> {
    ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        state.active_users = users.clone();
        store.save_state(&state)?;
        Ok(())
    })?;
    tracing::info!(cmd = "user", "set {}", users.join(" "));
    Ok(())
}

fn clear(ctx: &mut AppContext) -> anyhow::Result<()> {
    ctx.state_transaction(|store| {
        let mut state = store.get_state()?;
        state.active_users.clear();
        store.save_state(&state)?;
        Ok(())
    })?;
    tracing::info!(cmd = "user", "cleared");
    Ok(())
}

fn list(ctx: &mut AppContext) -> anyhow::Result<()> {
    let tasks = ctx.store.list_tasks()?;
    let mut users: Vec<String> = tasks
        .into_iter()
        .filter_map(|t| t.assignee)
        .collect();
    users.sort();
    users.dedup();

    if users.is_empty() {
        println!("No users assigned to any task.");
    } else {
        let state = ctx.store.get_state()?;
        for u in &users {
            let active = if state.active_users.contains(u) {
                " *"
            } else {
                ""
            };
            println!("  {u}{active}");
        }
    }
    Ok(())
}
