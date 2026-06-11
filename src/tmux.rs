use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Session {
    pub name: String,
    pub windows: u32,
    pub attached: bool,
}

pub fn list_sessions() -> io::Result<Vec<Session>> {
    let output = Command::new("tmux")
        .args([
            "list-sessions",
            "-F",
            "#{session_name}:#{session_windows}:#{session_attached}",
        ])
        .output()?;

    if !output.status.success() {
        return Err(io::Error::other(format!(
            "tmux list-sessions failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_sessions(&stdout))
}

pub fn parse_sessions(stdout: &str) -> Vec<Session> {
    stdout
        .trim()
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(parse_session_line)
        .collect()
}

fn parse_session_line(line: &str) -> Option<Session> {
    let mut parts = line.splitn(3, ':');
    let name = parts.next()?.to_string();
    let windows = parts.next()?.parse().ok()?;
    let attached_raw = parts.next()?;
    Some(Session {
        name,
        windows,
        attached: attached_raw == "1",
    })
}

pub fn session_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn create_session(name: &str, path: &str) -> io::Result<()> {
    let mut cmd = Command::new("tmux");
    cmd.args(["new-session", "-d", "-s", name]);
    if !path.is_empty() {
        cmd.args(["-c", path]);
    }
    let status = cmd.status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "tmux new-session exited with status {status}"
        )));
    }
    Ok(())
}

pub fn switch_session(name: &str) -> io::Result<()> {
    // Inside tmux: switch the current client.
    if std::env::var_os("TMUX").is_some() {
        let status = Command::new("tmux")
            .args(["switch-client", "-t", name])
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "tmux switch-client exited with status {status}"
            )));
        }
        return Ok(());
    }
    // Otherwise attach — inherit stdio so the user sees the session.
    let status = Command::new("tmux")
        .args(["attach-session", "-t", name])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "tmux attach-session exited with status {status}"
        )));
    }
    Ok(())
}

/// Name of the throwaway session [`boot_and_restore`] spawns to start the
/// server. Callers must filter this out of any session list shown to the user,
/// since kill-session cleanup is best-effort.
pub const BOOTSTRAP_SESSION: &str = "_tmx_boot";

/// Cold-start helper: when no tmux server is running, launching a throwaway
/// detached session starts the server, which sources `~/.tmux.conf` and lets
/// tmux-continuum auto-restore the previously saved environment in the
/// background. continuum only triggers a restore when the server has just
/// started (within `@continuum-restore-max-delay`, default 10s), so `tmx`
/// itself must be what boots the server — otherwise the restore window never
/// opens. We poll until the restored session count settles, then drop the
/// bootstrap session so it never shows up in the switcher.
pub fn boot_and_restore() -> io::Result<()> {
    let status = Command::new("tmux")
        .args(["new-session", "-d", "-s", BOOTSTRAP_SESSION])
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "tmux new-session (bootstrap) exited with status {status}"
        )));
    }

    // continuum_restore.sh sleeps 1s, then recreates every saved session. Use
    // the number of sessions in the most recent resurrect save as a
    // deterministic completion target — wait until that many have come back
    // (plus our bootstrap), rather than guessing from count stability. The
    // restore process is independent of the bootstrap session, so the timeout
    // is only a safety cap, not a hard cutoff of the restore itself.
    match saved_session_count() {
        Some(target) => {
            // `> target` == "all `target` restored sessions plus our bootstrap".
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if list_sessions().map(|s| s.len()).unwrap_or(0) > target {
                    break;
                }
            }
        }
        // No save to count against (e.g. first-ever run): nothing deterministic
        // to wait for, so just give any in-flight restore a brief moment.
        None => std::thread::sleep(std::time::Duration::from_millis(1500)),
    }

    let _ = Command::new("tmux")
        .args(["kill-session", "-t", BOOTSTRAP_SESSION])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

/// Number of distinct sessions in the most recent tmux-resurrect save, used as
/// the completion target for a continuum restore. Best-effort: returns `None`
/// if the save file can't be located or read.
fn saved_session_count() -> Option<usize> {
    let last = resurrect_dir()?.join("last");
    let content = std::fs::read_to_string(last).ok()?;
    let count = count_saved_sessions(&content);
    (count > 0).then_some(count)
}

/// Count distinct session names in a tmux-resurrect save. Sessions are the
/// field-2 values of `pane`/`window` records; other record types
/// (`state`, `grouped_session`) are ignored.
fn count_saved_sessions(content: &str) -> usize {
    content
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            match fields.next()? {
                "pane" | "window" => fields.next(),
                _ => None,
            }
        })
        .collect::<std::collections::HashSet<&str>>()
        .len()
}

/// Resolve the tmux-resurrect save directory, mirroring its `helpers.sh`:
/// honor `@resurrect-dir`, else legacy `~/.tmux/resurrect` if it exists, else
/// `$XDG_DATA_HOME/tmux/resurrect` (XDG default `~/.local/share`).
fn resurrect_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;

    if let Some(opt) = tmux_global_option("@resurrect-dir") {
        let expanded = match opt.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(opt),
        };
        return Some(expanded);
    }

    let legacy = home.join(".tmux/resurrect");
    if legacy.is_dir() {
        return Some(legacy);
    }

    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    Some(base.join("tmux/resurrect"))
}

