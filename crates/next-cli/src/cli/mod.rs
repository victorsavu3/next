pub mod commands;
pub mod filter;
pub mod render;

use clap::{Parser, Subcommand};

/// next — a GTD task manager.
#[derive(Parser, Debug)]
#[command(name = "next", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Add a new task.
    Add(commands::add::Args),

    /// List tasks.
    List(commands::list::Args),

    /// Show the most actionable tasks.
    #[command(name = "next")]
    Next(commands::next_cmd::Args),

    /// Show details for a task.
    Show(commands::show::Args),

    /// Mark a task as done.
    Done(commands::done::Args),

    /// Cancel a task.
    Cancel(commands::cancel::Args),

    /// Edit a task.
    Edit(commands::edit::Args),

    /// Delete a task.
    Delete(commands::delete::Args),

    /// Move a task to a different project or stage.
    #[command(name = "move")]
    Move(commands::move_cmd::Args),

    /// Manage projects.
    Project(commands::project::Args),

    /// Manage active contexts.
    Context(commands::context::Args),

    /// Manage resource availability.
    Resource(commands::resource::Args),

    /// Manage the active user filter.
    User(commands::user::Args),

    /// Show a due-date forecast.
    Forecast(commands::forecast::Args),

    /// Run a GTD weekly review.
    Review(commands::review::Args),

    /// Sync with the remote VCS backend.
    Sync(commands::sync::Args),

    /// Import tasks from an external source.
    Import(commands::import::Args),

    /// Export tasks to an external format.
    Export(commands::export::Args),
}
