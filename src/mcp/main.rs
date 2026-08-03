use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use next::mcp::{
    config::McpConfig,
    git_init::clone_or_open,
    server::{build_router, AppState},
    sync_manager::{spawn_deferred_sync, spawn_periodic_sync},
};
use next::TaskRepository;

/// Minimal argv scan for `--config <path>` / `--config=<path>`.
///
/// next-mcp intentionally does not depend on clap (keeps the container image
/// lean); this one flag is all the CLI surface the server needs.
fn config_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(path) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(path));
        }
        if arg == "--config" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = McpConfig::load(config_arg())?;

    // Ensure the git repo is present (clone if needed).
    let repo_path = clone_or_open(&config)?;

    // Open the task store.
    let (store, vcs) = next::core::storage::open(repo_path.clone())
        .map_err(|e| anyhow::anyhow!("failed to open task store: {e}"))?;
    let vcs = vcs.with_credentials(config.git_user.clone(), config.git_token.clone());

    let ctx = TaskRepository::with_parts(Box::new(store), Box::new(vcs), repo_path);
    let ctx = Arc::new(Mutex::new(ctx));

    // Background deferred-sync task.
    let scheduler = spawn_deferred_sync(ctx.clone(), config.deferred_sync_delay);

    // Background periodic-sync task — shares the scheduler's semaphore.
    if let Some(interval) = config.sync_interval {
        spawn_periodic_sync(ctx.clone(), interval, &scheduler);
    }

    let state = AppState {
        ctx,
        bearer_token: config.bearer_token,
        webhook_token: config.webhook_token,
        scheduler,
        pull_before_query: config.autopull,
        staleness: config.staleness,
    };

    let router = build_router(state);

    let listener = TcpListener::bind(config.bind_addr).await?;
    eprintln!("next-mcp listening on {}", config.bind_addr);

    axum::serve(listener, router).await?;
    Ok(())
}
