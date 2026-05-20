use clap::Parser;
use next_cli::{
    AppContext,
    cli::{Cli, Command, commands},
};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut ctx = AppContext::new()?;

    let cmd_name = match &cli.command {
        Command::Add(_) => "add",
        Command::List(_) => "list",
        Command::Next(_) => "next",
        Command::Show(_) => "show",
        Command::Done(_) => "done",
        Command::Cancel(_) => "cancel",
        Command::Edit(_) => "edit",
        Command::Delete(_) => "delete",
        Command::Move(_) => "move",
        Command::Project(_) => "project",
        Command::Context(_) => "context",
        Command::Resource(_) => "resource",
        Command::User(_) => "user",
        Command::Forecast(_) => "forecast",
        Command::Review(_) => "review",
        Command::Sync(_) => "sync",
        Command::Import(_) => "import",
        Command::Export(_) => "export",
    };

    let result = match cli.command {
        Command::Add(args) => commands::add::run(args, &mut ctx),
        Command::List(args) => commands::list::run(args, &mut ctx),
        Command::Next(args) => commands::next_cmd::run(args, &mut ctx),
        Command::Show(args) => commands::show::run(args, &mut ctx),
        Command::Done(args) => commands::done::run(args, &mut ctx),
        Command::Cancel(args) => commands::cancel::run(args, &mut ctx),
        Command::Edit(args) => commands::edit::run(args, &mut ctx),
        Command::Delete(args) => commands::delete::run(args, &mut ctx),
        Command::Move(args) => commands::move_cmd::run(args, &mut ctx),
        Command::Project(args) => commands::project::run(args, &mut ctx),
        Command::Context(args) => commands::context::run(args, &mut ctx),
        Command::Resource(args) => commands::resource::run(args, &mut ctx),
        Command::User(args) => commands::user::run(args, &mut ctx),
        Command::Forecast(args) => commands::forecast::run(args, &mut ctx),
        Command::Review(args) => commands::review::run(args, &mut ctx),
        Command::Sync(args) => commands::sync::run(args, &mut ctx),
        Command::Import(args) => commands::import::run(args, &mut ctx),
        Command::Export(args) => commands::export::run(args, &mut ctx),
    };

    if let Err(ref e) = result {
        ctx.log.error(cmd_name, &format!("{e:#}"));
    }

    result
}
