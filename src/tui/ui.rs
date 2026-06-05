//! Immediate-mode drawing for the TUI. This module is rendering-only: it reads
//! from [`App`] and never mutates it.

use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::core::domain::task::{Priority, Status, Task};
use crate::core::scoring::ScoredTask;

use super::VERSION;
use super::app::{App, Mode};

/// Draws a full frame: title bar, filter bar, list/detail split, and footer.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title bar
            Constraint::Length(1), // filter bar
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    draw_title(frame, chunks[0], app);
    draw_filter_bar(frame, chunks[1], app);
    draw_body(frame, chunks[2], app);
    draw_footer(frame, chunks[3], app);
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![
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
    ];
    for toggle in active_toggles(app) {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            toggle,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ));
    }
    let title = Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, area);
}

/// The active flag toggles as bracketed badges, e.g. `[all] [future]`.
fn active_toggles(app: &App) -> Vec<String> {
    let mut out = Vec::new();
    if app.filter_all() {
        out.push("[all]".to_owned());
    }
    if app.filter_future() {
        out.push("[future]".to_owned());
    }
    if app.filter_all_users() {
        out.push("[all-users]".to_owned());
    }
    out
}

/// One-line filter bar above the list. In [`Mode::Filter`] it is an editable
/// input with a visible cursor; otherwise it shows a summary of the active
/// filter (tokens + toggles), or a hint when nothing is set.
fn draw_filter_bar(frame: &mut Frame, area: Rect, app: &App) {
    if app.mode() == Mode::Filter {
        let input = app.filter_input();
        // Scroll the value so the cursor stays visible on a narrow bar.
        let prefix = "filter: ";
        let inner_width = area.width.saturating_sub(prefix.len() as u16) as usize;
        let scroll = input.visual_scroll(inner_width.max(1));
        let line = Line::from(vec![
            Span::styled(prefix, Style::default().fg(Color::Yellow)),
            Span::raw(input.value()[scroll.min(input.value().len())..].to_owned()),
        ]);
        frame.render_widget(Paragraph::new(line), area);

        // Place the terminal cursor at the edit position.
        let cursor_col =
            prefix.len() as u16 + (input.visual_cursor().saturating_sub(scroll)) as u16;
        frame.set_cursor_position(Position::new(
            area.x + cursor_col.min(area.width.saturating_sub(1)),
            area.y,
        ));
        return;
    }

    let tokens = app.filter_tokens();
    let summary = if tokens.is_empty() {
        Span::styled(
            "filter: (none) — press / to edit",
            Style::default().add_modifier(Modifier::DIM),
        )
    } else {
        Span::styled(
            format!("filter: {}", tokens.join(" ")),
            Style::default().fg(Color::Yellow),
        )
    };
    frame.render_widget(Paragraph::new(Line::from(summary)), area);
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
        .title(format!(" {} tasks ", app.tasks().len()));

    if app.tasks().is_empty() {
        let empty = Paragraph::new("No tasks match the current view.")
            .block(block)
            .style(Style::default().add_modifier(Modifier::DIM));
        frame.render_widget(empty, area);
        return;
    }

    // Width available for the row content, inside the borders.
    let inner_width = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = app
        .tasks()
        .iter()
        .map(|s| list_item(s, app.today(), inner_width))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    state.select(Some(app.selected()));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Renders one task row: `<score> <prio> <due> <title>  <tags>`.
///
/// Overdue rows are red, due-today rows yellow/bold, started rows emphasised,
/// low-priority rows dimmed. Tags are appended dimmed when they fit.
fn list_item(scored: &ScoredTask, today: NaiveDate, width: usize) -> ListItem<'static> {
    let task = &scored.task;

    let due_state = task.due.map(|d| DueState::classify(d, today));
    let started = task.status == Status::Started;
    let low = task.priority == Priority::Low;

    // Base style for the whole row, by urgency / priority.
    let base = match due_state {
        Some(DueState::Overdue) => Style::default().fg(Color::Red),
        Some(DueState::Today) => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        _ if started => Style::default().add_modifier(Modifier::BOLD),
        _ if low => Style::default().add_modifier(Modifier::DIM),
        _ => Style::default(),
    };

    let due = task
        .due
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "          ".to_owned());

    let mut spans = vec![
        Span::styled(format!("{:5.1}", scored.score), base.add_modifier(Modifier::DIM)),
        Span::raw(" "),
        priority_marker(&task.priority, base),
        Span::raw(" "),
        Span::styled(due, base.add_modifier(Modifier::DIM)),
        Span::raw("  "),
        Span::styled(task.title.clone(), base),
    ];

    // Append tags dimmed if there is room left on the line.
    if !task.tags.is_empty() {
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let tag_str = format!("  {}", task.tags.join(" "));
        if used + tag_str.chars().count() <= width {
            spans.push(Span::styled(
                tag_str,
                base.add_modifier(Modifier::DIM | Modifier::ITALIC),
            ));
        }
    }

    ListItem::new(Line::from(spans))
}

/// Whether a due date is overdue, due today, or in the future.
enum DueState {
    Overdue,
    Today,
    Future,
}

impl DueState {
    fn classify(due: NaiveDate, today: NaiveDate) -> Self {
        if due < today {
            DueState::Overdue
        } else if due == today {
            DueState::Today
        } else {
            DueState::Future
        }
    }
}

/// A single-character priority marker, layered over the row's base style.
fn priority_marker(priority: &Priority, base: Style) -> Span<'static> {
    match priority {
        Priority::High => Span::styled("!", base.fg(Color::Red).add_modifier(Modifier::BOLD)),
        Priority::Medium => Span::styled("·", base),
        Priority::Low => Span::styled("˅", base.add_modifier(Modifier::DIM)),
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
    let hint = match app.mode() {
        Mode::Normal => {
            "q quit  j/k nav  g/G first/last  r reload  / filter  A all  F future  U all-users"
        }
        Mode::Filter => "Enter apply  Esc cancel",
    };
    let mut spans = vec![Span::styled(hint, Style::default().add_modifier(Modifier::DIM))];
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

    use crate::tui::app::{Action, App};
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

    #[test]
    fn renders_filter_mode_without_panicking() {
        let mut app = app_over_tempdir();
        app.update(Action::OpenFilter);
        assert_eq!(app.mode(), Mode::Filter);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
    }
}