/// Read a global tmux option (`show-options -gqv`), returning `None` when unset.
fn tmux_global_option(name: &str) -> Option<String> {
    let output = Command::new("tmux")
        .args(["show-options", "-gqv", name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Capture the visible content of a single tmux pane as plain text.
/// `target` is the `session:window.pane` form. ANSI escapes are stripped
/// by tmux (no `-e`), which is what we want for pattern matching.
pub fn capture_pane(target: &str) -> io::Result<String> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-t", target])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "tmux capture-pane -t {target} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Switch the current client to a specific pane (`session:window.pane`).
/// `switch-client -t session:window.pane` is unreliable across tmux versions
/// for selecting the window/pane within the session, so we do the dance
/// explicitly: select-pane first (which carries window selection), then
/// switch the client. All errors are surfaced.
pub fn switch_to_pane(target: &str) -> io::Result<()> {
    let session = target.split(':').next().unwrap_or(target);
    let status = Command::new("tmux")
        .args(["select-pane", "-t", target])
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "tmux select-pane -t {target} exited with status {status}"
        )));
    }
    if std::env::var_os("TMUX").is_some() {
        let status = Command::new("tmux")
            .args(["switch-client", "-t", session])
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "tmux switch-client -t {session} exited with status {status}"
            )));
        }
    } else {
        let status = Command::new("tmux")
            .args(["attach-session", "-t", session])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "tmux attach-session -t {session} exited with status {status}"
            )));
        }
    }
    Ok(())
}

pub fn kill_session(name: &str) -> io::Result<()> {
    let status = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "tmux kill-session exited with status {status}"
        )));
    }
    Ok(())
}

/// Kill a session by its stable id (e.g. `$3`). Safer than by name when
/// sessions may be renamed concurrently.
pub fn kill_session_id(id: &str) -> io::Result<()> {
    let status = Command::new("tmux")
        .args(["kill-session", "-t", id])
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "tmux kill-session exited with status {status}"
        )));
    }
    Ok(())
}

/// Return the current tmux session id (e.g. `$3`), or `None` if not inside
/// a tmux server or the query fails.
pub fn current_session_id() -> Option<String> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_id}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if id.is_empty() { None } else { Some(id) }
}

pub fn apply_layout(session_name: &str, _project_type: &str) -> io::Result<()> {
    Command::new("tmux")
        .args([
            "rename-window",
            "-t",
            &format!("{session_name}:0"),
            "editor",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok();
    Ok(())
}

pub fn cleanup_sessions() -> io::Result<Vec<String>> {
    let sessions = list_sessions()?;
    let mut killed = Vec::new();
    for session in sessions {
        if !session.attached && kill_session(&session.name).is_ok() {
            killed.push(session.name);
        }
    }
    Ok(killed)
}

/// Normalise a tmux session name. Mirrors the convention used elsewhere
/// (dots and slashes become safe characters).
pub fn safe_session_name(repo: &str, branch: &str) -> String {
    let combined = format!("{repo}-{branch}");
    combined.replace('/', "-").replace('.', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sessions_handles_multiple_lines() {
        let input = "main:3:1\nside:1:0\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0],
            Session {
                name: "main".into(),
                windows: 3,
                attached: true
            }
        );
        assert_eq!(
            sessions[1],
            Session {
                name: "side".into(),
                windows: 1,
                attached: false
            }
        );
    }

    #[test]
    fn parse_sessions_handles_session_with_colons_in_name() {
        // splitn(3) means trailing colons stay in the windows/attached fields,
        // not the name. The Go version behaves identically.
        let input = "weird:name:5:1";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 0); // "name" is not a valid u32 → dropped
    }

    #[test]
    fn parse_sessions_skips_empty_lines() {
        let input = "\nfoo:1:0\n\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "foo");
    }

    #[test]
    fn parse_sessions_empty_input() {
        assert!(parse_sessions("").is_empty());
        assert!(parse_sessions("\n\n").is_empty());
    }

    #[test]
    fn safe_session_name_replaces_special_chars() {
        assert_eq!(safe_session_name("my.repo", "feat/x"), "my_repo-feat-x");
        assert_eq!(safe_session_name("plain", "main"), "plain-main");
    }

    #[test]
    fn count_saved_sessions_counts_distinct_pane_window_names() {
        let save = "\
pane\tbrowser\t0\t1\t:*\t0\ttitle\t:/home/u/browser\t1\tzsh\t:
pane\tbrowser\t1\t0\t:\t0\ttitle\t:/home/u/browser\t1\tzsh\t:
window\tdocs\t0\t:*\tlayout
pane\tdocs\t0\t1\t:*\t0\ttitle\t:/home/u/docs\t1\tzsh\t:
state\tbrowser\t/home/u/browser
grouped_session\tfoo\tbar";
        // browser + docs == 2; state / grouped_session ignored.
        assert_eq!(count_saved_sessions(save), 2);
    }

    #[test]
    fn count_saved_sessions_empty_or_irrelevant() {
        assert_eq!(count_saved_sessions(""), 0);
        assert_eq!(
            count_saved_sessions("state\tx\ty\ngrouped_session\ta\tb"),
            0
        );
    }
}
