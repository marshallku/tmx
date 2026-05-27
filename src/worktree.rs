use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use crate::config::Config;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub locked: bool,
    pub prunable: bool,
    pub is_main: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneEntry {
    pub session_id: String,
    pub session_name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedSession {
    pub id: String,
    pub name: String,
}

/// Decision returned by the remove preflight. CLI layer is responsible for
/// applying it (printing messages, killing sessions, invoking git).
#[derive(Debug, PartialEq, Eq)]
pub enum RemoveDecision {
    LockedRefuse,
    Proceed,
}

pub fn remove_decision(entry: &WorktreeEntry) -> RemoveDecision {
    if entry.locked {
        RemoveDecision::LockedRefuse
    } else {
        RemoveDecision::Proceed
    }
}

/// Parse `git worktree list --porcelain` output. The first `worktree` block
/// is always the main worktree per git's documented contract.
pub fn parse_porcelain(stdout: &str) -> Vec<WorktreeEntry> {
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    let mut current: Option<WorktreeEntry> = None;

    let flush = |cur: &mut Option<WorktreeEntry>, out: &mut Vec<WorktreeEntry>| {
        if let Some(e) = cur.take() {
            out.push(e);
        }
    };

    for raw in stdout.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            flush(&mut current, &mut entries);
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            flush(&mut current, &mut entries);
            current = Some(WorktreeEntry {
                path: PathBuf::from(path),
                branch: None,
                head: None,
                locked: false,
                prunable: false,
                is_main: false,
            });
        } else if let Some(sha) = line.strip_prefix("HEAD ")
            && let Some(e) = current.as_mut()
        {
            e.head = Some(sha.to_string());
        } else if let Some(branch) = line.strip_prefix("branch ")
            && let Some(e) = current.as_mut()
        {
            e.branch = Some(branch.to_string());
        } else if line == "detached" {
            // No branch; leave entry.branch as None.
        } else if (line == "locked" || line.starts_with("locked "))
            && let Some(e) = current.as_mut()
        {
            e.locked = true;
        } else if (line == "prunable" || line.starts_with("prunable "))
            && let Some(e) = current.as_mut()
        {
            e.prunable = true;
        }
    }
    flush(&mut current, &mut entries);

    if let Some(first) = entries.first_mut() {
        first.is_main = true;
    }
    entries
}

/// Short branch name from `refs/heads/foo` → `foo`. Returns the original if
/// it doesn't carry the prefix.
pub fn short_branch(branch: &str) -> &str {
    branch.strip_prefix("refs/heads/").unwrap_or(branch)
}

pub fn list_entries(start_in: &Path) -> Result<Vec<WorktreeEntry>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(start_in)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .with_context(|| format!("invoking git in {}", start_in.display()))?;
    if !output.status.success() {
        bail!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse `tmux list-panes` output formatted as
/// `#{session_id}\t#{session_name}\t#{pane_current_path}`.
pub fn parse_pane_lines(stdout: &str) -> Vec<PaneEntry> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let id = parts.next()?.to_string();
            let name = parts.next()?.to_string();
            let path = parts.next()?.to_string();
            if path.is_empty() {
                return None;
            }
            Some(PaneEntry {
                session_id: id,
                session_name: name,
                path,
            })
        })
        .collect()
}

/// True if `pane_path` is `worktree` itself or a descendant, with `/` boundary.
fn pane_path_inside(pane_path: &str, worktree: &Path) -> bool {
    let wt = worktree.to_string_lossy();
    let wt = wt.trim_end_matches('/');
    if pane_path == wt {
        return true;
    }
    if let Some(rest) = pane_path.strip_prefix(wt)
        && rest.starts_with('/')
    {
        return true;
    }
    false
}

/// Returns offenders (sessions whose any pane is inside `worktree`),
/// de-duplicated by session id, in first-seen order.
pub fn match_attached(panes: &[PaneEntry], worktree: &Path) -> Vec<AttachedSession> {
    // Try canonicalized comparison if possible; otherwise fall back to raw.
    let wt_canon = worktree.canonicalize().ok();
    let mut seen_ids: Vec<String> = Vec::new();
    let mut out: Vec<AttachedSession> = Vec::new();
    for p in panes {
        let pane_canon = PathBuf::from(&p.path).canonicalize().ok();
        let matches = match (&wt_canon, &pane_canon) {
            (Some(w), Some(pc)) => pane_path_inside(&pc.to_string_lossy(), w),
            _ => pane_path_inside(&p.path, worktree),
        };
        if matches && !seen_ids.iter().any(|id| id == &p.session_id) {
            seen_ids.push(p.session_id.clone());
            out.push(AttachedSession {
                id: p.session_id.clone(),
                name: p.session_name.clone(),
            });
        }
    }
    out
}

