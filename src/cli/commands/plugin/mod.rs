//! `next plugin` — manage machine-local export plugins.
//!
//! Plugins subscribe to individual tasks and are spawned when a watched task is
//! updated.  Registrations live in the `[[plugin]]` section of the machine-local
//! `state.toml` (not synced via git); see [`crate::core::plugin`].

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
    /// Set a plugin's periodic-sync (import) command (upserts the plugin).
    #[command(name = "set-sync")]
    SetSync(SetSyncArgs),
    /// Set the user override for a plugin's sync interval (seconds).
    #[command(name = "set-interval")]
    SetInterval(SetIntervalArgs),
    /// Enable a plugin's periodic sync.
    Enable(NameArgs),
    /// Disable a plugin's periodic sync.
    Disable(NameArgs),
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

#[derive(clap::Args, Debug)]
pub struct SetSyncArgs {
    /// Plugin name (created if it does not exist).
    pub name: String,
    /// The plugin's advertised default sync interval, in seconds (PLUGIN
    /// priority, below a user `set-interval` override).
    #[arg(long)]
    pub default_interval: Option<u64>,
    /// Sync command to spawn (program followed by its arguments). Captured
    /// verbatim; run with `NEXT_REPO` set when the plugin is due.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true, num_args = 1..)]
    pub command: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub struct SetIntervalArgs {
    /// Plugin name (must already be registered).
    pub name: String,
    /// Interval in seconds (USER override). Omit with `--clear` to remove it.
    #[arg(required_unless_present = "clear")]
    pub secs: Option<u64>,
    /// Clear the user override, falling back to the plugin/system default.
    #[arg(long)]
    pub clear: bool,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    let root = ctx.repo.repo_root.clone();
    match args.subcommand {
        PluginSubcommand::Register(a) => {
            registry::register(&root, &a.name, a.command.clone())?;
            tracing::info!(cmd = "plugin", "registered {} -> {}", a.name, a.command.join(" "));
        }
        PluginSubcommand::Watch(a) => {
            let id = resolve_task_id(&*ctx.repo.store, &a.task)?;
            registry::watch(&root, &a.name, id)?;
            tracing::info!(cmd = "plugin", "{} now watches {}", a.name, id);
        }
        PluginSubcommand::Unwatch(a) => {
            let id = resolve_task_id(&*ctx.repo.store, &a.task)?;
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
        PluginSubcommand::SetSync(a) => {
            registry::set_sync_command(&root, &a.name, a.command.clone())?;
            if let Some(secs) = a.default_interval {
                registry::set_default_sync_interval(&root, &a.name, Some(secs))?;
            }
            tracing::info!(cmd = "plugin", "{} sync -> {}", a.name, a.command.join(" "));
        }
        PluginSubcommand::SetInterval(a) => {
            let secs = if a.clear { None } else { a.secs };
            registry::set_sync_interval(&root, &a.name, secs)?;
            match secs {
                Some(s) => tracing::info!(cmd = "plugin", "{} sync interval -> {}s", a.name, s),
                None => tracing::info!(cmd = "plugin", "{} sync interval cleared", a.name),
            }
        }
        PluginSubcommand::Enable(a) => {
            registry::set_enabled(&root, &a.name, true)?;
            tracing::info!(cmd = "plugin", "{} enabled", a.name);
        }
        PluginSubcommand::Disable(a) => {
            registry::set_enabled(&root, &a.name, false)?;
            tracing::info!(cmd = "plugin", "{} disabled", a.name);
        }
        PluginSubcommand::List => list(ctx)?,
    }
    Ok(())
}

fn list(ctx: &AppContext) -> anyhow::Result<()> {
    let root = &ctx.repo.repo_root;
    let reg = registry::load(root)?;
    if reg.plugins.is_empty() {
        println!("No plugins registered.");
        return Ok(());
    }
    let sync = crate::core::sync_state::load(root)?;
    let system_default = ctx.config.sync.plugin_sync_default_secs;
    for plugin in &reg.plugins {
        let status = if plugin.enabled { "" } else { "  [disabled]" };
        if plugin.command.is_empty() {
            println!("{}{status}", plugin.name);
        } else {
            println!("{}  ({}){status}", plugin.name, plugin.command.join(" "));
        }
        // Periodic-sync configuration, when set.
        if !plugin.sync_command.is_empty() {
            let interval = crate::core::plugin::resolve_sync_interval(plugin, system_default);
            let last = sync
                .plugins
                .get(&plugin.name)
                .and_then(|p| p.last_sync)
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "never".to_owned());
            println!(
                "  sync: {}  (every {}s, last {last})",
                plugin.sync_command.join(" "),
                interval.as_secs()
            );
        }
        if plugin.tasks.is_empty() {
            println!("  (no tasks watched)");
        } else {
            for id in &plugin.tasks {
                // Show the title when the task still exists; bare id otherwise.
                match ctx.repo.store.get_task(*id) {
                    Ok(task) => println!("  {}  {}", &id.to_string()[..8], task.title),
                    Err(_) => println!("  {}  (unknown task)", &id.to_string()[..8]),
                }
            }
        }
    }
    Ok(())
}
