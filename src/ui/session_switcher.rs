use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{Frame, Terminal};

use crate::tmux::Session;
use crate::ui::{fuzzy_match, theme};

pub fn run_session_switcher(sessions: Vec<Session>) -> Result<Option<Session>> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, sessions);
    ratatui::restore();
    result
}

struct Model {
    sessions: Vec<Session>,
    filtered_idx: Vec<usize>,
    cursor: usize,
    search: String,
    selected: Option<usize>,
    quit: bool,
}

impl Model {
    fn new(sessions: Vec<Session>) -> Self {
        let filtered_idx = (0..sessions.len()).collect();
        Self {
            sessions,
            filtered_idx,
            cursor: 0,
            search: String::new(),
            selected: None,
            quit: false,
        }
    }

    fn refilter(&mut self) {
        if self.search.is_empty() {
            self.filtered_idx = (0..self.sessions.len()).collect();
        } else {
            self.filtered_idx = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| fuzzy_match(&s.name, &self.search))
                .map(|(i, _)| i)
                .collect();
        }
        if self.cursor >= self.filtered_idx.len() {
            self.cursor = self.filtered_idx.len().saturating_sub(1);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.quit = true;
            }
            KeyCode::Enter if !self.filtered_idx.is_empty() => {
                self.selected = Some(self.filtered_idx[self.cursor]);
                self.quit = true;
            }
            KeyCode::Up => self.move_up(),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => self.move_down(),
            KeyCode::Backspace => {
                self.search.pop();
                self.refilter();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.push(c);
                self.refilter();
            }
            _ => {}
        }
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.cursor + 1 < self.filtered_idx.len() {
            self.cursor += 1;
        }
    }
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    sessions: Vec<Session>,
) -> Result<Option<Session>> {
    let mut model = Model::new(sessions);
    while !model.quit {
        terminal.draw(|frame| render(frame, &model))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            model.handle_key(key);
        }
    }
    Ok(model.selected.map(|i| model.sessions[i].clone()))
}

fn render(frame: &mut Frame, model: &Model) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled("  tmux sessions", theme::title_style())),
        layout[0],
    );

    let search_line = Line::from(vec![
        Span::raw("  "),
        Span::styled("> ", Style::default().fg(theme::VIOLET)),
        Span::styled(&model.search, Style::default().fg(theme::TEXT)),
        Span::styled(
            if model.search.is_empty() {
                "Search sessions..."
            } else {
                ""
            },
            theme::muted_style(),
        ),
    ]);
    frame.render_widget(Paragraph::new(search_line), layout[2]);

    render_list(frame, model, layout[4]);

    let status = format!(
        " {}/{} sessions  ↑↓ navigate  ⏎ select  esc quit",
        model.filtered_idx.len(),
        model.sessions.len()
    );
    frame.render_widget(
        Paragraph::new(Span::styled(status, theme::status_bar_style())),
        layout[5],
    );
}

fn render_list(frame: &mut Frame, model: &Model, area: Rect) {
    if model.filtered_idx.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("  No sessions found", theme::muted_style())),
            area,
        );
        return;
    }

    let max_visible = area.height.max(1) as usize;
    let start = model.cursor.saturating_sub(max_visible.saturating_sub(1));
    let end = (start + max_visible).min(model.filtered_idx.len());

    let lines: Vec<Line> = (start..end)
        .map(|i| session_line(&model.sessions[model.filtered_idx[i]], i == model.cursor))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(Block::default()), area);
}

fn session_line(session: &Session, is_cursor: bool) -> Line<'static> {
    let (cursor_marker, name_style) = if is_cursor {
        (
            Span::styled("▸ ", Style::default().fg(theme::VIOLET)),
            theme::selected_style(),
        )
    } else {
        (Span::raw("  "), theme::normal_style())
    };

    let mut spans = vec![
        cursor_marker,
        Span::styled(session.name.clone(), name_style),
        Span::styled(
            format!(" {} window(s)", session.windows),
            theme::muted_style(),
        ),
    ];

    if session.attached {
        spans.push(Span::styled(" ●", Style::default().fg(theme::GREEN)));
    }
    Line::from(spans)
}
