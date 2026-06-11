//! Combine tmux panes + Claude state markers + process tree into a
//! `Snapshot`. Pure I/O orchestration — all the parsing lives elsewhere.

use std::path::Path;
use std::time::SystemTime;

use crate::config::AgentsConfig;
use crate::tmux;

use super::attention;
use super::classify::{self, ClaudeUiState};
use super::panes::{self, PaneInfo};
use super::proc::ProcSnapshot;
use super::session_meta::{self, SessionStatus};
use super::state::{self, CodexJob};
use super::{Agent, AgentKind, Flags, PaneLocator, Snapshot, Status};

const AGENT_TARGETS: &[&str] = &["claude", "codex"];

/// Same 1-hour cutoff `attention-picker.sh` uses. Older entries are
/// considered stale and never surfaced.
const ATTENTION_CUTOFF_SECS: i64 = 3600;

/// Collect a fresh snapshot. `proc` may be reused across ticks — call
/// `refresh()` on it before passing it in. `cfg` carries the dashboard
/// knobs from `[agents]` in config (attention cap, extra agent names).
pub fn collect(proc: &ProcSnapshot, cfg: &AgentsConfig) -> Snapshot {
    let (panes, panes_error) = match panes::list_panes() {
        Ok(p) => (p, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let markers = state::scan();
    let codex_jobs: Vec<_> = state::codex_jobs()
        .into_iter()
        .filter(|j| is_codex_job_alive(j, proc))
        .collect();

    let mut targets: Vec<&str> = AGENT_TARGETS.to_vec();
    targets.extend(cfg.extra_agents.iter().map(String::as_str));

    let mut agents: Vec<Agent> = Vec::with_capacity(panes.len() + codex_jobs.len());
    for pane in &panes {
        agents.push(build_pane_agent(pane, proc, &markers, &targets, cfg));
    }
    for job in &codex_jobs {
        agents.push(build_codex_job(job));
    }

    Snapshot {
        agents,
        captured_at: SystemTime::now(),
        global_blocked: markers.blocked_count,
        panes_error,
        attention: attention::read_recent(ATTENTION_CUTOFF_SECS, cfg.attention_limit),
        codex_jobs,
    }
}

/// Drop zombie codex jobs — entries left as `running` by a crashed
/// codex-companion. A job is real iff:
///   * its recorded `pid` still exists in the process table, AND
///   * `updatedAt` is within the freshness window.
///
/// The `updatedAt` guard covers the PID-reuse case where a long-dead
/// codex PID happens to match some other current process.
fn is_codex_job_alive(job: &CodexJob, proc: &ProcSnapshot) -> bool {
    let now_ms = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let fresh = job
        .updated_at_ms
        .map(|t| (now_ms - t).abs() < state::CODEX_JOB_MAX_AGE_MS)
        .unwrap_or(false);
    if !fresh {
        return false;
    }
    match job.pid {
        Some(pid) => proc.pid_alive(pid),
        // Recent + null pid is uncommon but plausible during a startup
        // race; trust the file rather than drop.
        None => true,
    }
}

fn build_pane_agent(
    pane: &PaneInfo,
    proc: &ProcSnapshot,
    markers: &state::ClaudeStateMarkers,
    targets: &[&str],
    cfg: &AgentsConfig,
) -> Agent {
    // Prefer the deepest agent descendant for cwd + identity. Fall back to
    // the pane's own current_command + current_path when no agent process
    // is found (idle shell, nvim, etc).
    let descendant = proc.find_descendant(pane.pane_pid, targets);

    let (kind, command, cwd_for_state, status, extra) = match &descendant {
        Some(d) => {
            let kind = AgentKind::from_command(&d.name, &cfg.extra_agents);
            let cwd = d.cwd.clone().unwrap_or_else(|| pane.current_path.clone());
            let status = resolve_status(d.pid, kind, pane);
            (kind, d.name.clone(), cwd, status, format!("pid {}", d.pid))
        }
        None => {
            let kind = AgentKind::from_command(&pane.current_command, &cfg.extra_agents);
            // No agent process — the pane is at a plain shell or running
            // something we don't classify. All of these are idle. Surface
            // the actual foreground command so a `shell`/`other` row still
            // tells the user what is running there.
            (
                kind,
                pane.current_command.clone(),
                pane.current_path.clone(),
                Status::Idle,
                pane.current_command.clone(),
            )
        }
    };

    let repo_root = state::find_repo_root(&cwd_for_state);
    let repo_name = repo_root
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            cwd_for_state
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        });

    let flags = build_flags(repo_root.as_deref(), markers);

    let locator = PaneLocator {
        session: pane.session.clone(),
        window: pane.window,
        pane: pane.pane,
        pane_pid: pane.pane_pid,
    };
    Agent {
        id: format!("pane:{}", locator.target()),
        pane: Some(locator),
        kind,
        command,
        status,
        cwd: cwd_for_state,
        repo_name,
        flags,
        extra,
    }
}

fn pane_target(pane: &PaneInfo) -> String {
    format!("{}:{}.{}", pane.session, pane.window, pane.pane)
}

