use anyhow::Context as _;
use clap::Parser;
use next::{
    AppContext,
    cli::{Cli, Command, commands},
    cli::commands::sync as sync_cmd,
};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    // Init and Tutorial run before the repository exists — handle them before AppContext.
    if let Some(Command::Init(args)) = cli.command {
        let dir = if let Some(ref p) = cli.repo {
            p.clone()
        } else {
            std::env::current_dir().context("cannot determine current directory")?
        };
        return commands::init::run(args, &dir);
    }
    if let Some(Command::Tutorial(args)) = cli.command {
        return commands::tutorial::run(args);
    }

    let cli_autosync = cli.autosync;
    let cli_no_autosync = cli.no_autosync;
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
        Command::Start(_) => "start",
        Command::Stop(_) => "stop",
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
        Command::Tag(_) => "tag",
        Command::Tree(_) => "tree",
        Command::Plugin(_) => "plugin",
        Command::Tutorial(_) => unreachable!("handled above"),
    };

    let is_mutation = matches!(
        cmd_name,
        "add" | "start" | "stop" | "done" | "cancel" | "edit" | "delete" | "move" | "tag" | "data"
    );

    let result = match command {
        Command::Init(_) => unreachable!("handled above"),
        Command::Add(args) => commands::add::run(args, &mut ctx),
        Command::List(args) => commands::list::run(args, &ctx),
        Command::Next(args) => commands::next_cmd::run(args, &ctx),
        Command::Show(args) => commands::show::run(args, &ctx),
        Command::Open(args) => commands::open::run(args, &ctx),
        Command::Data(args) => commands::data::run(args, &mut ctx),
        Command::Start(args) => commands::start::run(args, &mut ctx),
        Command::Stop(args) => commands::stop::run(args, &mut ctx),
        Command::Done(args) => commands::done::run(args, &mut ctx),
        Command::Cancel(args) => commands::cancel::run(args, &mut ctx),
        Command::Edit(args) => commands::edit::run(args, &mut ctx),
        Command::Delete(args) => commands::delete::run(args, &mut ctx),
        Command::Move(args) => commands::move_cmd::run(args, &mut ctx),
        Command::Context(args) => commands::context::run(args, &mut ctx),
        Command::Resource(args) => commands::resource::run(args, &mut ctx),
        Command::User(args) => commands::user::run(args, &mut ctx),
        Command::Forecast(args) => commands::forecast::run(args, &ctx),
        Command::Sync(args) => commands::sync::run(args, &mut ctx),
        Command::Tag(args) => commands::tag::run(args, &mut ctx),
        Command::Tree(args) => commands::tree::run(args, &ctx),
        Command::Plugin(args) => commands::plugin::run(args, &mut ctx),
        Command::Tutorial(_) => unreachable!("handled above"),
    };

    if let Err(ref e) = result {
        tracing::error!(cmd = cmd_name, "{e:#}");
    }

    if result.is_ok() && is_mutation && (cli_autosync || ctx.config.autosync) && !cli_no_autosync {
        let sync_args = sync_cmd::Args { push_only: false, pull_only: false };
        if let Err(e) = sync_cmd::run(sync_args, &mut ctx) {
            eprintln!("autosync failed: {e:#}");
            tracing::error!(cmd = "sync", "autosync: {e:#}");
        }
    }

    // Notify subscribed plugins of any task changes, after the repo lock has
    // been released (and after autosync). Best-effort; never fails the command.
    if result.is_ok() {
        let events = ctx.repo.take_task_events();
        let repo_root = ctx.repo.repo_root.clone();
        next::core::plugin::notify(&repo_root, &events, ctx.repo.plugin_origin());
        for ev in &events {
            if ev.verb == "delete" {
                let _ = next::core::plugin::registry::prune_task(&repo_root, ev.task_id);
            }
        }
    }

    result
}
