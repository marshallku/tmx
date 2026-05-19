use std::io;
use std::path::PathBuf;
use std::process::Command;

/// One row from `tmux list-panes -a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub session: String,
    pub window: u32,
    pub pane: u32,
    pub pane_pid: u32,
    pub current_command: String,
    pub current_path: PathBuf,
}

/// Custom field separator. Pipe is the cleanest neutral char: not in shell
/// commands, not in standard cwd paths, and not used by tmux's own format
/// language for anything we touch here.
const SEP: char = '\u{1f}'; // ASCII Unit Separator — guaranteed not in paths/cmds.

pub fn list_panes() -> io::Result<Vec<PaneInfo>> {
    let fmt = format!(
        "#{{session_name}}{0}#{{window_index}}{0}#{{pane_index}}{0}#{{pane_pid}}{0}#{{pane_current_command}}{0}#{{pane_current_path}}",
        SEP,
    );
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", &fmt])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "tmux list-panes failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_panes(&stdout))
}

pub fn parse_panes(stdout: &str) -> Vec<PaneInfo> {
    stdout
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(parse_pane_line)
        .collect()
}

fn parse_pane_line(line: &str) -> Option<PaneInfo> {
    let mut parts = line.splitn(6, SEP);
    let session = parts.next()?.to_string();
    let window = parts.next()?.parse().ok()?;
    let pane = parts.next()?.parse().ok()?;
    let pane_pid = parts.next()?.parse().ok()?;
    let current_command = parts.next()?.to_string();
    let current_path = PathBuf::from(parts.next()?);
    Some(PaneInfo {
        session,
        window,
        pane,
        pane_pid,
        current_command,
        current_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(parts: &[&str]) -> String {
        parts.join(&SEP.to_string())
    }

    #[test]
    fn parse_panes_single_row() {
        let input = line(&["main", "0", "0", "12345", "claude", "/home/me/dev/x"]);
        let panes = parse_panes(&input);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].session, "main");
        assert_eq!(panes[0].window, 0);
        assert_eq!(panes[0].pane, 0);
        assert_eq!(panes[0].pane_pid, 12345);
        assert_eq!(panes[0].current_command, "claude");
        assert_eq!(panes[0].current_path, PathBuf::from("/home/me/dev/x"));
    }

    #[test]
    fn parse_panes_multiple_rows() {
        let input = format!(
            "{}\n{}\n",
            line(&["a", "0", "0", "1", "zsh", "/tmp"]),
            line(&["b", "1", "2", "9999", "claude", "/x/y"]),
        );
        let panes = parse_panes(&input);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[1].session, "b");
        assert_eq!(panes[1].window, 1);
        assert_eq!(panes[1].pane, 2);
        assert_eq!(panes[1].pane_pid, 9999);
    }

    #[test]
    fn parse_panes_skips_empty_and_malformed() {
        let input = format!(
            "\n{}\nnot-a-row\n{}\n",
            line(&["a", "0", "0", "1", "zsh", "/tmp"]),
            line(&["bad", "x", "0", "1", "zsh", "/tmp"]), // window not a u32
        );
        let panes = parse_panes(&input);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].session, "a");
    }

    #[test]
    fn parse_panes_tolerates_spaces_in_paths() {
        let input = line(&["a", "0", "0", "1", "claude", "/path with/spaces and stuff"]);
        let panes = parse_panes(&input);
        assert_eq!(panes.len(), 1);
        assert_eq!(
            panes[0].current_path,
            PathBuf::from("/path with/spaces and stuff")
        );
    }
}
