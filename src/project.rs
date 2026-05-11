use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectType {
    Go,
    Node,
    Rust,
    Python,
    Generic,
}

impl ProjectType {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectType::Go => "go",
            ProjectType::Node => "node",
            ProjectType::Rust => "rust",
            ProjectType::Python => "python",
            ProjectType::Generic => "generic",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub git_branch: String,
    pub git_dirty: bool,
    pub git_ahead: u32,
    pub git_behind: u32,
    pub project_type: ProjectType,
}

pub fn scan_projects(cfg: &Config) -> Vec<Project> {
    let mut projects = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for root in &cfg.roots {
        let root_path = PathBuf::from(shellexpand_env(root));
        let Ok(entries) = std::fs::read_dir(&root_path) else {
            continue;
        };

        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }

            let full_path = root_path.join(&name);
            if !seen.insert(full_path.clone()) {
                continue;
            }

            if !full_path.join(".git").exists() {
                continue;
            }

            let project_type = detect_project_type(&full_path);
            let (git_branch, git_dirty, git_ahead, git_behind) = get_git_info(&full_path);

            projects.push(Project {
                name,
                path: full_path,
                git_branch,
                git_dirty,
                git_ahead,
                git_behind,
                project_type,
            });
        }
    }

    projects
}

pub fn detect_project_type(path: &Path) -> ProjectType {
    // Order matches the Go version's intent but is explicit (HashMap order was
    // accidental there). Check the most language-specific markers first.
    let checks: &[(&str, ProjectType)] = &[
        ("go.mod", ProjectType::Go),
        ("Cargo.toml", ProjectType::Rust),
        ("package.json", ProjectType::Node),
        ("pyproject.toml", ProjectType::Python),
        ("requirements.txt", ProjectType::Python),
    ];
    for (file, kind) in checks {
        if path.join(file).exists() {
            return *kind;
        }
    }
    ProjectType::Generic
}

pub fn get_git_info(path: &Path) -> (String, bool, u32, u32) {
    let branch = run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = match run_git(path, &["status", "--porcelain"]) {
        Some(out) => !out.trim().is_empty(),
        None => false,
    };

    let (ahead, behind) = match run_git(
        path,
        &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    ) {
        Some(out) => parse_ahead_behind(&out),
        None => (0, 0),
    };

    (branch, dirty, ahead, behind)
}

fn parse_ahead_behind(s: &str) -> (u32, u32) {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 2 {
        return (0, 0);
    }
    let ahead = parts[0].parse().unwrap_or(0);
    let behind = parts[1].parse().unwrap_or(0);
    (ahead, behind)
}

