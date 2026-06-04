//! `next plugin` — manage machine-local export plugins.
//!
//! Plugins subscribe to individual tasks and are spawned when a watched task is
//! updated.  Registrations live in a machine-local `plugins.toml` (not synced
//! via git); see [`crate::core::plugin`].

use crate::{core::{plugin::registry, resolve::resolve_task_id}, AppContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: PluginSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum PluginSubcommand {
    /// Register a plugin, or replace an existing one's command (subscriptions kept).
    Register(RegisterArgs),
    /// Subscribe a plugin to a task; it is notified on any update to that task.
    Watch(WatchArgs),
    /// Unsubscribe a plugin from a task.
    Unwatch(WatchArgs),
    /// Remove a plugin and all its subscriptions.
    Unregister(NameArgs),
    /// List registered plugins and the tasks they watch.
    List,
}

#[derive(clap::Args, Debug)]
pub struct RegisterArgs {
    /// Unique plugin name (also used as the loop-guard origin token).
    pub name: String,
    /// Command to spawn (program followed by its arguments). Captured verbatim;
    /// the event is delivered on stdin and in the `NEXT_PLUGIN_EVENT` env var.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true, num_args = 1..)]
    pub command: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct WatchArgs {
    /// Plugin name (must already be registered).
    pub name: String,
    /// Task to (un)watch: UUID, UUID prefix, or slug.
    pub task: String,
}

#[derive(clap::Args, Debug)]
pub struct NameArgs {
    /// Plugin name.
    pub name: String,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let root = ctx.repo_root.clone();
    match args.subcommand {
        PluginSubcommand::Register(a) => {
            registry::register(&root, &a.name, a.command.clone())?;
            tracing::info!(cmd = "plugin", "registered {} -> {}", a.name, a.command.join(" "));
        }
        PluginSubcommand::Watch(a) => {
            let id = resolve_task_id(&*ctx.store, &a.task)?;
            registry::watch(&root, &a.name, id)?;
            tracing::info!(cmd = "plugin", "{} now watches {}", a.name, id);
        }
        PluginSubcommand::Unwatch(a) => {
            let id = resolve_task_id(&*ctx.store, &a.task)?;
            registry::unwatch(&root, &a.name, id)?;
            tracing::info!(cmd = "plugin", "{} no longer watches {}", a.name, id);
        }
        PluginSubcommand::Unregister(a) => {
            if registry::unregister(&root, &a.name)? {
                tracing::info!(cmd = "plugin", "unregistered {}", a.name);
            } else {
                anyhow::bail!("no plugin named {:?}", a.name);
            }
        }
        PluginSubcommand::List => list(ctx)?,
    }
    Ok(())
}

fn list(ctx: &AppContext) -> anyhow::Result<()> {
    let reg = registry::load(&ctx.repo_root)?;
    if reg.plugins.is_empty() {
        println!("No plugins registered.");
        return Ok(());
    }
    for plugin in &reg.plugins {
        println!("{}  ({})", plugin.name, plugin.command.join(" "));
        if plugin.tasks.is_empty() {
            println!("  (no tasks watched)");
        } else {
            for id in &plugin.tasks {
                // Show the title when the task still exists; bare id otherwise.
                match ctx.store.get_task(*id) {
                    Ok(task) => println!("  {}  {}", &id.to_string()[..8], task.title),
                    Err(_) => println!("  {}  (unknown task)", &id.to_string()[..8]),
                }
            }
        }
    }
    Ok(())
}
