package worktree

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/marshallku/tmux-powertools/internal/project"
)

type Options struct {
	Branch  string // new branch name (also used to derive dir name)
	From    string // base ref for the new branch; empty → git default (HEAD)
	StartIn string // working directory to resolve the source repo from; empty → cwd
}

type Result struct {
	RepoRoot     string // source repo root
	RepoName     string // basename of repo root
	WorktreePath string // absolute path of the new worktree
	ScriptRan    string // post-create script that was executed (empty if none)
}

// Create runs the full worktree-creation flow: resolves the source repo, computes
// the sibling path, runs `git worktree add`, and executes the matching post-create
// script. The caller (cmd layer) is responsible for any tmux integration.
func Create(opts Options, cfg project.Config) (*Result, error) {
	if strings.TrimSpace(opts.Branch) == "" {
		return nil, errors.New("branch name is required")
	}

	startIn := opts.StartIn
	if startIn == "" {
		cwd, err := os.Getwd()
		if err != nil {
			return nil, fmt.Errorf("resolve cwd: %w", err)
		}
		startIn = cwd
	}

	repoRoot, err := resolveRepoRoot(startIn)
	if err != nil {
		return nil, err
	}

	repoName := filepath.Base(repoRoot)
	dirName := renderNaming(cfg.Worktree.Naming, repoName, opts.Branch)
	worktreePath := filepath.Join(filepath.Dir(repoRoot), dirName)

	if _, err := os.Stat(worktreePath); err == nil {
		return nil, fmt.Errorf("target path already exists: %s", worktreePath)
	}

	if err := runGitWorktreeAdd(repoRoot, worktreePath, opts.Branch, opts.From); err != nil {
		return nil, err
	}

	res := &Result{
		RepoRoot:     repoRoot,
		RepoName:     repoName,
		WorktreePath: worktreePath,
	}

	script := cfg.Worktree.Scripts[repoRoot]
	if script != "" {
		if err := runPostCreate(script, worktreePath); err != nil {
			return res, fmt.Errorf("post_create script failed: %w", err)
		}
		res.ScriptRan = script
	}

	return res, nil
}

func resolveRepoRoot(startIn string) (string, error) {
	cmd := exec.Command("git", "-C", startIn, "rev-parse", "--show-toplevel")
	out, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("not inside a git repository: %s", startIn)
	}
	return strings.TrimSpace(string(out)), nil
}

func renderNaming(pattern, repo, branch string) string {
	if pattern == "" {
		pattern = "{repo}-{branch}"
	}
	safeBranch := strings.ReplaceAll(branch, "/", "-")
	out := strings.ReplaceAll(pattern, "{repo}", repo)
	out = strings.ReplaceAll(out, "{branch}", safeBranch)
	return out
}

func runGitWorktreeAdd(repoRoot, worktreePath, branch, from string) error {
	args := []string{"-C", repoRoot, "worktree", "add", "-b", branch, worktreePath}
	if from != "" {
		args = append(args, from)
	}
	cmd := exec.Command("git", args...)
	cmd.Stdout = os.Stderr
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("git worktree add: %w", err)
	}
	return nil
}

func runPostCreate(script, worktreePath string) error {
	resolved := resolveScriptPath(script, worktreePath)
	cmd := exec.Command("bash", resolved)
	cmd.Dir = worktreePath
	cmd.Stdout = os.Stderr
	cmd.Stderr = os.Stderr
	cmd.Env = append(os.Environ(),
		"WORKTREE_PATH="+worktreePath,
	)
	return cmd.Run()
}

func resolveScriptPath(script, worktreePath string) string {
	expanded := project.ExpandHome(script)
	if filepath.IsAbs(expanded) {
		return expanded
	}
	return filepath.Join(worktreePath, expanded)
}
