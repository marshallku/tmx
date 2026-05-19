//! Process tree walker. Given a tmux pane PID, find the most recently
//! spawned `claude` or `codex` descendant so we can read its cwd and
//! report it as the agent's working directory.
//!
//! `sysinfo` abstracts /proc on Linux and libproc on macOS, so the same
//! code path works on both. We refresh with a minimal `ProcessRefreshKind`
//! (only the fields we read) to keep the cost low — refreshing all
//! processes every tick is sub-millisecond on typical workstations.

use std::path::PathBuf;

use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

pub struct ProcSnapshot {
    sys: System,
}

impl ProcSnapshot {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, Self::refresh_kind());
        Self { sys }
    }

    pub fn refresh(&mut self) {
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, Self::refresh_kind());
    }

    fn refresh_kind() -> ProcessRefreshKind {
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::Always)
    }

    /// Walk descendants of `root_pid` (BFS) and return the most recently
    /// started one whose exe basename matches any of `targets`.
    pub fn find_descendant(&self, root_pid: u32, targets: &[&str]) -> Option<ProcInfo> {
        let mut best: Option<&Process> = None;
        let mut stack = vec![Pid::from_u32(root_pid)];

        // BFS over children. We don't pre-index parent→children, so we
        // iterate all processes once per visited PID. With <1000 procs
        // this is still well under a millisecond per call.
        while let Some(pid) = stack.pop() {
            for (other_pid, proc) in self.sys.processes() {
                if proc.parent() != Some(pid) {
                    continue;
                }
                if let Some(name) = exe_basename(proc)
                    && targets.contains(&name)
                {
                    best = match best {
                        None => Some(proc),
                        Some(prev) if proc.start_time() > prev.start_time() => Some(proc),
                        Some(_) => best,
                    };
                }
                stack.push(*other_pid);
            }
        }

        best.map(|p| ProcInfo {
            pid: p.pid().as_u32(),
            name: exe_basename(p).unwrap_or_default().to_string(),
            cwd: p.cwd().map(PathBuf::from),
        })
    }
}

impl Default for ProcSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cwd: Option<PathBuf>,
}

fn exe_basename(proc: &Process) -> Option<&str> {
    proc.exe()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        // Fall back to the process name when exe isn't readable (e.g.
        // privileged or short-lived). `name()` returns OsStr.
        .or_else(|| proc.name().to_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_finds_self_as_no_target() {
        // Smoke test: refresh succeeds and a known-bogus target returns None.
        let snap = ProcSnapshot::new();
        let me = std::process::id();
        assert!(
            snap.find_descendant(me, &["this_binary_does_not_exist"])
                .is_none()
        );
    }
}
