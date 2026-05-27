use anyhow::Context as _;
use clap::Parser;
use next::{
    AppContext,
    cli::{Cli, Command, commands},
};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Init runs before the repository exists — handle it before AppContext.
    if let Some(Command::Init(args)) = cli.command {
        let dir = if let Some(ref p) = cli.repo {
            p.clone()
        } else {
            std::env::current_dir().context("cannot determine current directory")?
        };
        return commands::init::run(args, &dir);
    }

    let mut ctx = AppContext::new(cli.config.as_deref(), cli.repo.as_deref())?;

    // Default to `list` when no subcommand is given.
    let command = cli.command.unwrap_or_else(|| {
        Command::List(commands::list::Args {
            future: false,
            all: false,
            all_users: false,
            json: false,
            limit: None,
            tokens: vec![],
        })
    });

    let cmd_name = match &command {
        Command::Init(_) => unreachable!("handled above"),
        Command::Add(_) => "add",
        Command::List(_) => "list",
        Command::Next(_) => "next",
        Command::Show(_) => "show",
        Command::Open(_) => "open",
        Command::Data(_) => "data",
        Command::Done(_) => "done",
        Command::Cancel(_) => "cancel",
        Command::Edit(_) => "edit",
        Command::Delete(_) => "delete",
        Command::Move(_) => "move",
        Command::Context(_) => "context",
        Command::Resource(_) => "resource",
        Command::User(_) => "user",
        Command::Forecast(_) => "forecast",
        Command::Sync(_) => "sync",
        Command::Import(_) => "import",
        Command::Export(_) => "export",
        Command::Tag(_) => "tag",
        Command::Tree(_) => "tree",
    };

    let result = match command {
        Command::Init(_) => unreachable!("handled above"),
        Command::Add(args) => commands::add::run(args, &mut ctx),
        Command::List(args) => commands::list::run(args, &mut ctx),
        Command::Next(args) => commands::next_cmd::run(args, &mut ctx),
        Command::Show(args) => commands::show::run(args, &mut ctx),
        Command::Open(args) => commands::open::run(args, &mut ctx),
        Command::Data(args) => commands::data::run(args, &mut ctx),
        Command::Done(args) => commands::done::run(args, &mut ctx),
        Command::Cancel(args) => commands::cancel::run(args, &mut ctx),
        Command::Edit(args) => commands::edit::run(args, &mut ctx),
        Command::Delete(args) => commands::delete::run(args, &mut ctx),
        Command::Move(args) => commands::move_cmd::run(args, &mut ctx),
        Command::Context(args) => commands::context::run(args, &mut ctx),
        Command::Resource(args) => commands::resource::run(args, &mut ctx),
        Command::User(args) => commands::user::run(args, &mut ctx),
        Command::Forecast(args) => commands::forecast::run(args, &mut ctx),
        Command::Sync(args) => commands::sync::run(args, &mut ctx),
        Command::Import(args) => commands::import::run(args, &mut ctx),
        Command::Export(args) => commands::export::run(args, &mut ctx),
        Command::Tag(args) => commands::tag::run(args, &mut ctx),
        Command::Tree(args) => commands::tree::run(args, &mut ctx),
    };

    if let Err(ref e) = result {
        ctx.log.error(cmd_name, &format!("{e:#}"));
    }

    result
}
