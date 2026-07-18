pub mod app_context;
pub mod commands;
pub mod render;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn autosync_and_no_autosync_conflict() {
        let result = Cli::try_parse_from(["next", "--autosync", "--no-autosync", "list"]);
        assert!(result.is_err(), "--autosync and --no-autosync must conflict");
    }

    #[test]
    fn autosync_alone_is_accepted() {
        Cli::try_parse_from(["next", "--autosync", "list"]).unwrap();
    }

    #[test]
    fn no_autosync_alone_is_accepted() {
        Cli::try_parse_from(["next", "--no-autosync", "list"]).unwrap();
    }

    #[test]
    fn offline_alone_is_accepted() {
        Cli::try_parse_from(["next", "--offline", "list"]).unwrap();
    }

    #[test]
    fn granular_flags_alone_are_accepted() {
        Cli::try_parse_from(["next", "--autopull", "list"]).unwrap();
        Cli::try_parse_from(["next", "--no-autopull", "list"]).unwrap();
        Cli::try_parse_from(["next", "--autopush", "list"]).unwrap();
        Cli::try_parse_from(["next", "--no-autopush", "list"]).unwrap();
    }

    #[test]
    fn granular_flags_can_combine_across_capabilities() {
        // autopull and autopush are orthogonal, so mixing one of each is fine.
        Cli::try_parse_from(["next", "--autopull", "--no-autopush", "list"]).unwrap();
        Cli::try_parse_from(["next", "--no-autopull", "--autopush", "list"]).unwrap();
    }

    #[test]
    fn offline_and_autosync_conflict() {
        let result = Cli::try_parse_from(["next", "--offline", "--autosync", "list"]);
        assert!(result.is_err(), "--offline and --autosync must conflict");
    }

    #[test]
    fn no_autosync_and_offline_conflict() {
        let result = Cli::try_parse_from(["next", "--no-autosync", "--offline", "list"]);
        assert!(result.is_err(), "--no-autosync and --offline must conflict");
    }

    #[test]
    fn autopull_and_no_autopull_conflict() {
        let result = Cli::try_parse_from(["next", "--autopull", "--no-autopull", "list"]);
        assert!(result.is_err(), "--autopull and --no-autopull must conflict");
    }

    #[test]
    fn autopush_and_no_autopush_conflict() {
        let result = Cli::try_parse_from(["next", "--autopush", "--no-autopush", "list"]);
        assert!(result.is_err(), "--autopush and --no-autopush must conflict");
    }

    #[test]
    fn master_and_granular_flags_conflict() {
        assert!(
            Cli::try_parse_from(["next", "--autosync", "--autopull", "list"]).is_err(),
            "--autosync conflicts with granular flags"
        );
        assert!(
            Cli::try_parse_from(["next", "--no-autosync", "--autopush", "list"]).is_err(),
            "--no-autosync conflicts with granular flags"
        );
        assert!(
            Cli::try_parse_from(["next", "--offline", "--no-autopull", "list"]).is_err(),
            "--offline conflicts with granular flags"
        );
    }

    #[test]
    fn no_sync_flag_is_removed() {
        let result = Cli::try_parse_from(["next", "--no-sync", "list"]);
        assert!(result.is_err(), "--no-sync must no longer be recognised");
    }
}

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// next — a task manager with automatic urgency scoring.
#[derive(Parser, Debug)]
#[command(name = "next", version, about)]
pub struct Cli {
    /// Path to the configuration file (default: $XDG_CONFIG_HOME/task-manager/config.toml).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Path to the task repository root.  Overrides the `repository` key in
    /// the config file and the automatic upward search from the current
    /// directory.
    #[arg(long, global = true, value_name = "PATH")]
    pub repo: Option<PathBuf>,

    /// Master toggle: enable both autopull and autopush for this invocation.
    /// Mutually exclusive with the other sync flags.
    #[arg(
        long,
        global = true,
        conflicts_with_all = ["no_autosync", "offline", "autopull", "no_autopull", "autopush", "no_autopush"]
    )]
    pub autosync: bool,

    /// Master toggle: disable both autopull and autopush for this invocation.
    /// Mutually exclusive with the other sync flags.
    #[arg(
        long,
        global = true,
        conflicts_with_all = ["offline", "autopull", "no_autopull", "autopush", "no_autopush"]
    )]
    pub no_autosync: bool,

    /// Alias for `--no-autosync`: disable both autopull and autopush for this
    /// invocation (no network I/O). Mutually exclusive with the other sync flags.
    #[arg(
        long,
        global = true,
        conflicts_with_all = ["autopull", "no_autopull", "autopush", "no_autopush"]
    )]
    pub offline: bool,

    /// Pull from the remote before this command if the local copy is stale.
    /// Overrides `[sync] autopull` in config for this invocation.
    #[arg(long, global = true, conflicts_with = "no_autopull")]
    pub autopull: bool,

    /// Skip the pre-command staleness pull for this invocation.
    #[arg(long, global = true)]
    pub no_autopull: bool,

    /// Push to the remote after a successful mutation.
    /// Overrides `[sync] autopush` in config for this invocation.
    #[arg(long, global = true, conflicts_with = "no_autopush")]
    pub autopush: bool,

    /// Skip the post-mutation push for this invocation.
    #[arg(long, global = true)]
    pub no_autopush: bool,

    /// Subcommand to run. Defaults to `list` when omitted.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialise a new task repository in the current directory.
    Init(commands::init::Args),

    /// Add a new task.
    Add(commands::add::Args),

    /// List tasks.
    List(commands::list::Args),

    /// Show the most actionable tasks.
    #[command(name = "next")]
    Next(commands::next_cmd::Args),

    /// Show details for a task.
    Show(commands::show::Args),

    /// Open the URL attached to a task in the default browser.
    Open(commands::open::Args),

    /// Read or write individual keys in a task's data map.
    Data(commands::data::Args),

    /// Mark a task as started (in-progress).
    Start(commands::start::Args),

    /// Stop a started task (returns to open).
    Stop(commands::stop::Args),

    /// Mark a task as done.
    Done(commands::done::Args),

    /// Cancel a task.
    Cancel(commands::cancel::Args),

    /// Edit a task.
    Edit(commands::edit::Args),

    /// Delete a task.
    Delete(commands::delete::Args),

    /// Change the parent of a task.
    #[command(name = "move")]
    Move(commands::move_cmd::Args),

    /// Manage active contexts.
    Context(commands::context::Args),

    /// Manage resource availability.
    Resource(commands::resource::Args),

    /// Manage the active user filter.
    User(commands::user::Args),

    /// Show a due-date forecast.
    Forecast(commands::forecast::Args),

    /// Sync with the remote VCS backend.
    Sync(commands::sync::Args),

    /// Repository maintenance: rebuild the local cache, run the archive pass.
    Maintenance(commands::maintenance::Args),

    /// Manage tag descriptions.
    Tag(commands::tag::Args),

    /// Show all tasks in a tree view.
    Tree(commands::tree::Args),

    /// Manage export plugins (machine-local; not synced via git).
    Plugin(commands::plugin::Args),

    /// Show the interactive tutorial.
    Tutorial(commands::tutorial::Args),

    /// Read or write a config value.
    Config(commands::config::Args),
}