fn run_git(path: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(path);
    for a in args {
        cmd.arg(a);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Expand `$VAR` style references in a path. Mirrors Go's `os.ExpandEnv`.
fn shellexpand_env(input: &str) -> String {
    expand_env_with(input, |name| std::env::var(name).ok())
}

/// Core of `shellexpand_env` parameterised on the lookup function. Splitting
/// the I/O lets tests inject a stub instead of mutating process env, which
/// would race other parallel tests.
fn expand_env_with<F>(input: &str, lookup: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while !rest.is_empty() {
        // `'$'` is single-byte ASCII so byte-index `find` is UTF-8 safe and
        // slicing on it stays on a char boundary.
        let Some(dollar) = rest.find('$') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..dollar]);
        let after_dollar = &rest[dollar + 1..];

        // ${VAR}
        if let Some(inside) = after_dollar.strip_prefix('{')
            && let Some(close) = inside.find('}')
        {
            let name = &inside[..close];
            if let Some(val) = lookup(name) {
                out.push_str(&val);
            }
            rest = &inside[close + 1..];
            continue;
        }

        // $VAR — name is [A-Za-z0-9_]+
        let name_end = after_dollar
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(after_dollar.len());

        if name_end == 0 {
            // Bare `$` with no name: emit it as a literal.
            out.push('$');
            rest = after_dollar;
            continue;
        }

        let name = &after_dollar[..name_end];
        if let Some(val) = lookup(name) {
            out.push_str(&val);
        }
        rest = &after_dollar[name_end..];
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_type_recognises_markers() {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        assert_eq!(detect_project_type(p), ProjectType::Generic);

        std::fs::write(p.join("requirements.txt"), "").unwrap();
        assert_eq!(detect_project_type(p), ProjectType::Python);

        std::fs::write(p.join("package.json"), "{}").unwrap();
        assert_eq!(detect_project_type(p), ProjectType::Node);

        std::fs::write(p.join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_project_type(p), ProjectType::Rust);

        // go.mod wins because it is checked first
        std::fs::write(p.join("go.mod"), "").unwrap();
        assert_eq!(detect_project_type(p), ProjectType::Go);
    }

    #[test]
    fn project_type_string_matches_go_version() {
        assert_eq!(ProjectType::Go.as_str(), "go");
        assert_eq!(ProjectType::Node.as_str(), "node");
        assert_eq!(ProjectType::Rust.as_str(), "rust");
        assert_eq!(ProjectType::Python.as_str(), "python");
        assert_eq!(ProjectType::Generic.as_str(), "generic");
    }

    #[test]
    fn parse_ahead_behind_handles_well_formed() {
        assert_eq!(parse_ahead_behind("3\t5\n"), (3, 5));
        assert_eq!(parse_ahead_behind("0 0"), (0, 0));
    }

    #[test]
    fn parse_ahead_behind_handles_malformed() {
        assert_eq!(parse_ahead_behind(""), (0, 0));
        assert_eq!(parse_ahead_behind("3"), (0, 0));
        assert_eq!(parse_ahead_behind("abc def"), (0, 0));
    }

    #[test]
    fn expand_env_with_stub_lookup() {
        let lookup = |name: &str| match name {
            "FOO" => Some("bar".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        };
        assert_eq!(expand_env_with("$FOO/x", lookup), "bar/x");
        assert_eq!(expand_env_with("${FOO}-end", lookup), "bar-end");
        assert_eq!(expand_env_with("plain", lookup), "plain");
        assert_eq!(expand_env_with("$MISSING/x", lookup), "/x");
        assert_eq!(expand_env_with("${EMPTY}!", lookup), "!");
        assert_eq!(expand_env_with("$", lookup), "$");
        assert_eq!(expand_env_with("$ foo", lookup), "$ foo");
        assert_eq!(expand_env_with("price: $50", lookup), "price: ");
    }

    #[test]
    fn expand_env_with_preserves_unicode() {
        // Regression: a naive byte-level expander would corrupt multibyte
        // UTF-8 sequences. Roots like `/home/me/개발/$ORG` must round-trip
        // their non-ASCII segments untouched and still expand `$ORG`.
        let lookup = |name: &str| match name {
            "ORG" => Some("toss".to_string()),
            _ => None,
        };
        assert_eq!(
            expand_env_with("/home/me/개발/$ORG", lookup),
            "/home/me/개발/toss"
        );
        assert_eq!(
            expand_env_with("프로젝트/${ORG}/x", lookup),
            "프로젝트/toss/x"
        );
        assert_eq!(expand_env_with("개발-${MISSING}-끝", lookup), "개발--끝");
        assert_eq!(expand_env_with("순수한경로", lookup), "순수한경로");
    }

    #[test]
    fn scan_projects_skips_non_git_and_hidden() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        let repo = root.join("real-repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "").unwrap();

        let plain = root.join("not-a-repo");
        std::fs::create_dir(&plain).unwrap();

        let hidden = root.join(".hidden-repo");
        std::fs::create_dir(&hidden).unwrap();
        std::fs::create_dir(hidden.join(".git")).unwrap();

        let cfg = Config {
            roots: vec![root.to_string_lossy().into_owned()],
            ..Config::default()
        };
        let projects = scan_projects(&cfg);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "real-repo");
        assert_eq!(projects[0].project_type, ProjectType::Rust);
    }
}
