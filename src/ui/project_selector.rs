use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{Frame, Terminal};

use crate::project::Project;
use crate::tmux;
use crate::ui::{fuzzy_match, theme};

pub fn run_project_selector(projects: Vec<Project>) -> Result<Option<Project>> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, projects);
    ratatui::restore();
    result
}

pub fn open_project(project: &Project) -> Result<()> {
    let session_name = project.name.replace('.', "_");
    if tmux::session_exists(&session_name) {
        return tmux::switch_session(&session_name).context("switch tmux session");
    }
    let path_str = project.path.to_string_lossy();
    tmux::create_session(&session_name, &path_str).context("create tmux session")?;
    tmux::apply_layout(&session_name, project.project_type.as_str()).ok();
    tmux::switch_session(&session_name).context("switch tmux session")
}

struct Model {
    projects: Vec<Project>,
    filtered_idx: Vec<usize>,
    cursor: usize,
    search: String,
    selected: Option<usize>,
    quit: bool,
}

impl Model {
    fn new(projects: Vec<Project>) -> Self {
        let filtered_idx = (0..projects.len()).collect();
        Self {
            projects,
            filtered_idx,
            cursor: 0,
            search: String::new(),
            selected: None,
            quit: false,
        }
    }

    fn refilter(&mut self) {
        if self.search.is_empty() {
            self.filtered_idx = (0..self.projects.len()).collect();
        } else {
            self.filtered_idx = self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, p)| fuzzy_match(&p.name, &self.search))
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
    projects: Vec<Project>,
) -> Result<Option<Project>> {
    let mut model = Model::new(projects);

    while !model.quit {
        terminal.draw(|frame| render(frame, &model))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            model.handle_key(key);
        }
    }

    Ok(model.selected.map(|i| model.projects[i].clone()))
}

fn render(frame: &mut Frame, model: &Model) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // padding
            Constraint::Length(1), // search input
            Constraint::Length(1), // padding
            Constraint::Min(1),    // list
            Constraint::Length(1), // status
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled("  tmux-powertools", theme::title_style())),
        layout[0],
    );

    let search_line = Line::from(vec![
        Span::raw("  "),
        Span::styled("> ", Style::default().fg(theme::VIOLET)),
        Span::styled(&model.search, Style::default().fg(theme::TEXT)),
        Span::styled(
            if model.search.is_empty() {
                "Search projects..."
            } else {
                ""
            },
            theme::muted_style(),
        ),
    ]);
    frame.render_widget(Paragraph::new(search_line), layout[2]);

    render_list(frame, model, layout[4]);

    let status = format!(
        " {}/{} projects  ↑↓ navigate  ⏎ select  esc quit",
        model.filtered_idx.len(),
        model.projects.len()
    );
    frame.render_widget(
        Paragraph::new(Span::styled(status, theme::status_bar_style())),
        layout[5],
    );
}

fn render_list(frame: &mut Frame, model: &Model, area: Rect) {
    if model.filtered_idx.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("  No projects found", theme::muted_style())),
            area,
        );
        return;
    }

    let max_visible = area.height.max(1) as usize;
    let start = model.cursor.saturating_sub(max_visible.saturating_sub(1));
    let end = (start + max_visible).min(model.filtered_idx.len());

    let lines: Vec<Line> = (start..end)
        .map(|i| {
            let project = &model.projects[model.filtered_idx[i]];
            let is_cursor = i == model.cursor;
            project_line(project, is_cursor)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines).block(Block::default()), area);
}

fn project_line(project: &Project, is_cursor: bool) -> Line<'static> {
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
        Span::styled(project.name.clone(), name_style),
        Span::raw(" "),
        Span::styled(
            format!("[{}]", project.project_type.as_str()),
            theme::type_style(project.project_type),
        ),
        Span::raw(" "),
        Span::styled(project.git_branch.clone(), theme::branch_style()),
    ];

    if project.git_dirty {
        spans.push(Span::styled(" ●", theme::dirty_style()));
    }
    if project.git_ahead > 0 {
        spans.push(Span::styled(
            format!(" ↑{}", project.git_ahead),
            Style::default().fg(theme::GREEN),
        ));
    }
    if project.git_behind > 0 {
        spans.push(Span::styled(
            format!(" ↓{}", project.git_behind),
            Style::default().fg(theme::RED),
        ));
    }

    Line::from(spans)
}

// Allow callers that don't need a TUI (e.g. dumb terminals) to see why we bail.
#[allow(dead_code)]
fn require_tty() -> io::Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return Err(io::Error::other(
            "stdout is not a TTY; refusing to start TUI",
        ));
    }
    Ok(())
}
