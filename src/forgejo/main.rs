//! `next-plugin-forgejo` — Forgejo integration plugin binary.

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "next-plugin-forgejo", about = "Forgejo integration plugin for next")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Import issues from the mapped repositories and reconcile resolution.
    Sync {
        /// Print the actions that would be taken without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Handle a `next` export-hook event: close the linked issue on resolution.
    Hook,
    /// Register this plugin with `next`'s export hook.
    Register,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Sync { dry_run } => {
            let _ = dry_run;
            anyhow::bail!("sync: not yet implemented")
        }
        Command::Hook => anyhow::bail!("hook: not yet implemented"),
        Command::Register => anyhow::bail!("register: not yet implemented"),
    }
}
