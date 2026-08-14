use std::time::Duration;

use anyhow::Context as _;
use clap::Parser;
use next::{
    cli::commands::sync as sync_cmd,
    cli::{commands, Cli, Command},
    core, AppContext,
};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // `--log-level` sets the default filter directive; without it we fall back to
    // the historical default (error), still honouring any `RUST_LOG` directives.
    let default_directive = cli
        .log_level
        .map(next::cli::LogLevel::to_level_filter)
        .unwrap_or(tracing::level_filters::LevelFilter::ERROR);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(default_directive.into())
                .from_env_lossy(),
        )
        .init();

    // Init, Tutorial, and Config run before the repository exists — handle them before AppContext.
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
    if let Some(Command::Config(args)) = cli.command {
        return commands::config::run(args, cli.config.as_deref());
    }

    // Raw sync flags. clap guarantees the mutually-exclusive ones can't be set
    // together (see the `conflicts_with*` attributes on `Cli`).
    let cli_autosync = cli.autosync;
    let cli_no_autosync = cli.no_autosync;
    let cli_offline = cli.offline;
    let cli_autopull = cli.autopull;
    let cli_no_autopull = cli.no_autopull;
    let cli_autopush = cli.autopush;
    let cli_no_autopush = cli.no_autopush;
    let mut ctx = AppContext::new(cli.config.as_deref(), cli.repo.as_deref())?;

    // Resolve the two orthogonal sync capabilities for this invocation.
    // Precedence: granular flag → master flag → config default.
    let effective_autopull = if cli_autopull {
        true
    } else if cli_no_autopull {
        false
    } else if cli_autosync {
        true
    } else if cli_no_autosync || cli_offline {
        false
    } else {
        ctx.config.sync.autopull
    };
    let effective_autopush = if cli_autopush {
        true
    } else if cli_no_autopush {
        false
    } else if cli_autosync {
        true
    } else if cli_no_autosync || cli_offline {
        false
    } else {
        ctx.config.sync.autopush
    };

    // Default to `list` when no subcommand is given.
    let command = cli.command.unwrap_or_else(|| {
        Command::List(commands::list::Args {
            future: false,
            all: false,
            closed: false,
            archived: false,
            all_users: false,
            json: false,
            format: None,
            count: false,
            explain: false,
            fields: vec![],
            limit: None,
            page_size: None,
            page: 1,
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
        Command::User(_) => "user",
        Command::Forecast(_) => "forecast",
        Command::Sync(_) => "sync",
        Command::Maintenance(args) => match args.command {
            commands::maintenance::Command::Archive(_) => "archive",
            commands::maintenance::Command::RebuildCache(_) => "maintenance",
        },
        Command::Tag(_) => "tag",
        Command::Tree(_) => "tree",
        Command::Plugin(_) => "plugin",
        Command::Tutorial(_) => unreachable!("handled above"),
        Command::Config(_) => unreachable!("handled above"),
    };

    let is_mutation = matches!(
        cmd_name,
        "add"
            | "start"
            | "stop"
            | "done"
            | "cancel"
            | "edit"
            | "delete"
            | "move"
            | "tag"
            | "data"
            | "archive"
    );

    // An explicit `next sync` always pulls and pushes, so a flag that disables
    // either half contradicts the command — fail fast with an actionable error
    // rather than silently ignoring the flag.
    if cmd_name == "sync" && (cli_no_autosync || cli_offline || cli_no_autopull || cli_no_autopush)
    {
        anyhow::bail!(
            "next sync cannot run with --no-autopull/--no-autopush/--no-autosync/--offline: \
             it always pulls and pushes"
        );
    }

    // Autopull: before any command except `sync` (which pulls explicitly;
    // init/tutorial were handled earlier), pull from the remote if the local
    // copy is stale. Best-effort — it never fails the command.
    if cmd_name != "sync" {
        let opts = core::sync::StaleOpts {
            enabled: effective_autopull,
            staleness: Duration::from_secs(ctx.config.sync.staleness_secs),
            now: chrono::Utc::now(),
        };
        match core::sync::pull_if_stale(&mut ctx.repo, &opts) {
            core::sync::PullStatus::Pulled => {
                eprintln!("note: pulled latest changes (local copy was stale)");
            }
            core::sync::PullStatus::Failed(msg) => {
                eprintln!("warning: auto-pull failed: {msg}; results may be out of date");
            }
            core::sync::PullStatus::Fresh | core::sync::PullStatus::Disabled => {}
        }
    }

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
        Command::User(args) => commands::user::run(args, &mut ctx),
        Command::Forecast(args) => commands::forecast::run(args, &ctx),
        Command::Sync(args) => commands::sync::run(args, &mut ctx),
        Command::Maintenance(args) => commands::maintenance::run(args, &mut ctx),
        Command::Tag(args) => commands::tag::run(args, &mut ctx),
        Command::Tree(args) => commands::tree::run(args, &ctx),
        Command::Plugin(args) => commands::plugin::run(args, &mut ctx),
        Command::Tutorial(_) => unreachable!("handled above"),
        Command::Config(_) => unreachable!("handled above"),
    };

    if let Err(ref e) = result {
        // Merge conflicts from an explicit `next sync` exit with code 2
        // (REQUIREMENTS.md §2.2) so scripts can detect them; the detailed
        // message was already printed to stderr by the sync command.
        if e.downcast_ref::<sync_cmd::ConflictsError>().is_some() {
            std::process::exit(2);
        }
        tracing::error!(cmd = cmd_name, "{e:#}");
    }

    if result.is_ok() && is_mutation && effective_autopush {
        let sync_args = sync_cmd::Args {
            push_only: false,
            pull_only: false,
            quiet: true,
        };
        if let Err(e) = sync_cmd::run(sync_args, &mut ctx) {
            // Conflicts already printed their message inside `run`; the
            // mutation itself succeeded, so they never change its exit code.
            if e.downcast_ref::<sync_cmd::ConflictsError>().is_none() {
                eprintln!("autopush failed: {e:#}");
                tracing::error!(cmd = "sync", "autopush: {e:#}");
            }
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
