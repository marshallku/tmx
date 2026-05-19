//! Agent dashboard TUI. Renders to stderr so it composes with shell wrappers
//! that capture stdout (matches the picker convention in `src/ui/picker.rs`).
//!
//! Resource discipline:
//!   - tick = 1s; key poll = 200ms so input stays snappy without 5 wakes/s
//!   - `ProcSnapshot` is reused across ticks; only PID-tree fields refreshed
//!   - redraw only when the visible snapshot content changes (not every tick)
//!   - all user-displayed strings are stripped of control chars before render

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};

use crate::tmux;
use crate::ui::theme;

use super::collector;
use super::proc::ProcSnapshot;
use super::{Agent, AgentKind, Snapshot, Status};

const TICK: Duration = Duration::from_millis(1000);
const POLL: Duration = Duration::from_millis(200);

struct StderrGuard;

impl Drop for StderrGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

pub fn run() -> Result<()> {
    let action = {
        enable_raw_mode()?;
        let _guard = StderrGuard;
        execute!(io::stderr(), EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(io::stderr());
        let mut terminal = Terminal::new(backend)?;
        run_loop(&mut terminal)?
        // _guard drops here → alt-screen left, raw mode off.
    };
    if let Some(Action::Switch(target)) = action {
        // Surface failures so a misformed target or vanished pane doesn't
        // silently no-op. The caller (cli::run) propagates the error and
        // tmx exits non-zero.
        tmux::switch_to_pane(&target).map_err(|e| anyhow::anyhow!("switch to {target}: {e}"))?;
    }
    Ok(())
}

struct Model {
    snapshot: Snapshot,
    cursor: usize,
    quit: bool,
    /// Action requested by the last keypress that the outer loop should
    /// service after raw-mode is torn down (e.g. tmux switch-client).
    pending_action: Option<Action>,
}

enum Action {
    Switch(String),
}

impl Model {
    fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            cursor: 0,
            quit: false,
            pending_action: None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.quit = true;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Enter => self.activate_selection(),
            _ => {}
        }
    }

    fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.cursor + 1 < self.snapshot.agents.len() {
            self.cursor += 1;
        }
    }

    fn activate_selection(&mut self) {
        let Some(agent) = self.snapshot.agents.get(self.cursor) else {
            return;
        };
        let Some(pane) = agent.pane.as_ref() else {
            return; // codex background jobs have no tmux target
        };
        self.pending_action = Some(Action::Switch(pane.target()));
        self.quit = true;
    }

    /// Replace the snapshot while keeping the cursor on the same agent if
    /// possible (matched by stable `Agent::id`). Works for both tmux pane
    /// rows and codex background rows. Falls back to clamping.
    fn replace_snapshot(&mut self, next: Snapshot) {
        let anchor = self.snapshot.agents.get(self.cursor).map(|a| a.id.clone());
        self.snapshot = next;
        self.cursor = match anchor {
            Some(id) => self
                .snapshot
                .agents
                .iter()
                .position(|a| a.id == id)
                .unwrap_or(0),
            None => 0,
        };
        if self.cursor >= self.snapshot.agents.len() {
            self.cursor = self.snapshot.agents.len().saturating_sub(1);
        }
    }
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>) -> Result<Option<Action>> {
    let mut proc = ProcSnapshot::new();
    let mut model = Model::new(collector::collect(&proc));
    let mut last_tick = Instant::now();
    let mut last_rendered: Option<RenderKey> = None;

    while !model.quit {
        let key_now = RenderKey::from(&model);
        if last_rendered.as_ref() != Some(&key_now) {
            terminal.draw(|frame| render(frame, &model))?;
            last_rendered = Some(key_now);
        }

        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
        {
            model.handle_key(key);
        }

        if last_tick.elapsed() >= TICK {
            proc.refresh();
            let next = collector::collect(&proc);
            model.replace_snapshot(next);
            last_tick = Instant::now();
        }
    }

    Ok(model.pending_action)
}

