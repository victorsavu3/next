//! Terminal UI (`next-tui`) for the task manager.
//!
//! This is a thin, self-contained front-end built on top of the `next` core
//! library. It is gated behind the `tui` feature and ships as the `next-tui`
//! binary. The TUI deliberately does NOT depend on the `cli` module; it reuses
//! the shared core (`crate::core`) directly.

// The TUI carries its OWN version, deliberately decoupled from the crate
// version (currently 1.1.0). The terminal UI evolves on its own cadence, so we
// track it independently here and bump it as the TUI matures.
pub const VERSION: &str = "0.1.0";

pub mod app;
pub mod config;
pub mod ui;

use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::core::TaskRepository;
use crate::Config;

use app::App;
use config::ConfigSource;

/// How long to wait for an input event before redrawing. Keeps the loop
/// responsive without busy-spinning.
const TICK: Duration = Duration::from_millis(200);

/// Run the terminal UI.
///
/// Sets up the terminal (raw mode + alternate screen via [`ratatui::init`],
/// which also installs a panic hook that restores the terminal on panic), runs
/// the event loop, and restores the terminal on exit. Restoration happens on
/// both normal return and panic.
///
/// Takes the already-loaded `config`, opened `repo`, and the `source` the
/// config came from.
pub fn run(config: Config, repo: TaskRepository, source: ConfigSource) -> anyhow::Result<()> {
    let today = chrono::Local::now().date_naive();
    let mut app = App::new(config, repo, source, today);
    app.reload()?;

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    // Restore on normal/`?`-error exit; the panic hook handles the panic path.
    ratatui::restore();
    result
}

/// The synchronous draw / input loop. Returns when the user asks to quit.
fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> anyhow::Result<()> {
    while !app.should_quit() {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Block up to one tick for input; redraw on timeout so transient state
        // (e.g. a status message) stays fresh.
        if event::poll(TICK)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if let Some(action) = app.handle_key(key) {
                        app.update(action);
                    }
                }
            }
        }
    }
    Ok(())
}
