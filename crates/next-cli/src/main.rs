mod cli;
mod log;
mod resolve;

use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use cli::{Cli, Command};
use log::Logger;
use next::{BackendKind, Config, Store, VcsBackend};

pub struct AppContext {
    pub config: Config,
    pub store: Box<dyn Store>,
    pub vcs: Box<dyn VcsBackend>,
    /// Absolute path to the repository root (contains `.git` and `state.toml`).
    pub repo_root: PathBuf,
    pub log: Logger,
}

impl AppContext {
    fn new() -> anyhow::Result<Self> {
        let config = load_config();

        let (store, vcs, repo_root): (Box<dyn Store>, Box<dyn VcsBackend>, PathBuf) =
            match config.backend.kind {
                BackendKind::Local => {
                    let root = find_repo_root().context(
                        "not inside a task repository — run `git init` and `next add` to start",
                    )?;
                    let (s, v) = next_storage::open(root.clone())
                        .context("failed to open local task store")?;
                    (Box::new(s), Box::new(v), root)
                }
                BackendKind::Remote => {
                    let remote = config.backend.remote.as_ref().context(
                        "backend.kind = \"remote\" but no [backend.remote] section in config",
                    )?;
                    // Use the current directory as repo_root for the log; the
                    // remote backend does not require a git repository.
                    let root = std::env::current_dir()
                        .context("cannot determine current directory")?;
                    let store = next_remote_storage::RemoteStore::new(
                        &remote.url,
                        remote.token.as_deref(),
                    );
                    let vcs = next_remote_storage::RemoteVcs::new(
                        &remote.url,
                        remote.token.as_deref(),
                    );
                    (Box::new(store), Box::new(vcs), root)
                }
            };

        let log = Logger::new(&repo_root);
        Ok(Self {
            config,
            store,
            vcs,
            repo_root,
            log,
        })
    }
}

/// Walks up from the current directory until a `.git` directory is found.
fn find_repo_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Loads configuration from `$XDG_CONFIG_HOME/task-manager/config.toml`.
/// Returns `Config::default()` when the file is absent or unreadable.
fn load_config() -> Config {
    let path = dirs::config_dir()
        .map(|d| d.join("task-manager").join("config.toml"));

    let Some(path) = path else { return Config::default() };
    let Ok(content) = std::fs::read_to_string(&path) else { return Config::default() };

    match toml::from_str::<Config>(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("warning: failed to parse config file {}: {e}", path.display());
            Config::default()
        }
    }
}

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
        Command::User(args) => cli::commands::user::run(args, &mut ctx),
        Command::Forecast(args) => cli::commands::forecast::run(args, &mut ctx),
        Command::Review(args) => cli::commands::review::run(args, &mut ctx),
        Command::Sync(args) => cli::commands::sync::run(args, &mut ctx),
        Command::Import(args) => cli::commands::import::run(args, &mut ctx),
        Command::Export(args) => cli::commands::export::run(args, &mut ctx),
    };

    if let Err(ref e) = result {
        ctx.log.error(cmd_name, &format!("{e:#}"));
    }

    result
}