/// Shell out to tmux. Returns Err on command failure so caller can apply
/// fail-open / fail-closed policy based on `$TMUX`.
pub fn sessions_attached_to(worktree: &Path) -> Result<Vec<AttachedSession>> {
    let output = Command::new("tmux")
        .args([
            "list-panes",
            "-as",
            "-F",
            "#{session_id}\t#{session_name}\t#{pane_current_path}",
        ])
        .output()
        .context("invoking tmux list-panes")?;
    if !output.status.success() {
        bail!(
            "tmux list-panes failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let panes = parse_pane_lines(&stdout);
    Ok(match_attached(&panes, worktree))
}

/// Find the unique non-main entry matching `target` (exact path or short branch).
pub fn resolve_target<'a>(entries: &'a [WorktreeEntry], target: &str) -> Result<&'a WorktreeEntry> {
    resolve_target_inner(entries, target, false)
}

/// Same matching rules as `resolve_target` but includes the main worktree.
/// Used by read-only commands like `worktree list` where the main worktree is
/// a valid target.
pub fn resolve_target_any<'a>(
    entries: &'a [WorktreeEntry],
    target: &str,
) -> Result<&'a WorktreeEntry> {
    resolve_target_inner(entries, target, true)
}

fn resolve_target_inner<'a>(
    entries: &'a [WorktreeEntry],
    target: &str,
    include_main: bool,
) -> Result<&'a WorktreeEntry> {
    let target_norm = target.trim_end_matches('/');
    let target_canon = PathBuf::from(target).canonicalize().ok();

    let mut matches: Vec<&WorktreeEntry> = Vec::new();
    for e in entries.iter().filter(|e| include_main || !e.is_main) {
        let mut hit = false;

        let entry_str = e.path.to_string_lossy();
        let entry_norm = entry_str.trim_end_matches('/');
        if entry_norm == target_norm {
            hit = true;
        }
        if !hit
            && let (Some(t), Some(ec)) = (&target_canon, e.path.canonicalize().ok())
            && t == &ec
        {
            hit = true;
        }
        if !hit
            && let Some(b) = e.branch.as_deref()
            && short_branch(b) == target
        {
            hit = true;
        }
        if hit && !matches.iter().any(|m| std::ptr::eq(*m, e)) {
            matches.push(e);
        }
    }

    match matches.len() {
        0 => bail!("no worktree matches '{target}'"),
        1 => Ok(matches[0]),
        _ => {
            let paths: Vec<String> = matches
                .iter()
                .map(|m| m.path.display().to_string())
                .collect();
            bail!("'{target}' is ambiguous, matches: {}", paths.join(", "))
        }
    }
}

