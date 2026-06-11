use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DEFAULT_NAMING: &str = "{repo}-{branch}";
const DEFAULT_ATTENTION_LIMIT: usize = 20;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub worktree: WorktreeConfig,
    #[serde(default)]
    pub agents: AgentsConfig,
    /// `[theme]` color overrides (`violet = "#cba6f7"` …). Validated and
    /// applied by `ui::theme::init`; unknown keys / bad values warn there.
    #[serde(default)]
    pub theme: HashMap<String, String>,
    /// `[keys.picker]` / `[keys.agents]` binding overrides
    /// (`quit = ["esc", "ctrl-q"]` …). Validated by `ui::keys::init`.
    #[serde(default)]
    pub keys: KeysConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeysConfig {
    #[serde(default)]
    pub picker: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub agents: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorktreeConfig {
    /// Naming pattern for the worktree directory.
    /// Tokens: `{repo}` (source repo name), `{branch}` (branch name, slashes replaced with dashes).
    #[serde(default)]
    pub naming: String,
    /// Map of source repo path → post-create shell command. Keys may use `~` for `$HOME`.
    /// The value is passed to `bash -c` with the new worktree as the cwd and
    /// `WORKTREE_PATH` exported, so it can be an inline command
    /// (`"mise trust && yarn"`) or a script invocation (`"bash scripts/setup.sh"`).
    #[serde(default)]
    pub scripts: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    /// Maximum number of attention-queue entries the agents dashboard keeps
    /// and displays. `0` hides the attention panel entirely.
    pub attention_limit: usize,
    /// Extra agent process names to watch in addition to the built-in
    /// claude/codex (e.g. `["gemini", "opencode"]`). Matched against the
    /// process comm name; shown in the dashboard under their own name.
    pub extra_agents: Vec<String>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            attention_limit: DEFAULT_ATTENTION_LIMIT,
            extra_agents: Vec::new(),
        }
    }
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
            agents: AgentsConfig::default(),
            theme: HashMap::new(),
            keys: KeysConfig::default(),
        }
    }

    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        let (cfg, warning) = Self::load_from_with_warning(path);
        if let Some(warning) = warning {
            // Several commands load the config more than once per process;
            // a broken file should nag once, not per load.
            static WARN_ONCE: std::sync::Once = std::sync::Once::new();
            WARN_ONCE.call_once(|| eprintln!("tmx: warning: {warning}"));
        }
        cfg
    }

    /// Load a config, reporting why it fell back to defaults (if it did) instead of
    /// swallowing the failure. A missing file is the normal "no config" case and
    /// produces no warning; unreadable or unparsable files do.
    pub fn load_from_with_warning(path: &Path) -> (Self, Option<String>) {
        // Overlay-merge onto defaults: a partial config (e.g. `[worktree] naming = "..."`
        // with no `roots` key) must keep the default `~/dev` root. Parsing directly into
        // `Config` would silently zero out absent fields via `#[serde(default)]`.
        let data = match std::fs::read_to_string(path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (Self::defaults(), None);
            }
            Err(err) => {
                let warning = format!(
                    "failed to read config {} (using defaults): {err}",
                    path.display()
                );
                return (Self::defaults(), Some(warning));
            }
        };
        let partial = match toml::from_str::<PartialConfig>(&data) {
            Ok(partial) => partial,
            Err(err) => {
                // The TOML error is multi-line; lead with the fallback note so it isn't buried.
                let warning = format!("invalid config {} (using defaults): {err}", path.display());
                return (Self::defaults(), Some(warning));
            }
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
        if let Some(agents) = partial.agents {
            if let Some(limit) = agents.attention_limit {
                cfg.agents.attention_limit = limit;
            }
            if let Some(extra) = agents.extra_agents {
                cfg.agents.extra_agents = extra;
            }
        }
        if let Some(theme) = partial.theme {
            cfg.theme = theme;
        }
        if let Some(keys) = partial.keys {
            cfg.keys = keys;
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

        (cfg, None)
    }
}

#[derive(Debug, Default, Deserialize)]
struct PartialConfig {
    roots: Option<Vec<String>>,
    worktree: Option<PartialWorktreeConfig>,
    agents: Option<PartialAgentsConfig>,
    theme: Option<HashMap<String, String>>,
    keys: Option<KeysConfig>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialWorktreeConfig {
    naming: Option<String>,
    scripts: Option<HashMap<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
struct PartialAgentsConfig {
    attention_limit: Option<usize>,
    extra_agents: Option<Vec<String>>,
}

pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("tmx")
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
    fn load_from_missing_file_emits_no_warning() {
        let dir = TempDir::new().unwrap();
        let (_, warning) = Config::load_from_with_warning(&dir.path().join("nope.toml"));
        assert_eq!(warning, None);
    }

    #[test]
    fn load_from_invalid_toml_emits_warning_with_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is = not = valid").unwrap();
        let (cfg, warning) = Config::load_from_with_warning(&path);
        assert_eq!(cfg.worktree.naming, DEFAULT_NAMING);
        let warning = warning.expect("broken config must produce a warning");
        assert!(warning.contains("invalid config"));
        assert!(warning.contains(&path.display().to_string()));
    }

    #[test]
    fn load_from_unreadable_file_emits_warning() {
        // A directory at the config path is unreadable as a file but is not NotFound.
        let dir = TempDir::new().unwrap();
        let (cfg, warning) = Config::load_from_with_warning(dir.path());
        assert_eq!(cfg.worktree.naming, DEFAULT_NAMING);
        let warning = warning.expect("unreadable config must produce a warning");
        assert!(warning.contains("failed to read config"));
    }

    #[test]
    fn load_from_valid_toml_emits_no_warning() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "roots = [\"/opt/projects\"]\n").unwrap();
        let (_, warning) = Config::load_from_with_warning(&path);
        assert_eq!(warning, None);
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
    fn load_from_agents_attention_limit_overrides_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[agents]\nattention_limit = 5\n").unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.agents.attention_limit, 5);
        // Other sections keep their defaults.
        assert_eq!(cfg.worktree.naming, DEFAULT_NAMING);
    }

    #[test]
    fn load_from_missing_agents_section_uses_default_limit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "roots = []\n").unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.agents.attention_limit, DEFAULT_ATTENTION_LIMIT);
        assert!(cfg.agents.extra_agents.is_empty());
    }

    #[test]
    fn load_from_agents_extra_agents_parses() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[agents]\nextra_agents = [\"gemini\", \"opencode\"]\n",
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.agents.extra_agents, vec!["gemini", "opencode"]);
        // Sibling key keeps its default when absent.
        assert_eq!(cfg.agents.attention_limit, DEFAULT_ATTENTION_LIMIT);
    }

    #[test]
    fn load_from_theme_section_parses_as_map() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[theme]\nviolet = \"#112233\"\ntext = \"#445566\"\n").unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.theme.get("violet").map(String::as_str), Some("#112233"));
        assert_eq!(cfg.theme.get("text").map(String::as_str), Some("#445566"));
    }

    #[test]
    fn load_from_missing_theme_section_is_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "roots = []\n").unwrap();
        let cfg = Config::load_from(&path);
        assert!(cfg.theme.is_empty());
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
