use next::tui;

fn main() -> anyhow::Result<()> {
    // Handle `--version` / `-V` before touching the terminal.
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("next-tui {}", tui::VERSION);
        return Ok(());
    }

    // Minimal tracing setup. Logs go to stderr; in the alternate-screen TUI
    // they are not visible, but remain available when redirected to a file via
    // `RUST_LOG`.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    tui::run()
}