/// Run `git worktree remove [--force] <path>`. Surfaces git's stderr verbatim.
pub fn remove(worktree_path: &Path, force: bool) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.args(["worktree", "remove"]);
    if force {
        cmd.arg("--force");
    }
    cmd.arg(worktree_path);
    let output = cmd.output().context("invoking git worktree remove")?;
    let mut err = std::io::stderr().lock();
    err.write_all(&output.stdout).ok();
    err.write_all(&output.stderr).ok();
    if !output.status.success() {
        bail!("git worktree remove failed (status {})", output.status);
    }
    Ok(())
}

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

    if let Some(command) = cfg
        .worktree
        .scripts
        .get(repo_root.to_string_lossy().as_ref())
        && !command.is_empty()
    {
        run_post_create(command, &worktree_path).context("post_create command failed")?;
        result.script_ran = command.clone();
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

fn run_post_create(command: &str, worktree_path: &Path) -> Result<()> {
    let output = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(worktree_path)
        .env("WORKTREE_PATH", worktree_path)
        .output()
        .with_context(|| format!("invoking post-create command: {command}"))?;
    let mut err = std::io::stderr().lock();
    err.write_all(&output.stdout).ok();
    err.write_all(&output.stderr).ok();
    if !output.status.success() {
        bail!("post-create command exited with status {}", output.status);
    }
    Ok(())
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
    fn create_makes_worktree_and_runs_inline_command() {
        if !git_available() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("source-repo");
        init_repo(&repo_root);

        let marker = dir.path().join("ran.txt");
        let command = format!("echo ok > {}", marker.to_string_lossy());

        let mut scripts = HashMap::new();
        scripts.insert(repo_root.to_string_lossy().into_owned(), command.clone());
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
        assert_eq!(res.script_ran, command);
        assert!(marker.exists());
    }

    #[test]
    fn create_runs_chained_shell_command() {
        if !git_available() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("source-repo");
        init_repo(&repo_root);

        let marker_a = dir.path().join("a.txt");
        let marker_b = dir.path().join("b.txt");
        let command = format!(
            "echo a > {} && echo b > {}",
            marker_a.to_string_lossy(),
            marker_b.to_string_lossy()
        );

        let mut scripts = HashMap::new();
        scripts.insert(repo_root.to_string_lossy().into_owned(), command);
        let cfg = Config {
            roots: vec![],
            worktree: crate::config::WorktreeConfig {
                naming: "{repo}-{branch}".into(),
                scripts,
            },
        };

        create(
            Options {
                branch: "feat/chain".into(),
                start_in: repo_root.to_string_lossy().into_owned(),
                ..Default::default()
            },
            &cfg,
        )
        .unwrap();

        assert!(marker_a.exists());
        assert!(marker_b.exists());
    }

    #[test]
    fn create_runs_command_in_worktree_cwd() {
        if !git_available() {
            return;
        }
        let dir = TempDir::new().unwrap();
        let repo_root = dir.path().join("source-repo");
        init_repo(&repo_root);

        // Write to a relative path; success proves cwd is the new worktree.
        let mut scripts = HashMap::new();
        scripts.insert(
            repo_root.to_string_lossy().into_owned(),
            "echo ok > here.txt".into(),
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
                branch: "feat/cwd".into(),
                start_in: repo_root.to_string_lossy().into_owned(),
                ..Default::default()
            },
            &cfg,
        )
        .unwrap();

        assert!(res.worktree_path.join("here.txt").exists());
    }

    #[test]
    fn parse_porcelain_marks_first_as_main_and_extracts_fields() {
        let input = "\
worktree /home/user/repo
HEAD abc123
branch refs/heads/main

worktree /home/user/repo-feat
HEAD def456
branch refs/heads/feat

worktree /home/user/repo-detached
HEAD ff00ff
detached

worktree /home/user/repo-locked
HEAD 111111
branch refs/heads/lk
locked needed offline

worktree /home/user/repo-prunable
HEAD 222222
branch refs/heads/old
prunable gitdir file points to nonexistent location
";
        let v = parse_porcelain(input);
        assert_eq!(v.len(), 5);
        assert!(v[0].is_main);
        assert!(!v[1].is_main);
        assert_eq!(v[1].branch.as_deref(), Some("refs/heads/feat"));
        assert_eq!(v[2].branch, None);
        assert!(v[3].locked);
        assert!(v[4].prunable);
        assert_eq!(v[0].head.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_porcelain_handles_locked_without_reason() {
        let input = "worktree /a\nHEAD aa\nlocked\n";
        let v = parse_porcelain(input);
        assert_eq!(v.len(), 1);
        assert!(v[0].locked);
    }

    #[test]
    fn short_branch_strips_refs_heads() {
        assert_eq!(short_branch("refs/heads/feat-x"), "feat-x");
        assert_eq!(short_branch("feat-x"), "feat-x");
    }

    #[test]
    fn parse_pane_lines_splits_on_first_two_tabs() {
        let input = "\
$0\tmain\t/home/me/a
$1\tfoo\t/home/me/b
$2\tbar\t/path/with\ttab/in/it
$3\tempty\t
";
        let v = parse_pane_lines(input);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].session_id, "$0");
        assert_eq!(v[0].session_name, "main");
        assert_eq!(v[0].path, "/home/me/a");
        // path with embedded tab: only the first two tabs are split, the rest stays.
        assert_eq!(v[2].path, "/path/with\ttab/in/it");
    }

    #[test]
    fn match_attached_finds_direct_and_descendant_paths() {
        let panes = vec![
            PaneEntry {
                session_id: "$0".into(),
                session_name: "a".into(),
                path: "/tmp/wt".into(),
            },
            PaneEntry {
                session_id: "$1".into(),
                session_name: "b".into(),
                path: "/tmp/wt/sub".into(),
            },
            PaneEntry {
                session_id: "$2".into(),
                session_name: "c".into(),
                path: "/tmp/wt-other".into(),
            },
        ];
        let attached = match_attached(&panes, Path::new("/tmp/wt"));
        let ids: Vec<&str> = attached.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["$0", "$1"]);
    }

    #[test]
    fn match_attached_dedups_by_session_id() {
        let panes = vec![
            PaneEntry {
                session_id: "$0".into(),
                session_name: "a".into(),
                path: "/tmp/wt".into(),
            },
            PaneEntry {
                session_id: "$0".into(),
                session_name: "a".into(),
                path: "/tmp/wt/sub".into(),
            },
        ];
        let attached = match_attached(&panes, Path::new("/tmp/wt"));
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].id, "$0");
    }

    fn make_entry(path: &str, branch: Option<&str>, is_main: bool) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            branch: branch.map(|s| s.to_string()),
            head: None,
            locked: false,
            prunable: false,
            is_main,
        }
    }

    #[test]
    fn resolve_target_exact_path_match() {
        let entries = vec![
            make_entry("/main", Some("refs/heads/main"), true),
            make_entry("/wt-a", Some("refs/heads/feat-a"), false),
        ];
        let m = resolve_target(&entries, "/wt-a").unwrap();
        assert_eq!(m.path, PathBuf::from("/wt-a"));
    }

    #[test]
    fn resolve_target_short_branch_match() {
        let entries = vec![
            make_entry("/main", Some("refs/heads/main"), true),
            make_entry("/wt-a", Some("refs/heads/feat-a"), false),
        ];
        let m = resolve_target(&entries, "feat-a").unwrap();
        assert_eq!(m.path, PathBuf::from("/wt-a"));
    }

    #[test]
    fn resolve_target_excludes_main_worktree() {
        let entries = vec![make_entry("/main", Some("refs/heads/main"), true)];
        let err = resolve_target(&entries, "/main").unwrap_err();
        assert!(err.to_string().contains("no worktree matches"));
    }

    #[test]
    fn resolve_target_any_includes_main_worktree() {
        let entries = vec![
            make_entry("/main", Some("refs/heads/main"), true),
            make_entry("/wt", Some("refs/heads/feat"), false),
        ];
        let by_path = resolve_target_any(&entries, "/main").unwrap();
        assert_eq!(by_path.path, PathBuf::from("/main"));
        let by_branch = resolve_target_any(&entries, "main").unwrap();
        assert_eq!(by_branch.path, PathBuf::from("/main"));
    }

    #[test]
    fn resolve_target_not_found_errors() {
        let entries = vec![make_entry("/wt-a", Some("refs/heads/a"), false)];
        let err = resolve_target(&entries, "/nope").unwrap_err();
        assert!(err.to_string().contains("no worktree matches"));
    }

    #[test]
    fn resolve_target_ambiguous_duplicate_branch_errors() {
        // Two entries with the same branch — real git forbids this but the
        // helper must still surface ambiguity rather than silently picking.
        let entries = vec![
            make_entry("/main", Some("refs/heads/main"), true),
            make_entry("/wt-a", Some("refs/heads/feat-a"), false),
            make_entry("/wt-b", Some("refs/heads/feat-a"), false),
        ];
        let err = resolve_target(&entries, "feat-a").unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn resolve_target_ambiguous_cross_class_errors() {
        // target "feat-a" matches entry 1 by lexical path (no leading slash),
        // and entry 2 by short branch. Resolver must report ambiguous.
        let entries = vec![
            make_entry("/main", Some("refs/heads/main"), true),
            make_entry("feat-a", Some("refs/heads/other"), false),
            make_entry("/other-path", Some("refs/heads/feat-a"), false),
        ];
        let err = resolve_target(&entries, "feat-a").unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn resolve_target_detached_skipped_for_branch_match() {
        let entries = vec![
            make_entry("/main", Some("refs/heads/main"), true),
            make_entry("/det", None, false),
        ];
        let err = resolve_target(&entries, "det").unwrap_err();
        assert!(err.to_string().contains("no worktree matches"));
    }

    #[test]
    fn remove_decision_locked_refuses() {
        let mut e = make_entry("/a", Some("refs/heads/a"), false);
        e.locked = true;
        assert_eq!(remove_decision(&e), RemoveDecision::LockedRefuse);
    }

    #[test]
    fn remove_decision_clean_proceeds() {
        let e = make_entry("/a", Some("refs/heads/a"), false);
        assert_eq!(remove_decision(&e), RemoveDecision::Proceed);
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
