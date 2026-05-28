pub mod commands;
pub mod filter;
pub mod render;

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

    /// Sync with the remote after each mutation command.
    /// Overrides `autosync = false` in the config file.
    #[arg(long, global = true)]
    pub autosync: bool,

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

    /// Import tasks from an external source.
    Import(commands::import::Args),

    /// Export tasks to an external format.
    Export(commands::export::Args),

    /// Manage tag descriptions.
    Tag(commands::tag::Args),

    /// Show all tasks in a tree view.
    Tree(commands::tree::Args),
}