/// Cheap equality key. Comparing the full `Snapshot` would re-trigger on
/// every tick because `captured_at` always advances; this restricts to the
/// fields that actually affect what the user sees.
#[derive(PartialEq, Eq)]
struct RenderKey {
    cursor: usize,
    blocked: usize,
    panes_error: Option<String>,
    rows: Vec<RenderRow>,
}

#[derive(PartialEq, Eq)]
struct RenderRow {
    pane_target: Option<String>,
    kind: AgentKind,
    status: Status,
    repo: String,
    has_intent: bool,
    reviewed_fresh: bool,
    extra: String,
}

impl From<&Model> for RenderKey {
    fn from(model: &Model) -> Self {
        let rows = model
            .snapshot
            .agents
            .iter()
            .map(|a| RenderRow {
                pane_target: a.pane.as_ref().map(|p| p.target()),
                kind: a.kind,
                status: a.status,
                repo: a.repo_name.clone(),
                has_intent: a.flags.has_intent,
                reviewed_fresh: a.flags.reviewed_fresh,
                extra: a.extra.clone(),
            })
            .collect();
        Self {
            cursor: model.cursor,
            blocked: model.snapshot.global_blocked,
            panes_error: model.snapshot.panes_error.clone(),
            rows,
        }
    }
}

fn render(frame: &mut Frame, model: &Model) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled("  tmx agents", theme::title_style())),
        layout[0],
    );

    let header = Row::new(vec![
        Span::styled("  STATUS", theme::muted_style()),
        Span::styled("KIND", theme::muted_style()),
        Span::styled("SESSION", theme::muted_style()),
        Span::styled("REPO", theme::muted_style()),
        Span::styled("FLAGS", theme::muted_style()),
        Span::styled("INFO", theme::muted_style()),
    ]);

    let rows = model
        .snapshot
        .agents
        .iter()
        .enumerate()
        .map(|(i, agent)| build_row(agent, i == model.cursor));

    let widths = [
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(28),
        Constraint::Length(20),
        Constraint::Length(20),
        Constraint::Min(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default());
    frame.render_widget(table, layout[2]);

    let summary = build_summary(&model.snapshot);
    frame.render_widget(
        Paragraph::new(Span::styled(summary, theme::status_bar_style())),
        layout[3],
    );

    frame.render_widget(
        Paragraph::new(Span::styled(
            " j/k nav  enter switch  q quit",
            theme::muted_style(),
        )),
        layout[4],
    );

    // Banner line: prefer surfacing a tmux failure over the bare "empty"
    // state so the user doesn't mistake a misconfigured tmux for a quiet
    // workstation. Falls back to the empty hint, or stays blank.
    let banner = match (
        &model.snapshot.panes_error,
        model.snapshot.agents.is_empty(),
    ) {
        (Some(err), _) => Some((
            format!("  tmux unavailable: {}", sanitize(err)),
            Style::default().fg(theme::RED),
        )),
        (None, true) => Some((
            "  no tmux panes or codex jobs found".to_string(),
            theme::muted_style(),
        )),
        _ => None,
    };
    if let Some((text, style)) = banner {
        frame.render_widget(Paragraph::new(Span::styled(text, style)), layout[1]);
    }
}

fn build_row(agent: &Agent, is_cursor: bool) -> Row<'static> {
    let cursor_marker = if is_cursor { "▶ " } else { "  " };
    let status_style = match agent.status {
        Status::Running => Style::default().fg(theme::GREEN),
        Status::Idle => theme::muted_style(),
        Status::Background => Style::default().fg(theme::BLUE),
    };
    let status_cell = Line::from(vec![
        Span::raw(cursor_marker),
        Span::styled(agent.status.glyph().to_string(), status_style),
        Span::raw(" "),
        Span::styled(agent.status.label().to_string(), theme::muted_style()),
    ]);

    let kind_style = match agent.kind {
        AgentKind::Claude => Style::default().fg(theme::ORANGE),
        AgentKind::Codex => Style::default().fg(theme::BLUE),
        AgentKind::Shell => theme::muted_style(),
        AgentKind::Other => theme::muted_style(),
    };
    let kind_cell = Line::from(Span::styled(sanitize(agent.kind.label()), kind_style));

    let session_cell = match &agent.pane {
        Some(p) => Line::from(vec![
            Span::styled(sanitize(&p.session), theme::normal_style()),
            Span::styled(format!(" {}.{}", p.window, p.pane), theme::muted_style()),
        ]),
        None => Line::from(Span::styled("—", theme::muted_style())),
    };

    let repo_cell = Line::from(Span::styled(
        sanitize(&agent.repo_name),
        theme::branch_style(),
    ));

    let flags_cell = build_flag_line(agent);

    let info_cell = Line::from(Span::styled(sanitize(&agent.extra), theme::muted_style()));

    let mut row = Row::new(vec![
        status_cell,
        kind_cell,
        session_cell,
        repo_cell,
        flags_cell,
        info_cell,
    ]);
    if is_cursor {
        row = row.style(Style::default().add_modifier(Modifier::BOLD));
    }
    row
}

