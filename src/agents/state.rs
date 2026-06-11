//! Scan `~/.claude/state/` for markers that tell us what's blocked, what has
//! an active intent, and which repos have a fresh review marker.
//!
//! Marker schemes mirror `~/.claude/hooks/_lib.sh::repo_hash` (first 12 chars
//! of MD5 of the repo root path string). We deliberately re-implement the
//! hash rather than shelling out to keep this read-only and fast.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};

/// Snapshot of the marker files we care about.
#[derive(Debug, Default, Clone)]
pub struct ClaudeStateMarkers {
    /// All `reviewed-<repohash>` markers present.
    pub reviewed_repo_hashes: HashSet<String>,
    /// All `intent-active-<sid>-<repohash>.path` markers — keyed by repo hash.
    pub intent_repo_hashes: HashSet<String>,
    /// Total `stop-blocked-<sid>` markers (not pane-mappable without
    /// inspecting open file descriptors — surfaced as a global counter).
    pub blocked_count: usize,
}

pub fn state_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude/state"))
}

pub fn scan() -> ClaudeStateMarkers {
    let Some(dir) = state_dir() else {
        return ClaudeStateMarkers::default();
    };
    scan_dir(&dir)
}

pub fn scan_dir(dir: &Path) -> ClaudeStateMarkers {
    let Ok(read) = fs::read_dir(dir) else {
        return ClaudeStateMarkers::default();
    };
    let mut markers = ClaudeStateMarkers::default();
    for entry in read.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        classify(&name, &mut markers);
    }
    markers
}

fn classify(name: &str, markers: &mut ClaudeStateMarkers) {
    if let Some(hash) = name.strip_prefix("reviewed-") {
        // Markers come in 12-char and 40-char variants. Only the 12-char
        // (`repo_hash`) form is what the active hooks write today, but we
        // keep any hex-looking suffix as a candidate.
        if is_hex(hash) {
            markers.reviewed_repo_hashes.insert(hash.to_string());
        }
        return;
    }
    if let Some(rest) = name.strip_prefix("intent-active-")
        && let Some(stem) = rest.strip_suffix(".path")
    {
        // Filename shape: intent-active-<SESSION_ID>-<REPO_HASH>.path
        // SESSION_ID itself may contain dashes (e.g. "flow-test-1779031295"),
        // so we anchor on the *last* dash-separated chunk being a hex hash.
        if let Some((_, tail)) = stem.rsplit_once('-')
            && is_hex(tail)
        {
            markers.intent_repo_hashes.insert(tail.to_string());
        }
        return;
    }
    if name.starts_with("stop-blocked-") {
        markers.blocked_count += 1;
    }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Compute the same 12-char marker hash that `~/.claude/hooks/_lib.sh` writes.
pub fn repo_hash(repo_root: &Path) -> String {
    let s = repo_root.to_string_lossy();
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(12);
    for byte in digest.iter().take(6) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Walk parents of `path` until a `.git` entry is found; returns that
/// directory. `None` if not inside a git repo (or if I/O fails).
pub fn find_repo_root(path: &Path) -> Option<PathBuf> {
    let mut cur = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

/// One row from a codex-companion workspace's `state.json::jobs[]`. The
/// companion writes one workspace directory per repo, each with a single
/// `state.json` carrying the full job history — we filter to the still-
/// active ones so the dashboard isn't dominated by completed tasks.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CodexJob {
    /// Stable per-job identifier (used as the cursor anchor key, so
    /// navigation survives across refreshes).
    pub id: String,
    pub title: String,
    pub kind_label: String,
    /// Absolute workspace path the job was launched against.
    pub workspace_root: PathBuf,
    pub status: String,
    pub started_at_ms: Option<i64>,
    /// `updatedAt` epoch-millis. Used as a freshness signal: a `running`
    /// job whose `updatedAt` is hours old is almost certainly a zombie
    /// (codex-companion crashed without flipping the status).
    pub updated_at_ms: Option<i64>,
    /// PID the codex-companion recorded for this job. `None` for jobs
    /// where the field was absent or null (typical for terminal states).
    pub pid: Option<u32>,
}

impl CodexJob {
    /// Repo basename for display purposes — falls back to the last segment
    /// of the workspace root when present.
    pub fn repo_name(&self) -> String {
        self.workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    }

    /// True if the job is still doing work — anything not terminal. The
    /// codex-companion uses `completed` / `failed` / `cancelled` as
    /// terminal states; we treat any other value (including `running`,
    /// `queued`, missing, etc.) as active.
    pub fn is_active(&self) -> bool {
        !matches!(
            self.status.as_str(),
            "completed" | "failed" | "cancelled" | "canceled"
        )
    }
}

/// Heuristic: how long can a job sit in non-terminal status before we
/// stop trusting the file? Real codex tasks finish in seconds-minutes;
/// anything past an hour is almost certainly a zombie left by a
/// codex-companion crash that never wrote the terminal status.
pub const CODEX_JOB_MAX_AGE_MS: i64 = 3600 * 1000;

#[derive(serde::Deserialize)]
struct StateFile {
    jobs: Option<Vec<RawJob>>,
}

#[derive(serde::Deserialize)]
struct RawJob {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(rename = "kindLabel", default)]
    kind_label: String,
    #[serde(rename = "workspaceRoot", default)]
    workspace_root: String,
    #[serde(default)]
    status: String,
    #[serde(rename = "startedAt", default)]
    started_at: String,
    #[serde(rename = "updatedAt", default)]
    updated_at: String,
    /// codex-companion writes either an integer or `null` here.
    #[serde(default)]
    pid: Option<u32>,
}

/// Enumerate currently-active codex-companion jobs across every workspace
/// directory. `state.json` is the source of truth; directory mtime is no
/// longer trusted as a job signal.
pub fn codex_jobs() -> Vec<CodexJob> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let dir = home.join(".claude/state/codex-companion/state");
    codex_jobs_in(&dir)
}