/// Decide the UI status for a Claude/Codex descendant. Primary signal is
/// Claude's own `~/.claude/sessions/<pid>.json` (efficient + accurate);
/// pane-capture pattern matching is the fallback for codex sessions and
/// for cases where Claude's state file is missing / malformed / a
/// future version has changed the schema.
fn resolve_status(pid: u32, kind: AgentKind, pane: &PaneInfo) -> Status {
    if kind == AgentKind::Claude
        && let Some(meta) = session_meta::read(pid)
    {
        return match meta.status {
            SessionStatus::Busy => Status::Working,
            SessionStatus::Idle => Status::Ready,
            SessionStatus::Waiting => Status::AwaitingDecision,
        };
    }
    // Fallback: read what the user sees and classify by pattern.
    let ui_state = tmux::capture_pane(&pane_target(pane))
        .map(|s| classify::classify(&s))
        .unwrap_or(ClaudeUiState::Unknown);
    match ui_state {
        ClaudeUiState::AwaitingDecision => Status::AwaitingDecision,
        ClaudeUiState::Working => Status::Working,
        ClaudeUiState::Ready => Status::Ready,
        ClaudeUiState::Unknown => Status::Idle,
    }
}

fn build_flags(repo_root: Option<&Path>, markers: &state::ClaudeStateMarkers) -> Flags {
    let Some(root) = repo_root else {
        return Flags::default();
    };
    let hash = state::repo_hash(root);
    Flags {
        has_intent: markers.intent_repo_hashes.contains(&hash),
        blocked: false, // not pane-mappable; surfaced globally instead
        reviewed_fresh: markers.reviewed_repo_hashes.contains(&hash),
    }
}

fn build_codex_job(job: &CodexJob) -> Agent {
    let age = job.started_at_ms.map(format_age_ms).unwrap_or_default();
    let extra = if age.is_empty() {
        job.status.clone()
    } else {
        format!("{} • {}", job.status, age)
    };
    Agent {
        id: format!("codex:{}", job.id),
        pane: None,
        kind: AgentKind::Codex,
        command: "codex".to_string(),
        status: Status::Background,
        cwd: job.workspace_root.clone(),
        repo_name: job.repo_name(),
        flags: Flags::default(),
        extra,
    }
}

fn format_age_ms(epoch_ms: i64) -> String {
    let now_ms: i64 = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let age_secs = ((now_ms - epoch_ms).max(0) / 1000) as u64;
    if age_secs < 60 {
        format!("{age_secs}s ago")
    } else if age_secs < 3600 {
        format!("{}m ago", age_secs / 60)
    } else if age_secs < 86_400 {
        format!("{}h ago", age_secs / 3600)
    } else {
        format!("{}d ago", age_secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_age_ms_buckets() {
        let now_ms: i64 = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(format_age_ms(now_ms).ends_with("s ago"));
        assert!(format_age_ms(now_ms - 120_000).ends_with("m ago"));
        assert!(format_age_ms(now_ms - 7_200_000).ends_with("h ago"));
        assert!(format_age_ms(now_ms - 200_000_000).ends_with("d ago"));
    }

    #[test]
    fn build_flags_matches_repo_hash() {
        let mut markers = state::ClaudeStateMarkers::default();
        let root = Path::new("/home/me/dev/tmx");
        let hash = state::repo_hash(root);
        markers.reviewed_repo_hashes.insert(hash.clone());
        markers.intent_repo_hashes.insert(hash);
        let flags = build_flags(Some(root), &markers);
        assert!(flags.reviewed_fresh);
        assert!(flags.has_intent);
    }

    #[test]
    fn build_flags_no_root_is_empty() {
        let markers = state::ClaudeStateMarkers::default();
        let flags = build_flags(None, &markers);
        assert_eq!(flags, Flags::default());
    }

    fn fake_job(pid: Option<u32>, updated_ago_ms: i64) -> state::CodexJob {
        let now_ms = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        state::CodexJob {
            id: "task-x".into(),
            title: String::new(),
            kind_label: String::new(),
            workspace_root: std::path::PathBuf::new(),
            status: "running".into(),
            started_at_ms: Some(now_ms - updated_ago_ms),
            updated_at_ms: Some(now_ms - updated_ago_ms),
            pid,
        }
    }

    #[test]
    fn is_codex_job_alive_drops_stale() {
        let proc = ProcSnapshot::new();
        // Two hours old — well past the 1h freshness window.
        let stale = fake_job(Some(std::process::id()), 7_200_000);
        assert!(!is_codex_job_alive(&stale, &proc));
    }

    #[test]
    fn is_codex_job_alive_drops_dead_pid() {
        let proc = ProcSnapshot::new();
        // Fresh updatedAt, but pid 1 unlikely matches… on Linux pid 1
        // is init/systemd and is always alive, so use a guaranteed-dead
        // pid instead. u32::MAX is far above any realistic pid.
        let fresh_but_dead_pid = fake_job(Some(u32::MAX - 1), 1000);
        assert!(!is_codex_job_alive(&fresh_but_dead_pid, &proc));
    }

    #[test]
    fn is_codex_job_alive_keeps_fresh_with_live_pid() {
        let proc = ProcSnapshot::new();
        // Use our own PID — guaranteed live in the snapshot.
        let me = std::process::id();
        let live = fake_job(Some(me), 1000);
        assert!(is_codex_job_alive(&live, &proc));
    }

    #[test]
    fn is_codex_job_alive_no_updated_at_drops() {
        // Without an updatedAt we can't establish freshness; treat as
        // stale to err on the side of pruning zombies.
        let proc = ProcSnapshot::new();
        let mut j = fake_job(Some(std::process::id()), 0);
        j.updated_at_ms = None;
        assert!(!is_codex_job_alive(&j, &proc));
    }
}
