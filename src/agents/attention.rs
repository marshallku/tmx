//! Read the cross-tool attention queue at
//! `${XDG_CACHE_HOME:-$HOME/.cache}/claude-attention/queue.jsonl`.
//!
//! This file is the shared "notification inbox" produced by the
//! `notify-stop.sh` / `notify-notification.sh` / `notify-codex.sh`
//! hooks. Each line is one event Claude or Codex thinks the user should
//! eventually look at — turn finished, prompt waiting, codex response
//! ready. `attention-picker.sh` (bound to `prefix + A`) consumes them
//! via fzf with a 1-hour cutoff; we apply the same cutoff so the
//! dashboard agrees with the picker on "what's pending".
//!
//! The file is append-only — entries leave only when the picker
//! deletes them on consumption. We tolerate malformed lines (truncated
//! writes, hand-edited junk) by skipping them.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// One pending attention entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AttentionEntry {
    pub ts: i64,
    pub kind: String,
    pub source: String,
    pub title: String,
    pub body: String,
    pub session_id: String,
    pub cwd: PathBuf,
    pub tmux_target: String,
    pub tmux_session: String,
}

#[derive(serde::Deserialize)]
struct Raw {
    ts: i64,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    tmux_target: String,
    #[serde(default)]
    tmux_session: String,
}

/// Path the bash hooks write to. Mirrors `${XDG_CACHE_HOME:-$HOME/.cache}`
/// exactly — we deliberately do NOT use `dirs::cache_dir()` because on
/// macOS that returns `~/Library/Caches`, while the shell hooks fall back
/// to `$HOME/.cache` even on macOS.
pub fn queue_path() -> Option<PathBuf> {
    let cache = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))?;
    Some(cache.join("claude-attention/queue.jsonl"))
}

/// The queue file is append-only and can grow without bound; reading and
/// parsing all of it on every dashboard tick would scale with its lifetime
/// size. Only this much of the tail is read — far more than `limit` recent
/// entries can occupy.
const TAIL_READ_BYTES: u64 = 256 * 1024;

/// Read the queue, filter to entries with `ts >= now - cutoff_secs`,
/// return newest first, at most `limit` entries. Missing / unreadable
/// file → empty Vec.
pub fn read_recent(cutoff_secs: i64, limit: usize) -> Vec<AttentionEntry> {
    let Some(path) = queue_path() else {
        return Vec::new();
    };
    let Some(bytes) = read_tail(&path) else {
        return Vec::new();
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let cutoff = now.saturating_sub(cutoff_secs);
    parse_recent(&bytes, cutoff, limit)
}

/// Read at most the last [`TAIL_READ_BYTES`] of complete lines. When the
/// file is larger than the window, the read starts one byte earlier than
/// the window and everything through the first newline is dropped: if the
/// window happens to begin exactly at a line start, that extra byte is the
/// preceding record's `\n`, so the complete first line survives; otherwise
/// the dropped prefix is exactly the partial first line.
fn read_tail(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    // One byte wider than the nominal window: when the window happens to
    // begin exactly at a line start, that byte is the preceding record's
    // `\n`, letting the boundary check below keep the complete first line.
    let window = TAIL_READ_BYTES + 1;
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(window);
    if start > 0 {
        file.seek(SeekFrom::Start(start)).ok()?;
    }
    let mut buf = Vec::with_capacity(len.min(window) as usize);
    // `take` holds the bound even if the file grows between `metadata()`
    // and this read (the hooks append concurrently).
    file.take(window).read_to_end(&mut buf).ok()?;
    if start > 0 {
        match buf.iter().position(|b| *b == b'\n') {
            Some(pos) => {
                buf.drain(..=pos);
            }
            // The whole window is one partial line — nothing usable.
            None => buf.clear(),
        }
    }
    Some(buf)
}

pub fn parse_recent(bytes: &[u8], cutoff: i64, limit: usize) -> Vec<AttentionEntry> {
    let mut entries: Vec<AttentionEntry> = bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_slice::<Raw>(line).ok())
        .filter(|r| r.ts >= cutoff)
        .map(|r| AttentionEntry {
            ts: r.ts,
            kind: r.kind,
            source: r.source,
            title: r.title,
            body: r.body,
            session_id: r.session_id,
            cwd: PathBuf::from(r.cwd),
            tmux_target: r.tmux_target,
            tmux_session: r.tmux_session,
        })
        .collect();
    // Newest first — picker shows newest-first via `tac`; we sort
    // explicitly so the file order doesn't matter. The limit keeps the
    // newest entries because it is applied after this sort.
    entries.sort_by_key(|e| std::cmp::Reverse(e.ts));
    entries.truncate(limit);
    entries
}

