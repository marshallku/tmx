use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{Config, expand_home};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub branch: String,
    pub from: String,
    /// Working directory to resolve the source repo from; empty → cwd.
    pub start_in: String,
}

#[derive(Debug, Clone)]
pub struct CreateResult {
    pub repo_root: PathBuf,
    pub repo_name: String,
    pub worktree_path: PathBuf,
    /// Post-create script that was executed (empty if none).
    pub script_ran: String,
}

/// Run the full worktree-creation flow: resolve the source repo, compute the
/// sibling path, run `git worktree add`, and execute the matching post-create
/// script. The caller (CLI layer) is responsible for any tmux integration.
pub fn create(opts: Options, cfg: &Config) -> Result<CreateResult> {
    if opts.branch.trim().is_empty() {
        bail!("branch name is required");
    }

    let start_in = if opts.start_in.is_empty() {
        std::env::current_dir().context("resolve cwd")?
    } else {
        PathBuf::from(&opts.start_in)
    };

    let repo_root = resolve_repo_root(&start_in)?;
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| anyhow!("could not determine repo name from {}", repo_root.display()))?;

    let dir_name = render_naming(&cfg.worktree.naming, &repo_name, &opts.branch);
    let worktree_path = repo_root
        .parent()
        .map(|p| p.join(&dir_name))
        .ok_or_else(|| anyhow!("repo root has no parent: {}", repo_root.display()))?;

    if worktree_path.exists() {
        bail!("target path already exists: {}", worktree_path.display());
    }

    run_git_worktree_add(&repo_root, &worktree_path, &opts.branch, &opts.from)?;

    let mut result = CreateResult {
        repo_root: repo_root.clone(),
        repo_name,
        worktree_path: worktree_path.clone(),
        script_ran: String::new(),
    };

    if let Some(script) = cfg
        .worktree
        .scripts
        .get(repo_root.to_string_lossy().as_ref())
        && !script.is_empty()
    {
        run_post_create(script, &worktree_path).context("post_create script failed")?;
        result.script_ran = script.clone();
    }

    Ok(result)
}

pub fn resolve_repo_root(start_in: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(start_in)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| format!("invoking git in {}", start_in.display()))?;

    if !output.status.success() {
        bail!("not inside a git repository: {}", start_in.display());
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(path))
}

pub fn render_naming(pattern: &str, repo: &str, branch: &str) -> String {
    let effective = if pattern.is_empty() {
        "{repo}-{branch}"
    } else {
        pattern
    };
    let safe_branch = branch.replace('/', "-");
    effective
        .replace("{repo}", repo)
        .replace("{branch}", &safe_branch)
}

fn run_git_worktree_add(
    repo_root: &Path,
    worktree_path: &Path,
    branch: &str,
    from: &str,
) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_root)
        .args(["worktree", "add", "-b", branch])
        .arg(worktree_path);
    if !from.is_empty() {
        cmd.arg(from);
    }
    let output = cmd.output().context("invoking git worktree add")?;
    // Mirror Go's behaviour: git's stdout + stderr both go to our stderr so the
    // worktree path printed on stdout stays clean for `cd $(...)` callers.
    let mut err = std::io::stderr().lock();
    err.write_all(&output.stdout).ok();
    err.write_all(&output.stderr).ok();
    if !output.status.success() {
        bail!("git worktree add failed (status {})", output.status);
    }
    Ok(())
}

fn run_post_create(script: &str, worktree_path: &Path) -> Result<()> {
    let resolved = resolve_script_path(script, worktree_path);
    let output = Command::new("bash")
        .arg(&resolved)
        .current_dir(worktree_path)
        .env("WORKTREE_PATH", worktree_path)
        .output()
        .with_context(|| format!("invoking script {}", resolved.display()))?;
    let mut err = std::io::stderr().lock();
    err.write_all(&output.stdout).ok();
    err.write_all(&output.stderr).ok();
    if !output.status.success() {
        bail!("post-create script exited with status {}", output.status);
    }
    Ok(())
}

