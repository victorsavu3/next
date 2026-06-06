//! Immediate-mode drawing for the TUI. This module is rendering-only: it reads
//! from [`App`] and never mutates it.

use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::core::domain::task::{Priority, Recurrence, Status};
use crate::core::scoring::ScoredTask;

use super::VERSION;
use super::app::{App, DetailData, Mode};

/// Draws a full frame: title bar, filter bar, list/detail split, and footer.
///
/// Takes `&mut App` so the detail pane can clamp its scroll offset against the
/// content height, which is only known once the pane area is laid out.
pub fn draw(frame: &mut Frame, app: &mut App) {
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

    // Modal overlays float over the whole frame when active.
    match app.mode() {
        Mode::Edit => edit_modal::draw(frame, frame.area(), app),
        Mode::ConfirmDelete => confirm_popup::draw(frame, frame.area(), app),
        Mode::MovePicker => move_popup::draw(frame, frame.area(), app),
        Mode::Normal | Mode::Filter => {}
    }
}

/// Computes a rectangle `pct_x` × `pct_y` percent of `area`, centered within it.
/// Shared popup helper for the edit modal (and reusable by later confirm/action
/// popups in T7).
pub fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vertical[1])[1]
}

/// Splits the body into the list (60%) and detail (40%) panes.
fn draw_body(frame: &mut Frame, area: Rect, app: &mut App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    draw_list(frame, cols[0], app);
    draw_detail(frame, cols[1], app);
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

fn draw_detail(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::default().borders(Borders::ALL).title(" detail ");

    let Some(detail) = app.selected_detail() else {
        let placeholder = Paragraph::new("Nothing selected.")
            .block(block)
            .style(Style::default().add_modifier(Modifier::DIM));
        frame.render_widget(placeholder, area);
        return;
    };

    let today = app.today();
    // Wrap to the inner width so the scroll offset maps to displayed rows.
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2);
    let lines = detail_lines(&detail, today, inner_width);

    // Clamp the scroll so the last line can sit at the bottom of the pane but
    // we never scroll past the content.
    let content_height = lines.len() as u16;
    let max_scroll = content_height.saturating_sub(inner_height);
    app.clamp_detail_scroll(max_scroll);
    let scroll = app.detail_scroll();

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

/// Builds the full `next show`-parity detail view as styled, width-wrapped
/// lines. Optional fields are omitted when absent, mirroring the CLI.
fn detail_lines(detail: &DetailData, today: NaiveDate, width: usize) -> Vec<Line<'static>> {
    let task = detail.task;
    let mut lines = Vec::new();

    let short_id = task.id.to_string().replace('-', "")[..8].to_owned();
    lines.push(field("ID", short_id));
    lines.push(Line::from(Span::styled(
        task.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    // Status / priority carry colour.
    lines.push(Line::from(vec![
        label_span("Status"),
        status_span(&task.status),
    ]));
    lines.push(Line::from(vec![
        label_span("Priority"),
        priority_span(&task.priority),
    ]));

    // Score line + compact breakdown of non-zero factors.
    lines.push(score_line(&detail.breakdown));

    if let Some(due) = task.due {
        let style = match DueState::classify(due, today) {
            DueState::Overdue => Style::default().fg(Color::Red),
            DueState::Today => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            DueState::Future => Style::default(),
        };
        lines.push(Line::from(vec![
            label_span("Due"),
            Span::styled(due.format("%Y-%m-%d").to_string(), style),
        ]));
    }
    if let Some(start) = task.start {
        lines.push(field("Start", start.format("%Y-%m-%d").to_string()));
    }
    if !task.tags.is_empty() {
        lines.push(Line::from(vec![
            label_span("Tags"),
            Span::styled(
                task.tags.join("  "),
                Style::default().add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
    if let Some((id, title)) = &detail.parent {
        lines.push(field("Parent", format!("[{id}] {title}")));
    }
    if !detail.children.is_empty() {
        lines.push(Line::from(label_span("Children")));
        for (id, title) in &detail.children {
            lines.push(Line::from(Span::raw(format!("  [{id}] {title}"))));
        }
    }
    if !detail.blockers.is_empty() {
        lines.push(field("Blocked", detail.blockers.join(", ")));
    }
    if let Some(assignee) = &task.assignee {
        lines.push(field("Assignee", assignee.clone()));
    }
    if let Some(slug) = &task.slug {
        lines.push(field("Slug", slug.clone()));
    }
    if let Some(rec) = &task.recurrence {
        let value = match rec {
            Recurrence::Schedule { rrule, .. } => format!("schedule ({rrule})"),
            Recurrence::Completion { interval_days, .. } => {
                format!("{interval_days}d after completion")
            }
        };
        lines.push(field("Recur", value));
    }
    lines.push(field(
        "Created",
        task.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
    ));
    lines.push(field(
        "Updated",
        task.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
    ));
    if !task.data.is_empty() {
        let mut keys: Vec<&String> = task.data.keys().collect();
        keys.sort();
        for key in keys {
            lines.push(field(&format!("Data[{key}]"), task.data[key].to_string()));
        }
    }
    if let Some(url) = &task.url {
        lines.push(field("URL", url.clone()));
    }
    if let Some(desc) = &task.description {
        lines.push(Line::raw(""));
        lines.push(Line::from(label_span("Description")));
        lines.extend(block_lines(desc));
    }
    if let Some(notes) = &task.notes {
        lines.push(Line::raw(""));
        lines.push(Line::from(label_span("Notes")));
        lines.extend(block_lines(notes));
    }

    // `Paragraph::scroll` works on logical lines but `Wrap` reflows them at
    // render time, which would desync our scroll clamp from what is shown. So
    // pre-wrap here and feed already-wrapped lines to a non-trimming paragraph.
    wrap_lines(lines, width)
}

/// The `Score: <total>  (factor …)` line with the non-zero breakdown.
fn score_line(bd: &crate::core::scoring::ScoreBreakdown) -> Line<'static> {
    let mut parts: Vec<String> = bd
        .nonzero_factors()
        .into_iter()
        .map(|(name, value)| format!("{name} {value:.2}"))
        .collect();
    if bd.no_time_urgency {
        parts.push("no-time-urgency".to_owned());
    }
    let value = if parts.is_empty() {
        format!("{:.2}", bd.total)
    } else {
        format!("{:.2}  ({})", bd.total, parts.join("  "))
    };
    Line::from(vec![label_span("Score"), Span::raw(value)])
}

/// A dimmed `Label: ` span (note the trailing space).
fn label_span(label: &str) -> Span<'static> {
    Span::styled(
        format!("{label}: "),
        Style::default().add_modifier(Modifier::DIM),
    )
}

/// A `Label: value` detail line with a dimmed label and a plain value.
fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![label_span(label), Span::raw(value)])
}

/// Coloured status value.
fn status_span(status: &Status) -> Span<'static> {
    let style = match status {
        Status::Started => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        Status::Done => Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
        Status::Cancelled => Style::default().add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
        Status::Open => Style::default(),
    };
    Span::styled(status.to_string(), style)
}

/// Coloured priority value.
fn priority_span(priority: &Priority) -> Span<'static> {
    let style = match priority {
        Priority::High => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        Priority::Medium => Style::default(),
        Priority::Low => Style::default().add_modifier(Modifier::DIM),
    };
    Span::styled(priority.to_string(), style)
}

/// Splits a free-text block into one (plain) line per physical line.
fn block_lines(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|l| Line::from(Span::raw(l.to_owned())))
        .collect()
}

/// Hard-wraps each line to `width` columns, preserving span styling. Splits on
/// character boundaries so the rendered height equals `out.len()`, keeping the
/// scroll offset in sync with what the user sees.
fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let mut current: Vec<Span<'static>> = Vec::new();
        let mut col = 0usize;
        for span in line.spans {
            let style = span.style;
            for ch in span.content.chars() {
                if col == width {
                    out.push(Line::from(std::mem::take(&mut current)));
                    col = 0;
                }
                current.push(Span::styled(ch.to_string(), style));
                col += 1;
            }
        }
        out.push(Line::from(current));
    }
    out
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hint = match app.mode() {
        Mode::Normal => {
            "q quit  j/k nav  r reload  e edit  d done  s start/stop  c cancel  m move  o open  x del  / filter  A/F/U flags"
        }
        Mode::Filter => "Enter apply  Esc cancel",
        Mode::Edit => {
            "Tab/↑↓ field  Space/←→ toggle  Enter commit (tag/data)  Ctrl-S save  Esc cancel"
        }
        Mode::ConfirmDelete => "y/Enter delete  n/Esc cancel",
        Mode::MovePicker => "type to search  ↑↓ select  Enter move  Esc cancel",
    };
    let mut spans = vec![Span::styled(hint, Style::default().add_modifier(Modifier::DIM))];
    if let Some(status) = app.status() {
        spans.push(Span::raw("  |  "));
        spans.push(Span::raw(status.to_owned()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Rendering for the edit modal — a centered popup with one labelled row per
/// form field, the focused row highlighted, multi-line textareas for
/// description/notes, and live date previews.
mod edit_modal {
    use ratatui::Frame;
    use ratatui::layout::{Constraint, Direction, Layout, Rect};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    use crate::tui::app::App;
    use crate::tui::edit::{EditForm, Field, RecurMode};

    use super::centered_rect;

    pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
        let Some(form) = app.edit_form() else {
            return;
        };
        let today = app.today();

        let popup = centered_rect(80, 90, area);
        frame.render_widget(Clear, popup);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" edit task  (Ctrl-S save · Esc cancel) ")
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        // One line per single-line/value field; the two textareas get a small
        // fixed block each. Lay rows out top-to-bottom.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(1), // due
                Constraint::Length(1), // start
                Constraint::Length(1), // priority
                Constraint::Length(1), // tags
                Constraint::Length(1), // assignee
                Constraint::Length(1), // url
                Constraint::Length(1), // score adjustment
                Constraint::Length(1), // long term
                Constraint::Length(1), // recur mode
                Constraint::Length(1), // recur rule
                Constraint::Length(1), // recur completion
                Constraint::Length(1), // recur snap
                Constraint::Length(1), // data key
                Constraint::Length(1), // data value
                Constraint::Length(1), // data list header
                Constraint::Length(3), // description
                Constraint::Length(3), // notes
                Constraint::Min(0),    // tag/data summary
            ])
            .split(inner);

        text_row(frame, rows[0], "Title", form.title.value(), form.focus == Field::Title);
        date_row(frame, rows[1], "Due", form.due.value(), form.due_preview(today), form.focus == Field::Due);
        date_row(frame, rows[2], "Start", form.start.value(), form.start_preview(today), form.focus == Field::Start);
        value_row(frame, rows[3], "Priority", &form.priority.to_string(), form.focus == Field::Priority);
        text_row(frame, rows[4], "Add tag", form.tag_input.value(), form.focus == Field::Tags);
        text_row(frame, rows[5], "Assignee", form.assignee.value(), form.focus == Field::Assignee);
        text_row(frame, rows[6], "URL", form.url.value(), form.focus == Field::Url);
        text_row(frame, rows[7], "Score adj", form.score_adjustment.value(), form.focus == Field::ScoreAdjustment);
        value_row(frame, rows[8], "Long-term", if form.long_term { "yes" } else { "no" }, form.focus == Field::LongTerm);
        value_row(frame, rows[9], "Recur", form.recur_mode_label(), form.focus == Field::RecurMode);
        text_row(frame, rows[10], "  RRULE", form.recur_rule.value(), form.focus == Field::RecurRule);
        text_row(frame, rows[11], "  Interval", form.recur_completion.value(), form.focus == Field::RecurCompletion);
        text_row(frame, rows[12], "  Snap", form.recur_snap.value(), form.focus == Field::RecurSnap);
        text_row(frame, rows[13], "Data key", form.data_key.value(), form.focus == Field::DataKey);
        text_row(frame, rows[14], "Data val", form.data_value.value(), form.focus == Field::DataValue);

        // Description / notes textareas.
        labelled_textarea(frame, rows[16], "Description", &form.description, form.focus == Field::Description);
        labelled_textarea(frame, rows[17], "Notes", &form.notes, form.focus == Field::Notes);

        // Summary of committed tags + data entries.
        draw_summary(frame, rows[18], form);
    }

    /// `Label: value` row, focused row reversed. Single-line text fields.
    fn text_row(frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
        let style = row_style(focused);
        let line = Line::from(vec![
            Span::styled(format!("{label:>10}: "), Style::default().add_modifier(Modifier::DIM)),
            Span::styled(value.to_owned(), style),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    /// A non-text value row (cycled enums / toggles).
    fn value_row(frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
        let style = row_style(focused).fg(Color::Yellow);
        let line = Line::from(vec![
            Span::styled(format!("{label:>10}: "), Style::default().add_modifier(Modifier::DIM)),
            Span::styled(value.to_owned(), style),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    /// A date row with a parsed-date preview (or the parse error) appended.
    fn date_row(
        frame: &mut Frame,
        area: Rect,
        label: &str,
        value: &str,
        preview: Result<Option<chrono::NaiveDate>, String>,
        focused: bool,
    ) {
        let style = row_style(focused);
        let (preview_text, preview_style) = match preview {
            Ok(Some(d)) => (format!("  → {d}"), Style::default().fg(Color::Green)),
            Ok(None) => (String::new(), Style::default()),
            Err(e) => (format!("  ✗ {e}"), Style::default().fg(Color::Red)),
        };
        let line = Line::from(vec![
            Span::styled(format!("{label:>10}: "), Style::default().add_modifier(Modifier::DIM)),
            Span::styled(value.to_owned(), style),
            Span::styled(preview_text, preview_style),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn labelled_textarea(
        frame: &mut Frame,
        area: Rect,
        label: &str,
        textarea: &tui_textarea::TextArea,
        focused: bool,
    ) {
        let border = if focused { Color::Cyan } else { Color::DarkGray };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {label} "))
            .border_style(Style::default().fg(border));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(textarea, inner);
    }

    fn draw_summary(frame: &mut Frame, area: Rect, form: &EditForm) {
        let mut lines = Vec::new();
        if !form.tags.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Tags: ", Style::default().add_modifier(Modifier::DIM)),
                Span::styled(form.tags.join("  "), Style::default().add_modifier(Modifier::ITALIC)),
            ]));
        }
        if !form.data.is_empty() {
            for (k, v) in &form.data {
                lines.push(Line::from(Span::raw(format!("  {k} = {v}"))));
            }
        }
        if matches!(form.recur_mode, RecurMode::Schedule | RecurMode::Completion) {
            lines.push(Line::from(Span::styled(
                "Snap: blank, next-workday, monday…sunday, dom:N",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn row_style(focused: bool) -> Style {
        if focused {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    }
}

/// The delete-confirmation popup: a small centered box showing the task title
/// with a `y/N` prompt.
mod confirm_popup {
    use ratatui::Frame;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

    use crate::tui::app::App;

    use super::centered_rect;

    pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
        let Some(task) = app.selected_task() else {
            return;
        };

        let popup = centered_rect(60, 30, area);
        frame.render_widget(Clear, popup);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" delete task ")
            .border_style(Style::default().fg(Color::Red));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let lines = vec![
            Line::from(Span::raw("Delete this task?")),
            Line::from(Span::styled(
                task.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::from(vec![
                Span::styled("y/Enter", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw(" delete    "),
                Span::styled("n/Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" cancel"),
            ]),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

/// The move (parent-picker) popup: a search line over a scrollable list of
/// candidate parents (the moved task and its descendants are already excluded).
mod move_popup {
    use ratatui::Frame;
    use ratatui::layout::{Constraint, Direction, Layout, Rect};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

    use crate::tui::app::App;

    use super::centered_rect;

    pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
        let Some(picker) = app.move_picker() else {
            return;
        };

        let popup = centered_rect(70, 70, area);
        frame.render_widget(Clear, popup);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" move: pick new parent  (Enter move · Esc cancel) ")
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(inner);

        // Search line.
        let search = Line::from(vec![
            Span::styled("search: ", Style::default().fg(Color::Yellow)),
            Span::raw(picker.query().value().to_owned()),
        ]);
        frame.render_widget(Paragraph::new(search), rows[0]);

        // Candidate list (filtered by the query).
        let items: Vec<ListItem> = picker
            .filtered()
            .iter()
            .map(|c| ListItem::new(Line::from(Span::raw(c.label.clone()))))
            .collect();
        let list = List::new(items)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        let mut state = ListState::default();
        if picker.filtered().is_empty() {
            state.select(None);
        } else {
            state.select(Some(picker.selected()));
        }
        frame.render_stateful_widget(list, rows[1], &mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use crate::core::bootstrap;
    use crate::core::domain::task::{Recurrence, Task};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::tui::app::{Action, App};
    use crate::tui::config::ConfigSource;

    /// Builds an `App` over a real (empty) repo containing `tasks`, reloaded so
    /// the detail pane sees them through the cached vecs.
    fn app_with_tasks(tasks: Vec<Task>) -> App {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.keep();
        let gitrepo = git2::Repository::init(&root).unwrap();
        let mut cfg = gitrepo.config().unwrap();
        cfg.set_str("user.name", "Test").unwrap();
        cfg.set_str("user.email", "test@test.com").unwrap();
        let config = Config::default();
        let mut repo = bootstrap::open_repository(root, &config).unwrap();
        for task in &tasks {
            repo.store_mut().save_task(task).unwrap();
        }
        let today = chrono::NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        let mut app = App::new(config, repo, ConfigSource::Default, today);
        app.reload().unwrap();
        app
    }

    fn app_over_tempdir() -> App {
        app_with_tasks(Vec::new())
    }

    /// A parent + child where the child exercises most optional fields.
    fn rich_tasks() -> Vec<Task> {
        let mut parent = Task::new("the parent project");
        parent.tags = vec!["#proj".to_owned()];

        let mut blocker = Task::new("the blocker");

        let mut child = Task::new("richly populated task");
        child.parent_id = Some(parent.id);
        child.blocked_by = vec![blocker.id];
        child.due = Some(chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()); // overdue
        child.start = Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 20).unwrap());
        child.tags = vec!["#rust".to_owned(), "#urgent".to_owned()];
        child.assignee = Some("alice".to_owned());
        child.slug = Some("rich-task".to_owned());
        child.priority = crate::core::domain::task::Priority::High;
        child.url = Some("https://example.com".to_owned());
        child.description = Some("A long description.\nWith multiple lines.".to_owned());
        child.notes = Some("Some notes here.".to_owned());
        child.recurrence = Some(Recurrence::Completion {
            interval_days: 7,
            snap: None,
        });
        child
            .data
            .insert("zeta".to_owned(), serde_json::json!("last"));
        child
            .data
            .insert("alpha".to_owned(), serde_json::json!(42));

        // Mutate the blocker after capturing its id above.
        blocker.title = "the blocker".to_owned();

        vec![parent, blocker, child]
    }

    #[test]
    fn renders_one_frame_without_panicking() {
        let mut app = app_over_tempdir();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[test]
    fn renders_edit_modal_without_panicking() {
        let mut app = app_with_tasks(rich_tasks());
        // Reveal the blocked child, select it, then open the edit modal.
        app.update(Action::ToggleAll);
        let idx = app
            .tasks()
            .iter()
            .position(|s| s.task.title == "richly populated task")
            .unwrap();
        for _ in 0..idx {
            app.update(Action::SelectNext);
        }
        app.update(Action::OpenEdit);
        assert_eq!(app.mode(), Mode::Edit);
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[test]
    fn renders_delete_confirm_popup_without_panicking() {
        let mut app = app_with_tasks(vec![Task::new("delete me")]);
        app.update(Action::OpenDelete);
        assert_eq!(app.mode(), Mode::ConfirmDelete);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[test]
    fn renders_move_picker_without_panicking() {
        let mut app = app_with_tasks(vec![
            Task::new("child"),
            Task::new("candidate parent"),
        ]);
        app.update(Action::OpenMove);
        assert_eq!(app.mode(), Mode::MovePicker);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[test]
    fn renders_filter_mode_without_panicking() {
        let mut app = app_over_tempdir();
        app.update(Action::OpenFilter);
        assert_eq!(app.mode(), Mode::Filter);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[test]
    fn renders_rich_detail_without_panicking() {
        let mut app = app_with_tasks(rich_tasks());
        // The child is blocked, so reveal it via `--all`.
        app.update(Action::ToggleAll);
        // Select the child (the richly-populated one).
        let idx = app
            .tasks()
            .iter()
            .position(|s| s.task.title == "richly populated task")
            .unwrap();
        while app.selected() < idx {
            app.update(Action::SelectNext);
        }
        assert_eq!(app.selected_task().unwrap().title, "richly populated task");

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[test]
    fn detail_lines_cover_show_fields() {
        let mut app = app_with_tasks(rich_tasks());
        app.update(Action::ToggleAll);
        let idx = app
            .tasks()
            .iter()
            .position(|s| s.task.title == "richly populated task")
            .unwrap();
        let detail = {
            for _ in 0..idx {
                app.update(Action::SelectNext);
            }
            assert_eq!(app.selected_task().unwrap().title, "richly populated task");
            // Render to a wide pane so nothing wraps, then collect the text.
            let lines = detail_lines(&app.selected_detail().unwrap(), app.today(), 200);
            lines
                .into_iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let text = detail.join("\n");
        assert!(text.contains("ID: "), "{text}");
        assert!(text.contains("richly populated task"));
        assert!(text.contains("Status: open"));
        assert!(text.contains("Priority: high"));
        assert!(text.contains("Score: "));
        assert!(text.contains("Due: 2026-06-01"));
        assert!(text.contains("Start: 2026-05-20"));
        assert!(text.contains("#rust"));
        assert!(text.contains("Parent: [") && text.contains("the parent project"));
        // The selected child has no children of its own.
        assert!(!text.contains("Children:"));
        assert!(text.contains("Blocked: [") && text.contains("the blocker"));
        assert!(text.contains("Assignee: alice"));
        assert!(text.contains("Slug: rich-task"));
        assert!(text.contains("Recur: 7d after completion"));
        assert!(text.contains("Created: "));
        assert!(text.contains("Updated: "));
        // Data entries sorted: alpha before zeta.
        let a = text.find("Data[alpha]").unwrap();
        let z = text.find("Data[zeta]").unwrap();
        assert!(a < z, "data keys not sorted: {text}");
        assert!(text.contains("URL: https://example.com"));
        assert!(text.contains("Description"));
        assert!(text.contains("A long description."));
        assert!(text.contains("Notes"));
        assert!(text.contains("Some notes here."));
    }
}
