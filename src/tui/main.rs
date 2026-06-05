use std::path::PathBuf;

use anyhow::bail;
use next::tui;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Handle `--version` / `-V` before touching the terminal.
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("next-tui {}", tui::VERSION);
        return Ok(());
    }

    let Args { repo, config } = parse_args(&args)?;

    // Minimal tracing setup. Logs go to stderr; in the alternate-screen TUI
    // they are not visible, but remain available when redirected to a file via
    // `RUST_LOG`.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let (config, source, repo) = tui::config::bootstrap(repo.as_deref(), config.as_deref())?;
    tui::run(config, repo, source)
}

/// Parsed command-line overrides for the TUI.
struct Args {
    repo: Option<PathBuf>,
    config: Option<PathBuf>,
}

/// Minimal manual argument parsing for `--repo <path>` and `--config <path>`.
/// `--version`/`-V` are handled earlier; anything else is an error.
fn parse_args(args: &[String]) -> anyhow::Result<Args> {
    let mut repo = None;
    let mut config = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--repo" => {
                let p = it.next().ok_or_else(|| anyhow::anyhow!("--repo requires a path"))?;
                repo = Some(PathBuf::from(p));
            }
            "--config" => {
                let p = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--config requires a path"))?;
                config = Some(PathBuf::from(p));
            }
            other => bail!("unexpected argument: {other}"),
        }
    }
    Ok(Args { repo, config })
}