pub fn codex_jobs_in(dir: &Path) -> Vec<CodexJob> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut jobs: Vec<CodexJob> = Vec::new();
    for entry in read.flatten() {
        let ok = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !ok {
            continue;
        }
        jobs.extend(read_workspace_jobs(&entry.path()));
    }
    jobs.retain(CodexJob::is_active);
    jobs.sort_by_key(|j| std::cmp::Reverse(j.started_at_ms));
    jobs
}

fn read_workspace_jobs(workspace_dir: &Path) -> Vec<CodexJob> {
    let state_path = workspace_dir.join("state.json");
    let Ok(bytes) = fs::read(&state_path) else {
        return Vec::new();
    };
    let Ok(file): Result<StateFile, _> = serde_json::from_slice(&bytes) else {
        return Vec::new();
    };
    file.jobs
        .unwrap_or_default()
        .into_iter()
        .map(|j| CodexJob {
            id: j.id,
            title: j.title,
            kind_label: j.kind_label,
            workspace_root: PathBuf::from(j.workspace_root),
            status: j.status,
            started_at_ms: parse_iso8601_millis(&j.started_at),
            updated_at_ms: parse_iso8601_millis(&j.updated_at),
            pid: j.pid,
        })
        .collect()
}

/// Minimal ISO-8601 → epoch-millis parser tuned for codex-companion's
/// `YYYY-MM-DDTHH:MM:SS.fffZ` output. Anything more exotic returns None
/// and the row simply loses its age string.
fn parse_iso8601_millis(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    // Format: 2026-05-19T01:56:52.454Z
    let bytes = s.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    let mut millis: i64 = 0;
    if let Some(dot) = s.get(19..20)
        && dot == "."
    {
        let frac = s.get(20..23)?;
        millis = frac.parse().ok()?;
    }
    let days = days_from_civil(year, month, day);
    let epoch_days = days - 719_468; // days(1970-01-01)
    let secs = epoch_days * 86_400 + hour * 3600 + min * 60 + sec;
    Some(secs * 1000 + millis)
}