pub fn resolve_script_path(script: &str, worktree_path: &Path) -> PathBuf {
    let expanded = expand_home(script);
    let expanded_path = PathBuf::from(&expanded);
    if expanded_path.is_absolute() {
        return expanded_path;
    }
    worktree_path.join(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn init_repo(path: &Path) {
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("git command")
        };
        std::fs::create_dir_all(path).unwrap();
        assert!(run(&["init", "-q", "-b", "main"]).success());
        assert!(run(&["config", "user.email", "test@example.com"]).success());
        assert!(run(&["config", "user.name", "Test"]).success());
        assert!(run(&["commit", "--allow-empty", "-m", "init"]).success());
    }

    #[test]
    fn render_naming_default_pattern() {
        assert_eq!(render_naming("", "repo", "feat"), "repo-feat");
    }

    #[test]
    fn render_naming_replaces_slashes_in_branch() {
        assert_eq!(
            render_naming("{repo}-{branch}", "repo", "feat/x"),
            "repo-feat-x"
        );
    }

    #[test]
    fn render_naming_custom_pattern() {
        assert_eq!(
            render_naming("wt_{repo}_{branch}_v2", "my-project", "feat/cool"),
            "wt_my-project_feat-cool_v2"
        );
    }

    #[test]
    fn resolve_script_path_absolute_is_kept() {
        let wt = Path::new("/tmp/worktree");
        assert_eq!(
            resolve_script_path("/etc/script.sh", wt),
            PathBuf::from("/etc/script.sh")
        );
    }

    #[test]
    fn resolve_script_path_relative_resolves_against_worktree() {
        let wt = Path::new("/tmp/worktree");
        assert_eq!(
            resolve_script_path("scripts/setup.sh", wt),
            PathBuf::from("/tmp/worktree/scripts/setup.sh")
        );
    }

    #[test]
    fn resolve_script_path_tilde_expands() {
        let wt = Path::new("/tmp/worktree");
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            resolve_script_path("~/scripts/setup.sh", wt),
            home.join("scripts/setup.sh")
        );
    }

    #[test]
    fn create_rejects_empty_branch() {
        let cfg = Config::default();
        let err = create(
            Options {
                branch: "  ".into(),
                ..Default::default()
            },
            &cfg,
        )
        .unwrap_err();
        assert!(err.to_string().contains("branch name is required"));
    }

    #[test]
    fn create_rejects_non_repo_path() {
        if !git_available() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let err = create(
            Options {
                branch: "feat".into(),
                start_in: dir.path().to_string_lossy().into_owned(),
                ..Default::default()
            },
            &cfg,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not inside a git repository"));
    }

    #[test]
    fn create_makes_worktree_and_runs_script() {
        if !git_available() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("source-repo");
        init_repo(&repo_root);

        let script_path = repo_root.join("setup.sh");
        let marker = dir.path().join("ran.txt");
        std::fs::write(
            &script_path,
            format!(
                "#!/usr/bin/env bash\necho ok > {}\n",
                marker.to_string_lossy()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        let mut scripts = HashMap::new();
        scripts.insert(
            repo_root.to_string_lossy().into_owned(),
            script_path.to_string_lossy().into_owned(),
        );
        let cfg = Config {
            roots: vec![],
            worktree: crate::config::WorktreeConfig {
                naming: "{repo}-{branch}".into(),
                scripts,
            },
        };

        let res = create(
            Options {
                branch: "feat/x".into(),
                start_in: repo_root.to_string_lossy().into_owned(),
                ..Default::default()
            },
            &cfg,
        )
        .unwrap();

        let expected_wt = dir.path().join("source-repo-feat-x");
        assert_eq!(res.worktree_path, expected_wt);
        assert!(expected_wt.exists());
        assert_eq!(res.repo_name, "source-repo");
        assert_eq!(res.script_ran, script_path.to_string_lossy());
        assert!(marker.exists());
    }

    #[test]
    fn create_rejects_existing_target() {
        if !git_available() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("source-repo");
        init_repo(&repo_root);
        std::fs::create_dir(dir.path().join("source-repo-feat")).unwrap();

        let cfg = Config::default();
        let err = create(
            Options {
                branch: "feat".into(),
                start_in: repo_root.to_string_lossy().into_owned(),
                ..Default::default()
            },
            &cfg,
        )
        .unwrap_err();
        assert!(err.to_string().contains("target path already exists"));
    }
}
