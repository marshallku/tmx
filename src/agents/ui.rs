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
use ratatui::widgets::{Block, HighlightSpacing, Paragraph, Row, Table, TableState};
use ratatui::{Frame, Terminal};

use crate::config::Config;
use crate::tmux;
use crate::ui::theme;

use super::attention::{self, AttentionEntry};
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
    match action {
        // Surface switch-pane failures so a misformed target or vanished
        // pane doesn't silently no-op. The caller (cli::run) propagates
        // the error and tmx exits non-zero.
        Some(Action::Switch(target)) => {
            tmux::switch_to_pane(&target).map_err(|e| anyhow::anyhow!("switch to {target}: {e}"))?
        }
        Some(Action::OpenAttentionPicker) => run_helper_script(
            "TMX_ATTENTION_PICKER",
            ".claude/scripts/attention-picker.sh",
        )?,
        Some(Action::JumpLatestAttention) => {
            run_helper_script("TMX_ATTENTION_JUMP", ".claude/scripts/jump-attention.sh")?
        }
        None => {}
    }
    Ok(())
}

/// Resolve the path to one of the attention helper scripts and exec it.
/// Override is via env var (`TMX_ATTENTION_PICKER` / `TMX_ATTENTION_JUMP`),
/// default is `$HOME/<relative>`. We surface a clear error rather than
/// silently no-op so the user notices a misconfigured environment.
fn run_helper_script(env_var: &str, default_rel: &str) -> Result<()> {
    let path = std::env::var(env_var).ok().map(std::path::PathBuf::from);
    let path = match path {
        Some(p) => p,
        None => dirs::home_dir()
            .map(|h| h.join(default_rel))
            .ok_or_else(|| anyhow::anyhow!("cannot resolve $HOME for {default_rel}"))?,
    };
    if !path.exists() {
        anyhow::bail!(
            "attention helper not found at {} (set {env_var} to override)",
            path.display()
        );
    }
    let status = std::process::Command::new("bash").arg(&path).status()?;
    if !status.success() {
        anyhow::bail!("{} exited with status {status}", path.display());
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
    /// Short ephemeral note shown in the footer for one redraw — used to
    /// explain a swallowed keypress (e.g. "no fresh attention to jump").
    /// Cleared on the next state-changing event.
    notice: Option<String>,
    /// Drives ratatui's auto-scroll for the agents table — kept in sync
    /// with `cursor` so the selected row stays visible when the list is
    /// taller than its rendered area (many open tmux panes).
    table_state: TableState,
}

enum Action {
    /// `tmux select-pane + switch-client` to a specific pane target.
    Switch(String),
    /// Hand off to the fzf-based attention picker.
    OpenAttentionPicker,
    /// Hand off to the "jump to newest attention" fast-path.
    JumpLatestAttention,
}

impl Model {
    fn new(snapshot: Snapshot) -> Self {
        let mut table_state = TableState::default();
        if !snapshot.agents.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            snapshot,
            cursor: 0,
            quit: false,
            pending_action: None,
            notice: None,
            table_state,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        // Any real keypress clears the previous notice; specific arms
        // re-set it if they have new feedback to surface.
        self.notice = None;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.quit = true;
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Enter => self.activate_selection(),
            // Mirror the existing `prefix+a` / `prefix+A` tmux bindings:
            // lowercase a = jump to newest attention, uppercase A = full
            // fzf picker over the queue. Both hand off to the bash scripts
            // after raw mode is torn down.
            //
            // 'a' is guarded against the empty-queue case. Without the
            // guard, pressing 'a' with no fresh entries would close the
            // popup with no visible action (the script just exits) —
            // looks identical to a crash. Better to keep the dashboard
            // open and tell the user why nothing happened.
            KeyCode::Char('a') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.snapshot.attention.is_empty() {
                    self.notice = Some("no fresh attention to jump to".to_string());
                } else {
                    self.pending_action = Some(Action::JumpLatestAttention);
                    self.quit = true;
                }
            }
            KeyCode::Char('A') => {
                self.pending_action = Some(Action::OpenAttentionPicker);
                self.quit = true;
            }
            _ => {}
        }
    }

    fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.sync_selection();
    }

    fn move_down(&mut self) {
        if self.cursor + 1 < self.snapshot.agents.len() {
            self.cursor += 1;
            self.sync_selection();
        }
    }

    fn sync_selection(&mut self) {
        if self.snapshot.agents.is_empty() {
            self.table_state.select(None);
        } else {
            self.table_state.select(Some(self.cursor));
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
        self.sync_selection();
    }
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>) -> Result<Option<Action>> {
    let agents_cfg = Config::load().agents;
    let mut proc = ProcSnapshot::new();
    let mut model = Model::new(collector::collect(&proc, &agents_cfg));
    let mut last_tick = Instant::now();
    let mut last_rendered: Option<RenderKey> = None;

    while !model.quit {
        let key_now = RenderKey::from(&model);
        if last_rendered.as_ref() != Some(&key_now) {
            terminal.draw(|frame| render(frame, &mut model))?;
            last_rendered = Some(key_now);
        }

        if event::poll(POLL)?
            && let Event::Key(key) = event::read()?
        {
            model.handle_key(key);
        }

        if last_tick.elapsed() >= TICK {
            proc.refresh();
            let next = collector::collect(&proc, &agents_cfg);
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
    /// `(ts, kind, source, tmux_session, body)` per attention entry —
    /// the visible projection of the queue.
    attention: Vec<(i64, String, String, String, String)>,
    /// Coarse minute bucket so relative-age strings (`2m`, `1h`) tick
    /// over even when no underlying snapshot field changes. Without
    /// this, an attention entry's age display freezes at first paint.
    minute_bucket: i64,
    notice: Option<String>,
}

#[derive(PartialEq, Eq)]
struct RenderRow {
    pane_target: Option<String>,
    kind: AgentKind,
    command: String,
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
                command: a.command.clone(),
                status: a.status,
                repo: a.repo_name.clone(),
                has_intent: a.flags.has_intent,
                reviewed_fresh: a.flags.reviewed_fresh,
                extra: a.extra.clone(),
            })
            .collect();
        let attention = model
            .snapshot
            .attention
            .iter()
            .map(|a| {
                (
                    a.ts,
                    a.kind.clone(),
                    a.source.clone(),
                    a.tmux_session.clone(),
                    a.body.clone(),
                )
            })
            .collect();
        let minute_bucket = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_secs() / 60) as i64)
            .unwrap_or(0);
        Self {
            cursor: model.cursor,
            blocked: model.snapshot.global_blocked,
            panes_error: model.snapshot.panes_error.clone(),
            rows,
            attention,
            minute_bucket,
            notice: model.notice.clone(),
        }
    }
}

