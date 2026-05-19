//! Read Claude Code's own session-state file:
//! `~/.claude/sessions/<PID>.json`. The file holds Claude's view of the
//! UI status — `busy` (mid-turn), `idle` (at chat prompt),
//! `waiting` (selection / permission dialog up) — keyed by the Claude
//! process PID. That makes it a perfect primary signal for the
//! dashboard: no pane scraping, no pattern matching, no session-UUID
//! mapping.
//!
//! Caveats (per codex review):
//!   * Undocumented surface — Claude could rename / restructure / drop
//!     the file at any version. Treat as **best-effort telemetry**.
//!   * Partial writes are possible — JSON parse failure on a fresh
//!     mtime means we should retry rather than mis-classify.
//!   * Stale entries can outlive the process. Caller already gates on a
//!     live PID via sysinfo, but a check on `version`/`updatedAt` is
//!     still worth surfacing if drift is observed.
//!
//! All failure paths return `None` so the dashboard stays alive and
//! falls back to the pane-capture classifier.

use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// `busy` — Claude is mid-turn (running tools or composing).
    Busy,
    /// `idle` — Claude is parked at the chat input prompt.
    Idle,
    /// `waiting` — Claude is blocking on a permission / selection dialog.
    Waiting,
}

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub status: SessionStatus,
    pub session_id: String,
    /// `updatedAt` epoch-millis. Recent = the status we read reflects the
    /// current UI state. Hours old + `busy` = the previous Claude exited
    /// mid-turn without rewriting the file (rare but observable).
    pub updated_at_ms: i64,
}

#[derive(serde::Deserialize)]
struct Raw {
    #[serde(default)]
    status: String,
    #[serde(default, rename = "sessionId")]
    session_id: String,
    #[serde(default, rename = "updatedAt")]
    updated_at: i64,
}

pub fn session_path(pid: u32) -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(format!(".claude/sessions/{pid}.json")))
}

/// Read + parse the per-PID session file. Returns `None` on any failure
/// path (missing, unreadable, malformed JSON, unrecognised status) so
/// the caller can fall back to another signal.
pub fn read(pid: u32) -> Option<SessionMeta> {
    let path = session_path(pid)?;
    let bytes = fs::read(&path).ok()?;
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Option<SessionMeta> {
    let raw: Raw = serde_json::from_slice(bytes).ok()?;
    let status = parse_status(&raw.status)?;
    Some(SessionMeta {
        status,
        session_id: raw.session_id,
        updated_at_ms: raw.updated_at,
    })
}

fn parse_status(s: &str) -> Option<SessionStatus> {
    match s {
        "busy" => Some(SessionStatus::Busy),
        "idle" => Some(SessionStatus::Idle),
        "waiting" => Some(SessionStatus::Waiting),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_busy() {
        let json =
            br#"{"pid":1,"sessionId":"s-1","cwd":"/x","status":"busy","updatedAt":1700000000000}"#;
        let m = parse(json).expect("parses");
        assert_eq!(m.status, SessionStatus::Busy);
        assert_eq!(m.session_id, "s-1");
        assert_eq!(m.updated_at_ms, 1700000000000);
    }

    #[test]
    fn parse_known_idle() {
        let json = br#"{"status":"idle","sessionId":"sid"}"#;
        let m = parse(json).expect("parses");
        assert_eq!(m.status, SessionStatus::Idle);
    }

    #[test]
    fn parse_known_waiting() {
        let json = br#"{"status":"waiting"}"#;
        let m = parse(json).expect("parses");
        assert_eq!(m.status, SessionStatus::Waiting);
    }

    #[test]
    fn parse_unknown_status_returns_none() {
        // Future Claude versions might add new states; refuse to guess.
        let json = br#"{"status":"thinking-hard"}"#;
        assert!(parse(json).is_none());
    }

    #[test]
    fn parse_malformed_json_returns_none() {
        assert!(parse(b"not json").is_none());
        assert!(parse(b"").is_none());
        // Truncated mid-write — important since Claude rewrites the
        // file atomically-ish but a stat-then-read race is possible.
        assert!(parse(br#"{"status":"bu"#).is_none());
    }

    #[test]
    fn parse_missing_status_field_returns_none() {
        let json = br#"{"sessionId":"s-1"}"#;
        assert!(parse(json).is_none());
    }

    #[test]
    fn parse_tolerates_extra_fields() {
        // Forward-compat: the real file has pid/cwd/startedAt/procStart/
        // version/peerProtocol/kind/entrypoint we don't read.
        let json = br#"{
            "pid": 22650,
            "sessionId": "31e571fa-2230-4854-908a-5d13e7a017b6",
            "cwd": "/home/x/dev/tmx",
            "startedAt": 1779149382456,
            "procStart": "126214",
            "version": "2.1.143",
            "peerProtocol": 1,
            "kind": "interactive",
            "entrypoint": "cli",
            "status": "busy",
            "updatedAt": 1779165593130
        }"#;
        let m = parse(json).expect("parses real-shape file");
        assert_eq!(m.status, SessionStatus::Busy);
        assert_eq!(m.session_id, "31e571fa-2230-4854-908a-5d13e7a017b6");
        assert_eq!(m.updated_at_ms, 1779165593130);
    }

    #[test]
    fn session_path_uses_pid_filename() {
        let p = session_path(42).expect("home dir present in test env");
        assert!(p.ends_with(".claude/sessions/42.json"));
    }
}
