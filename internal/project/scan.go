package project

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/pelletier/go-toml/v2"
)

type Project struct {
	Name      string
	Path      string
	GitBranch string
	GitDirty  bool
	GitAhead  int
	GitBehind int
	Type      string // "go", "node", "rust", "python", "generic"
}

type Config struct {
	Roots    []string       `toml:"roots"`
	Worktree WorktreeConfig `toml:"worktree"`
}

type WorktreeConfig struct {
	// Naming pattern for the worktree directory.
	// Tokens: {repo} (source repo name), {branch} (branch name, slashes replaced with dashes).
	Naming string `toml:"naming"`
	// Map of source repo path → post-create script. Keys may use ~ for $HOME.
	// Script paths: relative → resolved against the new worktree directory; absolute / ~-prefixed used as-is.
	Scripts map[string]string `toml:"scripts"`
}

func DefaultConfig() Config {
	home, _ := os.UserHomeDir()
	return Config{
		Roots: []string{
			filepath.Join(home, "dev"),
		},
		Worktree: WorktreeConfig{
			Naming: "{repo}-{branch}",
		},
	}
}

func ConfigPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "tmux-powertools", "config.toml")
}

func LoadConfig() Config {
	cfg := DefaultConfig()
	data, err := os.ReadFile(ConfigPath())
	if err != nil {
		return cfg
	}

	if err := toml.Unmarshal(data, &cfg); err != nil {
		return DefaultConfig()
	}

	if cfg.Worktree.Naming == "" {
		cfg.Worktree.Naming = "{repo}-{branch}"
	}

	cfg.Roots = expandPaths(cfg.Roots)
	cfg.Worktree.Scripts = expandKeys(cfg.Worktree.Scripts)

	return cfg
}

func expandPaths(paths []string) []string {
	out := make([]string, len(paths))
	for i, p := range paths {
		out[i] = ExpandHome(p)
	}
	return out
}

func expandKeys(m map[string]string) map[string]string {
	if m == nil {
		return nil
	}
	out := make(map[string]string, len(m))
	for k, v := range m {
		out[ExpandHome(k)] = v
	}
	return out
}

// ExpandHome expands a leading ~ to $HOME. Other paths returned unchanged.
func ExpandHome(p string) string {
	if !strings.HasPrefix(p, "~") {
		return p
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return p
	}
	if p == "~" {
		return home
	}
	if strings.HasPrefix(p, "~/") {
		return filepath.Join(home, p[2:])
	}
	return p
}

func ScanProjects(cfg Config) []Project {
	var projects []Project
	seen := make(map[string]bool)

	for _, root := range cfg.Roots {
		root = os.ExpandEnv(root)
		entries, err := os.ReadDir(root)
		if err != nil {
			continue
		}

		for _, entry := range entries {
			if !entry.IsDir() || strings.HasPrefix(entry.Name(), ".") {
				continue
			}

			fullPath := filepath.Join(root, entry.Name())
			if seen[fullPath] {
				continue
			}
			seen[fullPath] = true

			// Check if it's a git repo
			if _, err := os.Stat(filepath.Join(fullPath, ".git")); err != nil {
				continue
			}

			p := Project{
				Name: entry.Name(),
				Path: fullPath,
				Type: detectProjectType(fullPath),
			}

			p.GitBranch, p.GitDirty, p.GitAhead, p.GitBehind = getGitInfo(fullPath)
			projects = append(projects, p)
		}
	}

	return projects
}

func detectProjectType(path string) string {
	checks := map[string]string{
		"go.mod":           "go",
		"package.json":     "node",
		"Cargo.toml":       "rust",
		"pyproject.toml":   "python",
		"requirements.txt": "python",
	}

	for file, typ := range checks {
		if _, err := os.Stat(filepath.Join(path, file)); err == nil {
			return typ
		}
	}

	return "generic"
}

func getGitInfo(path string) (branch string, dirty bool, ahead, behind int) {
	// Get branch name
	cmd := exec.Command("git", "-C", path, "rev-parse", "--abbrev-ref", "HEAD")
	out, err := cmd.Output()
	if err != nil {
		return "unknown", false, 0, 0
	}
	branch = strings.TrimSpace(string(out))

	// Check dirty
	cmd = exec.Command("git", "-C", path, "status", "--porcelain")
	out, err = cmd.Output()
	if err == nil {
		dirty = len(strings.TrimSpace(string(out))) > 0
	}

	// Check ahead/behind
	cmd = exec.Command("git", "-C", path, "rev-list", "--left-right", "--count", "HEAD...@{upstream}")
	out, err = cmd.Output()
	if err == nil {
		parts := strings.Fields(string(out))
		if len(parts) == 2 {
			fmt.Sscanf(parts[0], "%d", &ahead)
			fmt.Sscanf(parts[1], "%d", &behind)
		}
	}

	return
}
