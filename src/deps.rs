use std::ffi::OsStr;
use std::path::Path;

use anyhow::{Result, bail};

/// Fail fast with an actionable message when a required external binary is
/// missing, instead of surfacing a raw IO error at the first spawn deep
/// inside a command.
pub fn require(name: &str) -> Result<()> {
    if on_path(name) {
        return Ok(());
    }
    bail!("'{name}' not found on PATH — tmx needs it for this command. Install {name} and retry.");
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| search_path(&paths, name))
        .unwrap_or(false)
}

fn search_path(paths: &OsStr, name: &str) -> bool {
    std::env::split_paths(paths).any(|dir| is_executable(&dir.join(name)))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn make_executable(dir: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn search_path_finds_executable() {
        let dir = TempDir::new().unwrap();
        make_executable(dir.path(), "sometool");
        let paths = std::env::join_paths([dir.path()]).unwrap();
        assert!(search_path(&paths, "sometool"));
        assert!(!search_path(&paths, "othertool"));
    }

    #[test]
    #[cfg(unix)]
    fn search_path_ignores_non_executable_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("plainfile"), "data").unwrap();
        let paths = std::env::join_paths([dir.path()]).unwrap();
        assert!(!search_path(&paths, "plainfile"));
    }

    #[test]
    #[cfg(unix)]
    fn search_path_ignores_directory_with_matching_name() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("tooldir")).unwrap();
        let paths = std::env::join_paths([dir.path()]).unwrap();
        assert!(!search_path(&paths, "tooldir"));
    }

    #[test]
    fn require_missing_binary_reports_name() {
        let err = require("definitely-not-a-real-binary-xyz").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("definitely-not-a-real-binary-xyz"));
        assert!(msg.contains("not found on PATH"));
    }

    #[test]
    fn require_present_binary_succeeds() {
        // `sh` is guaranteed on every supported target (macOS, Linux).
        assert!(require("sh").is_ok());
    }
}
