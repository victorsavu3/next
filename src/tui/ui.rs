//! Immediate-mode drawing for the TUI. This module is rendering-only: it reads
//! from [`App`] and never mutates it.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::core::domain::task::{Priority, Task};
use crate::core::scoring::ScoredTask;

use super::VERSION;
use super::app::App;

/// Draws a full frame: title bar, list/detail split, and footer.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title bar
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    draw_title(frame, chunks[0], app);
    draw_body(frame, chunks[1], app);
    draw_footer(frame, chunks[2], app);
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App) {
    let line = Line::from(vec![
        Span::styled(
            format!("next-tui v{VERSION}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::raw(app.repo_root().display().to_string()),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", app.source().label()),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ]);
    let title = Paragraph::new(line).block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, area);
}

fn draw_body(frame: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    draw_list(frame, cols[0], app);
    draw_detail(frame, cols[1], app);
}

fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" tasks ({}) ", app.tasks().len()));

    if app.tasks().is_empty() {
        let empty = Paragraph::new("No tasks match the current view.")
            .block(block)
            .style(Style::default().add_modifier(Modifier::DIM));
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = app.tasks().iter().map(list_item).collect();

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .add_modifier(Modifier::BOLD | Modifier::REVERSED),
    );

    let mut state = ListState::default();
    state.select(Some(app.selected()));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Renders one task row: `<score>  <prio marker>  <due>  <title>`.
fn list_item(scored: &ScoredTask) -> ListItem<'static> {
    let task = &scored.task;
    let due = task
        .due
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "          ".to_owned());

    let line = Line::from(vec![
        Span::styled(
            format!("{:5.1}", scored.score),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::raw(" "),
        priority_marker(&task.priority),
        Span::raw(" "),
        Span::styled(due, Style::default().add_modifier(Modifier::DIM)),
        Span::raw("  "),
        Span::raw(task.title.clone()),
    ]);
    ListItem::new(line)
}

/// A single-character priority marker, styled by level.
fn priority_marker(priority: &Priority) -> Span<'static> {
    match priority {
        Priority::High => Span::raw("!").red().bold(),
        Priority::Medium => Span::raw("·"),
        Priority::Low => Span::styled("˅", Style::default().add_modifier(Modifier::DIM)),
    }
}

fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" detail ");

    let Some(task) = app.selected_task() else {
        let placeholder = Paragraph::new("Nothing selected.")
            .block(block)
            .style(Style::default().add_modifier(Modifier::DIM));
        frame.render_widget(placeholder, area);
        return;
    };

    let paragraph = Paragraph::new(detail_lines(task))
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// The minimal detail view (full `next show` formatting is T5).
fn detail_lines(task: &Task) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            task.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        field("status", task.status.to_string()),
        field("priority", task.priority.to_string()),
        field(
            "due",
            task.due
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "—".to_owned()),
        ),
    ];

    let tags = if task.tags.is_empty() {
        "—".to_owned()
    } else {
        task.tags.join(", ")
    };
    lines.push(field("tags", tags));

    lines
}

/// A `label: value` detail line with a dimmed label.
fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::raw(value),
    ])
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        "q quit  j/↓ next  k/↑ prev  g/Home first  G/End last  r reload",
        Style::default().add_modifier(Modifier::DIM),
    )];
    if let Some(status) = app.status() {
        spans.push(Span::raw("  |  "));
        spans.push(Span::raw(status.to_owned()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::bootstrap;
    use crate::Config;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::tui::app::App;
    use crate::tui::config::ConfigSource;

    fn app_over_tempdir() -> App {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.keep();
        let gitrepo = git2::Repository::init(&root).unwrap();
        let mut cfg = gitrepo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@test.com").unwrap();
        let config = Config::default();
        let repo = bootstrap::open_repository(root, &config).unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        let mut app = App::new(config, repo, ConfigSource::Default, today);
        app.reload().unwrap();
        app
    }

    #[test]
    fn renders_one_frame_without_panicking() {
        let app = app_over_tempdir();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
    }
}
