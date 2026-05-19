//! Agent dashboard — shows which Claude/Codex agents are running in which
//! tmux panes, with cwd, task, and review/intent status.
//!
//! Data sources are all local: tmux itself + `~/.claude/state/`. No daemon.
//! Identity anchor is the tmux pane PID; cwd is for state-marker matching
//! and display, never for identity. See `state.rs` for the matching logic.

pub mod classify;
pub mod collector;
pub mod panes;
pub mod proc;
pub mod session_meta;
pub mod state;
pub mod ui;

use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Agent {
    /// Stable identity used as the cursor anchor across snapshot refreshes.
    /// `pane:<target>` for tmux panes, `codex:<job_id>` for background jobs.
    pub id: String,
    pub pane: Option<PaneLocator>,
    pub kind: AgentKind,
    pub status: Status,
    pub cwd: PathBuf,
    pub repo_name: String,
    pub flags: Flags,
    pub extra: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneLocator {
    pub session: String,
    pub window: u32,
    pub pane: u32,
    pub pane_pid: u32,
}

impl PaneLocator {
    pub fn target(&self) -> String {
        format!("{}:{}.{}", self.session, self.window, self.pane)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    Shell,
    Other,
}

impl AgentKind {
    pub fn from_command(cmd: &str) -> Self {
        match cmd {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "zsh" | "bash" | "fish" | "sh" => Self::Shell,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Shell => "shell",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Claude/Codex is mid-turn (running tools or composing). No input
    /// from the user is needed.
    Working,
    /// Claude/Codex is parked at the chat prompt. User can type a new
    /// request whenever.
    Ready,
    /// Claude/Codex is blocking on a selection/permission dialog and
    /// will not progress until the user acts.
    AwaitingDecision,
    /// Pane has no recognised Claude/Codex UI (plain shell, nvim, etc.).
    Idle,
    /// Codex-companion background job (no associated tmux pane).
    Background,
}

impl Status {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Working => "●",
            Self::Ready => "◐",
            Self::AwaitingDecision => "⚠",
            Self::Idle => "○",
            Self::Background => "▷",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Ready => "ready",
            Self::AwaitingDecision => "decision",
            Self::Idle => "idle",
            Self::Background => "bg",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    pub has_intent: bool,
    pub blocked: bool,
    pub reviewed_fresh: bool,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub agents: Vec<Agent>,
    pub captured_at: SystemTime,
    /// Total blocked sessions in `~/.claude/state/` — not always pane-mappable.
    pub global_blocked: usize,
    /// Non-fatal error from the most recent `tmux list-panes` call, if any.
    /// Surfaced as a banner so the user doesn't mistake "tmux is broken" for
    /// "nothing is running."
    pub panes_error: Option<String>,
}

impl Snapshot {
    pub fn empty() -> Self {
        Self {
            agents: Vec::new(),
            captured_at: SystemTime::now(),
            global_blocked: 0,
            panes_error: None,
        }
    }
}

pub fn run() -> anyhow::Result<()> {
    ui::run()
}