fn build_flag_line(agent: &Agent) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if agent.flags.has_intent {
        spans.push(Span::styled("intent ", Style::default().fg(theme::VIOLET)));
    }
    if agent.flags.reviewed_fresh {
        spans.push(Span::styled("reviewed ", Style::default().fg(theme::GREEN)));
    }
    if spans.is_empty() {
        spans.push(Span::styled("—", theme::muted_style()));
    }
    Line::from(spans)
}

fn build_summary(snap: &Snapshot) -> String {
    let total = snap.agents.len();
    let claude = snap
        .agents
        .iter()
        .filter(|a| a.kind == AgentKind::Claude)
        .count();
    let codex = snap
        .agents
        .iter()
        .filter(|a| a.kind == AgentKind::Codex && a.status != Status::Background)
        .count();
    let bg = snap
        .agents
        .iter()
        .filter(|a| a.status == Status::Background)
        .count();
    format!(
        " rows: {total} • claude: {claude} • codex: {codex} • bg: {bg} • blocked: {}",
        snap.global_blocked,
    )
}

/// Strip ANSI/control chars so a hostile path or repo name can't inject
/// escape sequences via the TUI. ratatui passes spans to the backend as
/// raw bytes; without this a `\x1b[2J` in a process cwd would clear the
/// screen mid-render.
fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && *c != '\x7f')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Flags, PaneLocator};
    use std::path::PathBuf;

    fn agent(kind: AgentKind, status: Status, repo: &str) -> Agent {
        Agent {
            id: format!("pane:s:0.0-{repo}"),
            pane: Some(PaneLocator {
                session: "s".into(),
                window: 0,
                pane: 0,
                pane_pid: 1,
            }),
            kind,
            status,
            cwd: PathBuf::from("/x"),
            repo_name: repo.into(),
            flags: Flags::default(),
            extra: String::new(),
        }
    }

    #[test]
    fn sanitize_strips_escape_sequences() {
        // Stripping the leading ESC + BEL is sufficient: `[2J` without a
        // preceding ESC is inert text that the terminal won't interpret.
        let dirty = "evil\x1b[2J\x07path";
        assert_eq!(sanitize(dirty), "evil[2Jpath");
    }

    #[test]
    fn sanitize_strips_all_control_chars() {
        // Pure control payloads collapse to empty.
        assert_eq!(sanitize("\x1b\x07\x00\x7f"), "");
    }

    #[test]
    fn sanitize_preserves_normal_chars() {
        assert_eq!(sanitize("hello-world_123 ./"), "hello-world_123 ./");
    }

    #[test]
    fn build_summary_counts_kinds() {
        let snap = Snapshot {
            agents: vec![
                agent(AgentKind::Claude, Status::Running, "a"),
                agent(AgentKind::Claude, Status::Running, "b"),
                agent(AgentKind::Codex, Status::Background, "c"),
            ],
            captured_at: std::time::SystemTime::now(),
            global_blocked: 2,
            panes_error: None,
        };
        let s = build_summary(&snap);
        assert!(s.contains("rows: 3"));
        assert!(s.contains("claude: 2"));
        assert!(s.contains("bg: 1"));
        assert!(s.contains("blocked: 2"));
    }
}
