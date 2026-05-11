use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DEFAULT_NAMING: &str = "{repo}-{branch}";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub worktree: WorktreeConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorktreeConfig {
    /// Naming pattern for the worktree directory.
    /// Tokens: `{repo}` (source repo name), `{branch}` (branch name, slashes replaced with dashes).
    #[serde(default)]
    pub naming: String,
    /// Map of source repo path → post-create script. Keys may use `~` for `$HOME`.
    /// Script paths: relative → resolved against the new worktree directory;
    /// absolute / `~`-prefixed used as-is.
    #[serde(default)]
    pub scripts: HashMap<String, String>,
}

impl Config {
    pub fn defaults() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            roots: vec![home.join("dev").to_string_lossy().into_owned()],
            worktree: WorktreeConfig {
                naming: DEFAULT_NAMING.to_string(),
                scripts: HashMap::new(),
            },
        }
    }

    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        // Overlay-merge onto defaults: a partial config (e.g. `[worktree] naming = "..."`
        // with no `roots` key) must keep the default `~/dev` root. Parsing directly into
        // `Config` would silently zero out absent fields via `#[serde(default)]`.
        let Ok(data) = std::fs::read_to_string(path) else {
            return Self::defaults();
        };
        let Ok(partial) = toml::from_str::<PartialConfig>(&data) else {
            return Self::defaults();
        };

        let mut cfg = Self::defaults();
        if let Some(roots) = partial.roots {
            cfg.roots = roots;
        }
        if let Some(wt) = partial.worktree {
            if let Some(naming) = wt.naming {
                cfg.worktree.naming = naming;
            }
            if let Some(scripts) = wt.scripts {
                cfg.worktree.scripts = scripts;
            }
        }

        if cfg.worktree.naming.is_empty() {
            cfg.worktree.naming = DEFAULT_NAMING.to_string();
        }

        cfg.roots = cfg.roots.iter().map(|p| expand_home(p)).collect();
        cfg.worktree.scripts = cfg
            .worktree
            .scripts
            .iter()
            .map(|(k, v)| (expand_home(k), v.clone()))
            .collect();

        cfg
    }
}

#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    roots: Option<Vec<String>>,
    worktree: Option<PartialWorktreeConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialWorktreeConfig {
    naming: Option<String>,
    scripts: Option<HashMap<String, String>>,
}

pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("tmux-powertools")
        .join("config.toml")
}

pub fn ensure_config_dir() {
    if let Some(dir) = config_path().parent() {
        std::fs::create_dir_all(dir).ok();
    }
}

/// Expand a leading `~` to `$HOME`. Other paths returned unchanged.
pub fn expand_home(input: &str) -> String {
    if !input.starts_with('~') {
        return input.to_string();
    }
    let Some(home) = dirs::home_dir() else {
        return input.to_string();
    };
    if input == "~" {
        return home.to_string_lossy().into_owned();
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return home.join(rest).to_string_lossy().into_owned();
    }
    input.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn expand_home_passes_through_non_tilde() {
        assert_eq!(expand_home("/abs/path"), "/abs/path");
        assert_eq!(expand_home("rel/path"), "rel/path");
        assert_eq!(expand_home(""), "");
    }

    #[test]
    fn expand_home_expands_tilde_prefix() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~"), home.to_string_lossy());
        assert_eq!(
            expand_home("~/foo/bar"),
            home.join("foo/bar").to_string_lossy()
        );
    }

    #[test]
    fn expand_home_does_not_expand_tilde_user() {
        assert_eq!(expand_home("~someone/x"), "~someone/x");
    }

    #[test]
    fn load_from_missing_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::load_from(&dir.path().join("nope.toml"));
        assert!(!cfg.roots.is_empty());
        assert_eq!(cfg.worktree.naming, DEFAULT_NAMING);
    }

    #[test]
    fn load_from_invalid_toml_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is = not = valid").unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.worktree.naming, DEFAULT_NAMING);
    }

    #[test]
    fn load_from_valid_toml_parses_and_expands() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
roots = ["~/code", "/opt/projects"]

[worktree]
naming = "{repo}_{branch}"

[worktree.scripts]
"~/code/myrepo" = "scripts/setup.sh"
"#,
        )
        .unwrap();

        let cfg = Config::load_from(&path);
        let home = dirs::home_dir().unwrap();

        assert_eq!(cfg.worktree.naming, "{repo}_{branch}");
        assert_eq!(cfg.roots.len(), 2);
        assert_eq!(cfg.roots[0], home.join("code").to_string_lossy());
        assert_eq!(cfg.roots[1], "/opt/projects");
        let key = home.join("code/myrepo").to_string_lossy().into_owned();
        assert_eq!(
            cfg.worktree.scripts.get(&key).map(String::as_str),
            Some("scripts/setup.sh")
        );
    }

    #[test]
    fn load_from_partial_config_preserves_default_roots() {
        // Mirrors the Go LoadConfig behaviour: a config with only `[worktree]` but no
        // `roots` key must keep the default `~/dev` root, not silently disable scanning.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[worktree]\nnaming = \"wt_{repo}_{branch}\"\n").unwrap();

        let cfg = Config::load_from(&path);
        assert_eq!(cfg.worktree.naming, "wt_{repo}_{branch}");

        let defaults = Config::defaults();
        assert_eq!(
            cfg.roots, defaults.roots,
            "partial config dropped the default roots"
        );
    }

    #[test]
    fn load_from_explicit_empty_roots_overrides_defaults() {
        // `roots = []` is an explicit user choice and must override defaults.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "roots = []\n").unwrap();

        let cfg = Config::load_from(&path);
        assert!(cfg.roots.is_empty());
    }

    #[test]
    fn load_from_empty_naming_falls_back_to_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "roots = []\n[worktree]\nnaming = \"\"\n").unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.worktree.naming, DEFAULT_NAMING);
    }
}