/// Howard Hinnant's days_from_civil — fast, exact, no allocations.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u32; // [0, 399]
    let m_adj = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_adj + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn classify_reviewed_marker() {
        let mut m = ClaudeStateMarkers::default();
        classify("reviewed-0cc68405c025", &mut m);
        assert!(m.reviewed_repo_hashes.contains("0cc68405c025"));
    }

    #[test]
    fn classify_reviewed_longer_hash_still_hex() {
        let mut m = ClaudeStateMarkers::default();
        classify("reviewed-6e6c4ed60ca111a1ec98145c09ff9d9426b78f54", &mut m);
        assert_eq!(m.reviewed_repo_hashes.len(), 1);
    }

    #[test]
    fn classify_reviewed_rejects_non_hex() {
        let mut m = ClaudeStateMarkers::default();
        classify("reviewed-not_a_hash", &mut m);
        assert!(m.reviewed_repo_hashes.is_empty());
    }

    #[test]
    fn classify_intent_active_extracts_trailing_hash() {
        let mut m = ClaudeStateMarkers::default();
        classify(
            "intent-active-flow-test-1779031295-b2c6a07f1836.path",
            &mut m,
        );
        assert!(m.intent_repo_hashes.contains("b2c6a07f1836"));
    }

    #[test]
    fn classify_blocked_increments_count() {
        let mut m = ClaudeStateMarkers::default();
        classify("stop-blocked-abc123", &mut m);
        classify("stop-blocked-xyz789", &mut m);
        assert_eq!(m.blocked_count, 2);
    }

    #[test]
    fn classify_ignores_unrelated() {
        let mut m = ClaudeStateMarkers::default();
        classify("dirty-some-uuid.log", &mut m);
        classify("auto-review-disabled", &mut m);
        assert!(m.reviewed_repo_hashes.is_empty());
        assert!(m.intent_repo_hashes.is_empty());
        assert_eq!(m.blocked_count, 0);
    }

    #[test]
    fn repo_hash_stable() {
        let h1 = repo_hash(Path::new("/home/me/dev/tmx"));
        let h2 = repo_hash(Path::new("/home/me/dev/tmx"));
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn repo_hash_differs_by_path() {
        let h1 = repo_hash(Path::new("/home/me/dev/tmx"));
        let h2 = repo_hash(Path::new("/home/me/dev/nestty"));
        assert_ne!(h1, h2);
    }

    #[test]
    fn scan_dir_collects_all_marker_kinds() {
        let tmp = tempdir().unwrap();
        File::create(tmp.path().join("reviewed-deadbeef0123")).unwrap();
        File::create(tmp.path().join("intent-active-sid-uuid-cafef00d1234.path")).unwrap();
        File::create(tmp.path().join("stop-blocked-some-session")).unwrap();
        File::create(tmp.path().join("dirty-some-session.log")).unwrap();

        let markers = scan_dir(tmp.path());
        assert!(markers.reviewed_repo_hashes.contains("deadbeef0123"));
        assert!(markers.intent_repo_hashes.contains("cafef00d1234"));
        assert_eq!(markers.blocked_count, 1);
    }

    #[test]
    fn scan_dir_missing_dir_is_empty() {
        let markers = scan_dir(Path::new("/nonexistent/path/xyz/123"));
        assert!(markers.reviewed_repo_hashes.is_empty());
        assert_eq!(markers.blocked_count, 0);
    }

    #[test]
    fn find_repo_root_walks_up_to_git_dir() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        let root = find_repo_root(&nested).unwrap();
        assert_eq!(
            root.canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn codex_job_repo_name_uses_workspace_basename() {
        let j = CodexJob {
            id: "task-x".into(),
            title: "Codex Task".into(),
            kind_label: "rescue".into(),
            workspace_root: PathBuf::from("/home/me/dev/tmx"),
            status: "running".into(),
            started_at_ms: Some(0),
            updated_at_ms: Some(0),
            pid: None,
        };
        assert_eq!(j.repo_name(), "tmx");
    }

    #[test]
    fn codex_job_is_active_excludes_terminal_states() {
        let base = CodexJob {
            id: "task-x".into(),
            title: String::new(),
            kind_label: String::new(),
            workspace_root: PathBuf::new(),
            status: "running".into(),
            started_at_ms: None,
            updated_at_ms: None,
            pid: None,
        };
        assert!(base.is_active());
        for s in ["completed", "failed", "cancelled", "canceled"] {
            assert!(
                !CodexJob {
                    status: s.into(),
                    ..base.clone()
                }
                .is_active(),
                "{s} should be terminal"
            );
        }
    }

    #[test]
    fn codex_jobs_in_filters_to_active_and_parses_state_json() {
        let tmp = tempdir().unwrap();
        let ws = tmp.path().join("tmx-abcd1234");
        fs::create_dir_all(&ws).unwrap();
        let state = r#"{
            "version": 1,
            "jobs": [
                {"id": "task-running", "title": "Live", "kindLabel": "rescue",
                 "workspaceRoot": "/home/me/dev/tmx", "status": "running",
                 "startedAt": "2026-05-19T01:00:00.000Z",
                 "updatedAt": "2026-05-19T01:01:00.000Z",
                 "pid": 12345},
                {"id": "task-done", "title": "Old", "kindLabel": "rescue",
                 "workspaceRoot": "/home/me/dev/tmx", "status": "completed",
                 "startedAt": "2026-05-18T01:00:00.000Z",
                 "updatedAt": "2026-05-18T01:00:01.000Z",
                 "pid": null}
            ]
        }"#;
        fs::write(ws.join("state.json"), state).unwrap();

        let jobs = codex_jobs_in(tmp.path());
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "task-running");
        assert_eq!(jobs[0].status, "running");
        assert_eq!(jobs[0].pid, Some(12345));
        assert!(jobs[0].updated_at_ms.is_some());
    }

    #[test]
    fn codex_jobs_in_skips_missing_state_json() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("empty-workspace")).unwrap();
        let jobs = codex_jobs_in(tmp.path());
        assert!(jobs.is_empty());
    }

    #[test]
    fn parse_iso8601_known_value() {
        // 2026-05-19T01:56:52.454Z
        let ms = parse_iso8601_millis("2026-05-19T01:56:52.454Z").unwrap();
        // Sanity-check by round-tripping through a separate civil computation:
        // 2026-05-19 == days_from_civil(2026, 5, 19) - 719_468.
        let days = days_from_civil(2026, 5, 19) - 719_468;
        let secs = days * 86_400 + 3600 + 56 * 60 + 52;
        assert_eq!(ms, secs * 1000 + 454);
    }

    #[test]
    fn parse_iso8601_rejects_garbage() {
        assert!(parse_iso8601_millis("").is_none());
        assert!(parse_iso8601_millis("not a date").is_none());
        assert!(parse_iso8601_millis("2026-13-99T99:99:99.999Z").is_some()); // lenient on out-of-range, just needs to parse
    }
}
