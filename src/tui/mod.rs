//! Terminal UI (`next-tui`) for the task manager.
//!
//! This is a thin, self-contained front-end built on top of the `next` core
//! library. It is gated behind the `tui` feature and ships as the `next-tui`
//! binary.

// The TUI carries its OWN version, deliberately decoupled from the crate
// version (currently 1.1.0). The terminal UI evolves on its own cadence, so we
// track it independently here and bump it as the TUI matures.
pub const VERSION: &str = "0.1.0";

use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Run the terminal UI.
///
/// Sets up the terminal (raw mode + alternate screen via [`ratatui::init`],
/// which also installs a panic hook that restores the terminal on panic), runs
/// the event loop, and restores the terminal on exit. Restoration happens on
/// both normal return and panic.
pub fn run() -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal);
    // Restore on normal/`?`-error exit; the panic hook handles the panic path.
    ratatui::restore();
    result
}

/// The synchronous draw / input loop. Returns when the user asks to quit.
fn event_loop(terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
    loop {
        terminal.draw(draw)?;

        if let Event::Key(key) = event::read()? {
            if should_quit(key) {
                break;
            }
        }
    }
    Ok(())
}

/// Returns `true` when the given key event should terminate the UI.
fn should_quit(key: KeyEvent) -> bool {
    // Only react to presses (Windows also emits Release/Repeat events).
    if key.kind != KeyEventKind::Press {
        return false;
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}

/// Render a single frame: a title bar, an empty body, and a footer hint.
fn draw(frame: &mut Frame) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title bar
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    let title = Paragraph::new(Line::from(Span::styled(
        format!("next-tui v{VERSION}"),
        Style::default().add_modifier(Modifier::BOLD),
    )))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, chunks[0]);

    let footer = Paragraph::new(Line::from(Span::styled(
        "q: quit",
        Style::default().add_modifier(Modifier::DIM),
    )));
    frame.render_widget(footer, chunks[2]);
}