/// Render an epoch-second timestamp as a short relative age string
/// (`12s`, `4m`, `2h`, `3d`). Matches the `human_age` helper in
/// `attention-picker.sh` for visual consistency.
pub fn human_age(ts: i64, now: i64) -> String {
    let secs = (now - ts).max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ts: i64, kind: &str, body: &str) -> String {
        format!(
            r#"{{"ts":{ts},"kind":"{kind}","source":"claude","title":"t","body":"{body}","session_id":"sid","cwd":"/x","tmux_session":"s","tmux_window_idx":"0","tmux_window_name":"n","tmux_target":"s:0","tmux_socket":"","tmux_bin":"","tmux_client_pid":"","terminal_pid":""}}"#
        )
    }

    #[test]
    fn parse_recent_drops_entries_below_cutoff() {
        let now: i64 = 10_000;
        let cutoff = now - 3600;
        let raw = format!(
            "{}\n{}\n{}\n",
            line(now - 60, "notification", "fresh"),
            line(now - 7200, "stop", "stale"),
            line(now - 300, "codex-turn", "also-fresh"),
        );
        let entries = parse_recent(raw.as_bytes(), cutoff, usize::MAX);
        assert_eq!(entries.len(), 2);
        // Newest first
        assert_eq!(entries[0].body, "fresh");
        assert_eq!(entries[1].body, "also-fresh");
    }

    #[test]
    fn parse_recent_limit_keeps_newest() {
        let raw = format!(
            "{}\n{}\n{}\n",
            line(100, "stop", "oldest"),
            line(300, "stop", "newest"),
            line(200, "stop", "middle"),
        );
        let entries = parse_recent(raw.as_bytes(), 0, 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].body, "newest");
        assert_eq!(entries[1].body, "middle");
    }

    #[test]
    fn parse_recent_limit_zero_hides_all() {
        let raw = line(100, "stop", "x");
        assert!(parse_recent(raw.as_bytes(), 0, 0).is_empty());
    }

    #[test]
    fn read_tail_small_file_reads_everything() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("queue.jsonl");
        std::fs::write(&path, "line1\nline2\n").unwrap();
        assert_eq!(read_tail(&path).unwrap(), b"line1\nline2\n");
    }

    #[test]
    fn read_tail_large_file_drops_partial_first_line() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("queue.jsonl");
        // Pad well past TAIL_READ_BYTES so the read starts mid-line, then
        // append complete lines that must survive.
        let mut content = vec![b'x'; (TAIL_READ_BYTES + 1024) as usize];
        content.extend_from_slice(b"\nkeep1\nkeep2\n");
        std::fs::write(&path, &content).unwrap();
        let tail = read_tail(&path).unwrap();
        // The padding line was entered mid-stream → dropped up to its `\n`.
        assert_eq!(tail, b"keep1\nkeep2\n");
    }

    #[test]
    fn read_tail_window_at_exact_line_boundary_keeps_first_record() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("queue.jsonl");
        // Lay the file out so the nominal window begins exactly at a line
        // start: the complete first record must survive, not be dropped as
        // a "partial" line.
        let mut tail_block = b"first\n".to_vec();
        tail_block.extend(vec![b'z'; TAIL_READ_BYTES as usize - tail_block.len() - 6]);
        tail_block.extend_from_slice(b"\nlast\n");
        assert_eq!(tail_block.len(), TAIL_READ_BYTES as usize);
        let mut content = vec![b'x'; 50];
        content.push(b'\n');
        content.extend_from_slice(&tail_block);
        std::fs::write(&path, &content).unwrap();
        assert_eq!(read_tail(&path).unwrap(), tail_block);
    }

    #[test]
    fn read_tail_window_without_newline_yields_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("queue.jsonl");
        // One giant line larger than the window: everything in the window
        // is a partial line, so nothing usable remains.
        let content = vec![b'x'; (TAIL_READ_BYTES + 1024) as usize];
        std::fs::write(&path, &content).unwrap();
        assert_eq!(read_tail(&path).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn parse_recent_tolerates_malformed_lines() {
        let raw = b"\
{\"ts\":100,\"kind\":\"notification\",\"body\":\"ok\"}
not-json
{\"ts\":200,\"kind\":\"stop\",\"body\":\"ok2\"}
"
        .to_vec();
        let entries = parse_recent(&raw, 0, usize::MAX);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].body, "ok2");
        assert_eq!(entries[1].body, "ok");
    }

    #[test]
    fn parse_recent_empty_returns_empty() {
        assert!(parse_recent(b"", 0, usize::MAX).is_empty());
        assert!(parse_recent(b"\n\n", 0, usize::MAX).is_empty());
    }

    #[test]
    fn parse_recent_missing_ts_skipped() {
        // ts is required (no #[serde(default)]); entries without it
        // should not appear.
        let raw = br#"{"kind":"stop","body":"no-ts"}
{"ts":42,"kind":"stop","body":"ok"}
"#;
        let entries = parse_recent(raw, 0, usize::MAX);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].body, "ok");
    }

    #[test]
    fn human_age_buckets() {
        assert_eq!(human_age(0, 30), "30s");
        assert_eq!(human_age(0, 120), "2m");
        assert_eq!(human_age(0, 7200), "2h");
        assert_eq!(human_age(0, 200_000), "2d");
        // Future timestamp (clock skew) → 0 floor.
        assert_eq!(human_age(100, 0), "0s");
    }

    #[test]
    fn queue_path_respects_xdg_cache_home() {
        let saved = std::env::var("XDG_CACHE_HOME").ok();
        // SAFETY: tests are single-threaded for this module since the
        // env var is scoped to this body and restored on exit. (Cargo
        // runs tests in parallel across modules; this var is unlikely
        // to be observed by another test.)
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", "/tmp/_tmx_test_cache");
        }
        let p = queue_path().unwrap();
        assert!(p.ends_with("claude-attention/queue.jsonl"));
        assert!(p.starts_with("/tmp/_tmx_test_cache"));
        // Restore.
        unsafe {
            match saved {
                Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
                None => std::env::remove_var("XDG_CACHE_HOME"),
            }
        }
    }
}
