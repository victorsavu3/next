mod cli;

use clap::Parser;
use cli::{Cli, Command};

/// Minimal application context used until the real storage layer is available.
pub struct AppContext {
    pub config: next::Config,
}

impl AppContext {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            config: next::Config::default(),
        })
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut ctx = AppContext::new()?;

    match cli.command {
        Command::Add(args) => cli::commands::add::run(args, &mut ctx),
        Command::List(args) => cli::commands::list::run(args, &mut ctx),
        Command::Next(args) => cli::commands::next_cmd::run(args, &mut ctx),
        Command::Show(args) => cli::commands::show::run(args, &mut ctx),
        Command::Done(args) => cli::commands::done::run(args, &mut ctx),
        Command::Cancel(args) => cli::commands::cancel::run(args, &mut ctx),
        Command::Edit(args) => cli::commands::edit::run(args, &mut ctx),
        Command::Delete(args) => cli::commands::delete::run(args, &mut ctx),
        Command::Move(args) => cli::commands::move_cmd::run(args, &mut ctx),
        Command::Project(args) => cli::commands::project::run(args, &mut ctx),
        Command::Context(args) => cli::commands::context::run(args, &mut ctx),
        Command::Resource(args) => cli::commands::resource::run(args, &mut ctx),
        Command::Forecast(args) => cli::commands::forecast::run(args, &mut ctx),
        Command::Review(args) => cli::commands::review::run(args, &mut ctx),
        Command::Sync(args) => cli::commands::sync::run(args, &mut ctx),
        Command::Import(args) => cli::commands::import::run(args, &mut ctx),
        Command::Export(args) => cli::commands::export::run(args, &mut ctx),
    }
}