/// Lower bound on the queue panel — header + ≥3 entry rows so a non-empty
/// queue never collapses into a single header line on tiny popups.
const ATTENTION_PANEL_MIN_ROWS: u16 = 4;

fn render(frame: &mut Frame, model: &mut Model) {
    let area = frame.area();
    let attention = model.snapshot.attention.as_slice();
    // Let the queue grow up to half the popup so a long agent list can't
    // push its bottom entries off-screen. Surplus entries beyond what
    // fits are surfaced via a "+N more" indicator inside the panel.
    let max_attention_rows = (area.height / 2).max(ATTENTION_PANEL_MIN_ROWS);
    let attention_height = if attention.is_empty() {
        0
    } else {
        // +1 for the header row.
        ((attention.len() as u16) + 1).min(max_attention_rows)
    };

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                // title
            Constraint::Length(1),                // banner
            Constraint::Min(3),                   // agents table
            Constraint::Length(attention_height), // attention queue (0 = hidden)
            Constraint::Length(1),                // summary
            Constraint::Length(1),                // keys
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled("  tmx agents", theme::title_style())),
        layout[0],
    );

    let header = Row::new(vec![
        Span::styled("STATUS", theme::muted_style()),
        Span::styled("KIND", theme::muted_style()),
        Span::styled("SESSION", theme::muted_style()),
        Span::styled("REPO", theme::muted_style()),
        Span::styled("FLAGS", theme::muted_style()),
        Span::styled("INFO", theme::muted_style()),
    ]);

    let rows = model.snapshot.agents.iter().map(build_row);

    let widths = [
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(28),
        Constraint::Length(20),
        Constraint::Length(20),
        Constraint::Min(8),
    ];

    // `TableState` selection drives both the visible highlight and the
    // auto-scroll offset — ratatui shifts `offset` whenever the selected
    // row would fall outside the viewport, so j past the bottom scrolls.
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default())
        .highlight_symbol("▶ ")
        .highlight_spacing(HighlightSpacing::Always)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(table, layout[2], &mut model.table_state);

    if attention_height > 0 {
        render_attention_panel(frame, layout[3], attention);
    }

    let summary = build_summary(&model.snapshot);
    frame.render_widget(
        Paragraph::new(Span::styled(summary, theme::status_bar_style())),
        layout[4],
    );

    let (keys_text, keys_style) = match model.notice.as_deref() {
        Some(msg) => (
            format!(" ⓘ  {}", sanitize(msg)),
            Style::default()
                .fg(theme::palette().yellow)
                .add_modifier(Modifier::BOLD),
        ),
        None => (
            " j/k nav  enter switch  a jump  A picker  q quit".to_string(),
            theme::muted_style(),
        ),
    };
    frame.render_widget(
        Paragraph::new(Span::styled(keys_text, keys_style)),
        layout[5],
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
            Style::default().fg(theme::palette().red),
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

fn render_attention_panel(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    entries: &[AttentionEntry],
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let header = Line::from(Span::styled(
        format!(" attention queue ({} pending)", entries.len()),
        theme::status_bar_style(),
    ));

    // Reserve 1 row for the header; if we'd still truncate after that,
    // reserve another row for the "+N more" hint so the user knows the
    // queue continues offscreen instead of silently swallowing the tail.
    let entry_capacity = (area.height as usize).saturating_sub(1);
    let truncated = entries.len() > entry_capacity;
    let visible_count = if truncated {
        entry_capacity.saturating_sub(1)
    } else {
        entries.len()
    };

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible_count + 2);
    lines.push(header);
    for entry in entries.iter().take(visible_count) {
        let age = attention::human_age(entry.ts, now);
        let session_cell = if entry.tmux_session.is_empty() {
            "—".to_string()
        } else {
            entry.tmux_session.clone()
        };
        let kind_label = format!("{}·{}", entry.source, short_kind(&entry.kind));
        // Body fills remaining columns; truncate to fit width minus the
        // fixed-width prefix columns.
        let prefix_width = 4 + 1 + 14 + 1 + 18 + 2; // age + session + kind + padding
        let body_max = (area.width as usize).saturating_sub(prefix_width);
        let body = truncate_for_width(&sanitize(&entry.body), body_max);
        lines.push(Line::from(vec![
            Span::styled(format!(" {:>4}  ", age), theme::muted_style()),
            Span::styled(pad(&sanitize(&session_cell), 14), theme::normal_style()),
            Span::styled(pad(&sanitize(&kind_label), 18), theme::branch_style()),
            Span::raw("  "),
            Span::styled(body, theme::normal_style()),
        ]));
    }
    if truncated {
        let hidden = entries.len() - visible_count;
        lines.push(Line::from(Span::styled(
            format!(" … +{hidden} more — press A for full picker"),
            theme::muted_style(),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Render an attention-queue `kind` value compactly. Known kinds get
/// hand-picked short aliases (`notif`/`stop`/`turn`); anything else falls
/// through so a future `notify-*.sh` hook adding a new kind still surfaces
/// meaningfully rather than collapsing to a generic placeholder.
fn short_kind(kind: &str) -> String {
    match kind {
        "notification" => "notif".to_string(),
        "stop" => "stop".to_string(),
        "codex-turn" => "turn".to_string(),
        "" => "?".to_string(),
        other => other.chars().take(8).collect(),
    }
}

fn pad(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width {
        s.chars().take(width).collect()
    } else {
        let mut out = s.to_string();
        out.extend(std::iter::repeat_n(' ', width - n));
        out
    }
}

fn build_row(agent: &Agent) -> Row<'static> {
    let status_style = match agent.status {
        Status::Working => Style::default().fg(theme::palette().green),
        Status::Ready => Style::default().fg(theme::palette().blue),
        Status::AwaitingDecision => Style::default()
            .fg(theme::palette().yellow)
            .add_modifier(Modifier::BOLD),
        Status::Idle => theme::muted_style(),
        Status::Background => Style::default().fg(theme::palette().purple),
    };
    let status_cell = Line::from(vec![
        Span::styled(agent.status.glyph().to_string(), status_style),
        Span::raw(" "),
        Span::styled(agent.status.label().to_string(), theme::muted_style()),
    ]);

    let kind_style = match agent.kind {
        AgentKind::Claude => Style::default().fg(theme::palette().orange),
        AgentKind::Codex => Style::default().fg(theme::palette().blue),
        AgentKind::Custom => Style::default().fg(theme::palette().purple),
        AgentKind::Shell => theme::muted_style(),
        AgentKind::Other => theme::muted_style(),
    };
    // Custom agents show their actual name (the enum only says "custom").
    let kind_label = match agent.kind {
        AgentKind::Custom => agent.command.as_str(),
        kind => kind.label(),
    };
    let kind_cell = Line::from(Span::styled(sanitize(kind_label), kind_style));

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

    Row::new(vec![
        status_cell,
        kind_cell,
        session_cell,
        repo_cell,
        flags_cell,
        info_cell,
    ])
}

fn build_flag_line(agent: &Agent) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if agent.flags.has_intent {
        spans.push(Span::styled(
            "intent ",
            Style::default().fg(theme::palette().violet),
        ));
    }
    if agent.flags.reviewed_fresh {
        spans.push(Span::styled(
            "reviewed ",
            Style::default().fg(theme::palette().green),
        ));
    }
    if spans.is_empty() {
        spans.push(Span::styled("—", theme::muted_style()));
    }
    Line::from(spans)
}

fn build_summary(snap: &Snapshot) -> String {
    let total = snap.agents.len();
    let decisions = snap
        .agents
        .iter()
        .filter(|a| a.status == Status::AwaitingDecision)
        .count();
    let working = snap
        .agents
        .iter()
        .filter(|a| a.status == Status::Working)
        .count();
    let ready = snap
        .agents
        .iter()
        .filter(|a| a.status == Status::Ready)
        .count();
    let bg = snap
        .agents
        .iter()
        .filter(|a| a.status == Status::Background)
        .count();
    let attention = snap.attention.len();
    format!(
        " rows: {total} • working: {working} • ready: {ready} • decision: {decisions} • bg: {bg} • attention: {attention} • blocked: {}",
        snap.global_blocked,
    )
}

/// Truncate to `max` *char* count, appending `…` when cut.
fn truncate_for_width(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let mut out: String = chars[..max.saturating_sub(1)].iter().collect();
    out.push('…');
    out
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
            command: kind.label().to_string(),
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
    fn build_summary_counts_states() {
        let snap = Snapshot {
            agents: vec![
                agent(AgentKind::Claude, Status::Working, "a"),
                agent(AgentKind::Claude, Status::Ready, "b"),
                agent(AgentKind::Claude, Status::AwaitingDecision, "c"),
                agent(AgentKind::Codex, Status::Background, "d"),
            ],
            captured_at: std::time::SystemTime::now(),
            global_blocked: 2,
            panes_error: None,
            attention: Vec::new(),
            codex_jobs: Vec::new(),
        };
        let s = build_summary(&snap);
        assert!(s.contains("rows: 4"));
        assert!(s.contains("working: 1"));
        assert!(s.contains("ready: 1"));
        assert!(s.contains("decision: 1"));
        assert!(s.contains("bg: 1"));
        assert!(s.contains("attention: 0"));
        assert!(s.contains("blocked: 2"));
    }

    #[test]
    fn build_summary_reports_attention_count() {
        let snap = Snapshot {
            agents: Vec::new(),
            captured_at: std::time::SystemTime::now(),
            global_blocked: 0,
            panes_error: None,
            attention: vec![
                AttentionEntry {
                    ts: 1,
                    kind: "stop".into(),
                    source: "claude".into(),
                    title: String::new(),
                    body: "b".into(),
                    session_id: String::new(),
                    cwd: PathBuf::new(),
                    tmux_target: String::new(),
                    tmux_session: String::new(),
                },
                AttentionEntry {
                    ts: 2,
                    kind: "notification".into(),
                    source: "claude".into(),
                    title: String::new(),
                    body: "b".into(),
                    session_id: String::new(),
                    cwd: PathBuf::new(),
                    tmux_target: String::new(),
                    tmux_session: String::new(),
                },
            ],
            codex_jobs: Vec::new(),
        };
        let s = build_summary(&snap);
        assert!(s.contains("attention: 2"));
    }

    #[test]
    fn truncate_for_width_basic() {
        assert_eq!(truncate_for_width("hello", 10), "hello");
        assert_eq!(truncate_for_width("hello world", 5), "hell…");
        assert_eq!(truncate_for_width("a", 0), "");
    }

    #[test]
    fn pad_extends_or_truncates() {
        assert_eq!(pad("abc", 5), "abc  ");
        assert_eq!(pad("abcdef", 4), "abcd");
        assert_eq!(pad("", 3), "   ");
    }
}
