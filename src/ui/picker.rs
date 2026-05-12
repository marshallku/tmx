use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{Frame, Terminal};

use crate::ui::{fuzzy_match, theme};

pub trait PickerItem: Clone {
    fn fuzzy_key(&self) -> &str;
    fn render(&self, is_cursor: bool) -> Line<'static>;
}

pub struct PickerConfig<'a> {
    pub title: &'a str,
    pub search_placeholder: &'a str,
    pub empty_message: &'a str,
    pub item_noun: &'a str,
}

/// RAII guard so ratatui's terminal is restored even if the loop returns an
/// error via `?`.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> ratatui::DefaultTerminal {
        ratatui::init()
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub fn run_picker<T: PickerItem>(items: Vec<T>, config: PickerConfig<'_>) -> Result<Option<T>> {
    let _guard = TerminalGuard;
    let mut terminal = TerminalGuard::enter();
    run_loop(&mut terminal, items, &config)
}

struct Model<T> {
    items: Vec<T>,
    filtered_idx: Vec<usize>,
    cursor: usize,
    search: String,
    selected: Option<usize>,
    quit: bool,
}

impl<T: PickerItem> Model<T> {
    fn new(items: Vec<T>) -> Self {
        let filtered_idx = (0..items.len()).collect();
        Self {
            items,
            filtered_idx,
            cursor: 0,
            search: String::new(),
            selected: None,
            quit: false,
        }
    }

    fn refilter(&mut self) {
        if self.search.is_empty() {
            self.filtered_idx = (0..self.items.len()).collect();
        } else {
            self.filtered_idx = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| fuzzy_match(it.fuzzy_key(), &self.search))
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

fn run_loop<B: Backend, T: PickerItem>(
    terminal: &mut Terminal<B>,
    items: Vec<T>,
    config: &PickerConfig<'_>,
) -> Result<Option<T>> {
    let mut model = Model::new(items);
    while !model.quit {
        terminal.draw(|frame| render(frame, &model, config))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            model.handle_key(key);
        }
    }
    Ok(model.selected.map(|i| model.items[i].clone()))
}

fn render<T: PickerItem>(frame: &mut Frame, model: &Model<T>, config: &PickerConfig<'_>) {
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

    let title = format!("  {}", config.title);
    frame.render_widget(
        Paragraph::new(Span::styled(title, theme::title_style())),
        layout[0],
    );

    let search_line = Line::from(vec![
        Span::raw("  "),
        Span::styled("> ", Style::default().fg(theme::VIOLET)),
        Span::styled(model.search.clone(), Style::default().fg(theme::TEXT)),
        Span::styled(
            if model.search.is_empty() {
                config.search_placeholder.to_string()
            } else {
                String::new()
            },
            theme::muted_style(),
        ),
    ]);
    frame.render_widget(Paragraph::new(search_line), layout[2]);

    render_list(frame, model, layout[4], config.empty_message);

    let status = format!(
        " {}/{} {}  ↑↓ navigate  ⏎ select  esc quit",
        model.filtered_idx.len(),
        model.items.len(),
        config.item_noun,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(status, theme::status_bar_style())),
        layout[5],
    );
}

fn render_list<T: PickerItem>(
    frame: &mut Frame,
    model: &Model<T>,
    area: Rect,
    empty_message: &str,
) {
    if model.filtered_idx.is_empty() {
        let msg = format!("  {empty_message}");
        frame.render_widget(
            Paragraph::new(Span::styled(msg, theme::muted_style())),
            area,
        );
        return;
    }

    let max_visible = area.height.max(1) as usize;
    let start = model.cursor.saturating_sub(max_visible.saturating_sub(1));
    let end = (start + max_visible).min(model.filtered_idx.len());

    let lines: Vec<Line> = (start..end)
        .map(|i| model.items[model.filtered_idx[i]].render(i == model.cursor))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(Block::default()), area);
}
