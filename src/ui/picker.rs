use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::{Backend, CrosstermBackend};
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

/// Shared leading marker + name style for picker rows: `▸ ` with the
/// selected style under the cursor, plain padding otherwise. Every
/// `PickerItem::render` impl starts a row with this pair.
pub fn cursor_prefix(is_cursor: bool) -> (Span<'static>, Style) {
    if is_cursor {
        (
            Span::styled("▸ ", Style::default().fg(theme::palette().violet)),
            theme::selected_style(),
        )
    } else {
        (Span::raw("  "), theme::normal_style())
    }
}

pub struct PickerConfig<'a> {
    pub title: &'a str,
    pub search_placeholder: &'a str,
    pub empty_message: &'a str,
    pub item_noun: &'a str,
}

/// RAII guard that restores stderr's terminal state (raw mode + alternate
/// screen) even if the loop returns an error or panics partway through setup.
struct StderrGuard;

impl Drop for StderrGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Run the picker with stderr as the TUI backend. Stdout stays untouched so
/// callers like `tmx worktree list` can print the selection on stdout for
/// `$(...)` capture. Also avoids a macOS-specific freeze observed when
/// rendering to stdout inside a tmux popup.
pub fn run_picker<T: PickerItem>(items: Vec<T>, config: PickerConfig<'_>) -> Result<Option<T>> {
    enable_raw_mode()?;
    // Install the guard *before* anything that can fail so raw mode is
    // disabled if `EnterAlternateScreen` or terminal construction errors out.
    let _guard = StderrGuard;
    execute!(io::stderr(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stderr());
    let mut terminal = Terminal::new(backend)?;
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
        Span::styled("> ", Style::default().fg(theme::palette().violet)),
        Span::styled(
            model.search.clone(),
            Style::default().fg(theme::palette().text),
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

    impl PickerItem for String {
        fn fuzzy_key(&self) -> &str {
            self
        }

        fn render(&self, _is_cursor: bool) -> Line<'static> {
            Line::from(self.clone())
        }
    }

    fn items(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(model: &mut Model<String>, s: &str) {
        for c in s.chars() {
            model.handle_key(press(KeyCode::Char(c)));
        }
    }

    #[test]
    fn refilter_narrows_and_restores() {
        let mut m = Model::new(items(&["alpha", "beta", "albatross"]));
        type_str(&mut m, "al");
        assert_eq!(m.filtered_idx, vec![0, 2]);
        m.handle_key(press(KeyCode::Backspace));
        m.handle_key(press(KeyCode::Backspace));
        assert_eq!(m.filtered_idx, vec![0, 1, 2]);
    }

    #[test]
    fn cursor_clamps_when_filter_shrinks_list() {
        let mut m = Model::new(items(&["aa", "ab", "zz"]));
        m.handle_key(press(KeyCode::Down));
        m.handle_key(press(KeyCode::Down));
        assert_eq!(m.cursor, 2);
        type_str(&mut m, "a");
        assert_eq!(m.filtered_idx.len(), 2);
        assert_eq!(m.cursor, 1);
    }

    #[test]
    fn cursor_does_not_move_past_bounds() {
        let mut m = Model::new(items(&["one", "two"]));
        m.handle_key(press(KeyCode::Up));
        assert_eq!(m.cursor, 0);
        m.handle_key(press(KeyCode::Down));
        m.handle_key(press(KeyCode::Down));
        m.handle_key(press(KeyCode::Down));
        assert_eq!(m.cursor, 1);
    }

    #[test]
    fn enter_selects_filtered_item_by_original_index() {
        let mut m = Model::new(items(&["alpha", "beta", "albatross"]));
        type_str(&mut m, "bat");
        m.handle_key(press(KeyCode::Enter));
        assert!(m.quit);
        assert_eq!(m.selected, Some(2));
    }

    #[test]
    fn enter_on_empty_filter_selects_nothing() {
        let mut m = Model::new(items(&["alpha"]));
        type_str(&mut m, "zzz");
        m.handle_key(press(KeyCode::Enter));
        assert_eq!(m.selected, None);
        assert!(!m.quit);
    }

    #[test]
    fn esc_and_ctrl_c_quit_without_selection() {
        let mut m = Model::new(items(&["alpha"]));
        m.handle_key(press(KeyCode::Esc));
        assert!(m.quit);
        assert_eq!(m.selected, None);

        let mut m = Model::new(items(&["alpha"]));
        m.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(m.quit);
        assert_eq!(m.selected, None);
    }
}
