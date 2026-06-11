//! Agent dashboard — shows which Claude/Codex agents are running in which
//! tmux panes, with cwd, task, and review/intent status.
//!
//! Data sources are all local: tmux itself + `~/.claude/state/`. No daemon.
//! Identity anchor is the tmux pane PID; cwd is for state-marker matching
//! and display, never for identity. See `state.rs` for the matching logic.

pub mod attention;
pub mod classify;
pub mod collector;
pub mod panes;
pub mod proc;
pub mod session_meta;
pub mod state;
pub mod ui;

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    /// Stable identity used as the cursor anchor across snapshot refreshes.
    /// `pane:<target>` for tmux panes, `codex:<job_id>` for background jobs.
    pub id: String,
    pub pane: Option<PaneLocator>,
    pub kind: AgentKind,
    /// The process name behind this row (agent binary, shell, or whatever
    /// the pane runs). For `Custom` agents this is the display name.
    pub command: String,
    pub status: Status,
    pub cwd: PathBuf,
    pub repo_name: String,
    pub flags: Flags,
    pub extra: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Claude,
    Codex,
    /// A user-configured agent (`[agents] extra_agents` in config). The
    /// concrete name lives in [`Agent::command`].
    Custom,
    Shell,
    Other,
}

impl AgentKind {
    /// Classify a process/command name. `extra_agents` wins over the
    /// built-in table so users can promote anything to agent status.
    pub fn from_command(cmd: &str, extra_agents: &[String]) -> Self {
        if extra_agents.iter().any(|a| a == cmd) {
            return Self::Custom;
        }
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
            Self::Custom => "custom",
            Self::Shell => "shell",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Flags {
    pub has_intent: bool,
    pub blocked: bool,
    pub reviewed_fresh: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub agents: Vec<Agent>,
    /// Capture time as epoch milliseconds — stable across timezones, easy
    /// to diff between snapshots when consumers poll.
    #[serde(rename = "captured_at_ms", serialize_with = "serialize_epoch_ms")]
    pub captured_at: SystemTime,
    /// Total blocked sessions in `~/.claude/state/` — not always pane-mappable.
    pub global_blocked: usize,
    /// Non-fatal error from the most recent `tmux list-panes` call, if any.
    /// Surfaced as a banner so the user doesn't mistake "tmux is broken" for
    /// "nothing is running."
    pub panes_error: Option<String>,
    /// Cross-tool attention queue (Claude stop / notification / codex turn
    /// hooks). Newest first. Pre-filtered to the same 1h cutoff
    /// `attention-picker.sh` uses so the two surfaces agree on what's pending.
    pub attention: Vec<attention::AttentionEntry>,
    /// Live codex-companion background jobs, structured. The same jobs are
    /// folded into `agents[]` (kind=codex, status=bg) for the TUI, but that
    /// shape is lossy — `extra` flattens title/status/age into one display
    /// string. Machine consumers (copad's web-bridge cockpit) read this
    /// field instead of re-parsing `~/.claude/state/codex-companion/`.
    pub codex_jobs: Vec<state::CodexJob>,
}

fn serialize_epoch_ms<S: serde::Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
    let ms = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    s.serialize_i64(ms)
}

impl Snapshot {
    pub fn empty() -> Self {
        Self {
            agents: Vec::new(),
            captured_at: SystemTime::now(),
            global_blocked: 0,
            panes_error: None,
            attention: Vec::new(),
            codex_jobs: Vec::new(),
        }
    }
}

pub fn run() -> anyhow::Result<()> {
    ui::run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_command_builtin_table() {
        let none: &[String] = &[];
        assert_eq!(AgentKind::from_command("claude", none), AgentKind::Claude);
        assert_eq!(AgentKind::from_command("codex", none), AgentKind::Codex);
        assert_eq!(AgentKind::from_command("zsh", none), AgentKind::Shell);
        assert_eq!(AgentKind::from_command("nvim", none), AgentKind::Other);
    }

    #[test]
    fn from_command_extra_agents_become_custom() {
        let extras = vec!["gemini".to_string(), "opencode".to_string()];
        assert_eq!(
            AgentKind::from_command("gemini", &extras),
            AgentKind::Custom
        );
        assert_eq!(
            AgentKind::from_command("opencode", &extras),
            AgentKind::Custom
        );
        // Non-listed names keep their built-in classification.
        assert_eq!(
            AgentKind::from_command("claude", &extras),
            AgentKind::Claude
        );
        assert_eq!(AgentKind::from_command("htop", &extras), AgentKind::Other);
    }

    #[test]
    fn from_command_extra_agents_override_builtins() {
        // A user listing a shell as an agent wins over the built-in table.
        let extras = vec!["zsh".to_string()];
        assert_eq!(AgentKind::from_command("zsh", &extras), AgentKind::Custom);
    }

    #[test]
    fn agent_json_exposes_kind_and_command() {
        let agent = Agent {
            id: "pane:s:0.0".into(),
            pane: None,
            kind: AgentKind::Custom,
            command: "gemini".into(),
            status: Status::Idle,
            cwd: PathBuf::from("/x"),
            repo_name: "x".into(),
            flags: Flags::default(),
            extra: String::new(),
        };
        let json = serde_json::to_value(&agent).unwrap();
        assert_eq!(json["kind"], "custom");
        assert_eq!(json["command"], "gemini");
    }

    /// The `--json` contract for machine consumers (copad web-bridge):
    /// `codex_jobs` carries the structured rows with snake_case fields —
    /// the lossy `agents[]` fold (extra: "running • 3m ago") is for the
    /// TUI only. Renaming any field here breaks downstream deserializers.
    #[test]
    fn snapshot_json_exposes_structured_codex_jobs() {
        let mut snap = Snapshot::empty();
        snap.codex_jobs.push(state::CodexJob {
            id: "task-abc".into(),
            title: "Review diff".into(),
            kind_label: "task".into(),
            workspace_root: PathBuf::from("/home/u/dev/copad"),
            status: "running".into(),
            started_at_ms: Some(1_700_000_000_000),
            updated_at_ms: Some(1_700_000_001_000),
            pid: Some(4242),
        });
        let json = serde_json::to_value(&snap).unwrap();
        let job = &json["codex_jobs"][0];
        assert_eq!(job["id"], "task-abc");
        assert_eq!(job["title"], "Review diff");
        assert_eq!(job["kind_label"], "task");
        assert_eq!(job["workspace_root"], "/home/u/dev/copad");
        assert_eq!(job["status"], "running");
        assert_eq!(job["started_at_ms"], 1_700_000_000_000_i64);
        assert_eq!(job["updated_at_ms"], 1_700_000_001_000_i64);
        assert_eq!(job["pid"], 4242);
    }
}
