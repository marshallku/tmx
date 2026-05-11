package cmd

import (
	"fmt"
	"os"
	"strings"

	"github.com/marshallku/tmux-powertools/internal/project"
	"github.com/marshallku/tmux-powertools/internal/tmux"
	"github.com/marshallku/tmux-powertools/internal/worktree"
	"github.com/spf13/cobra"
)

var (
	worktreeTmux bool
	worktreeFrom string
)

var worktreeCmd = &cobra.Command{
	Use:   "worktree <branch>",
	Short: "Create a sibling git worktree for the current repo",
	Long: `Create a sibling git worktree, optionally run a post-create script,
and optionally spawn a tmux session at the new worktree.

Worktree is placed next to the source repo (sibling), named via the
'worktree.naming' template in ~/.config/tmux-powertools/config.toml
(default: '{repo}-{branch}'). Slashes in branch names are replaced
with dashes for the directory name.`,
	Args:          cobra.ExactArgs(1),
	RunE:          runWorktree,
	SilenceUsage:  true,
	SilenceErrors: true,
}

func init() {
	worktreeCmd.Flags().BoolVarP(&worktreeTmux, "tmux", "t", false, "create a tmux session at the new worktree and switch to it")
	worktreeCmd.Flags().StringVar(&worktreeFrom, "from", "", "base ref for the new branch (default: HEAD)")
	rootCmd.AddCommand(worktreeCmd)
}

func runWorktree(cmd *cobra.Command, args []string) error {
	branch := args[0]
	cfg := project.LoadConfig()

	res, err := worktree.Create(worktree.Options{
		Branch: branch,
		From:   worktreeFrom,
	}, cfg)
	if err != nil {
		return err
	}

	fmt.Fprintln(os.Stderr, "Created worktree:", res.WorktreePath)
	if res.ScriptRan != "" {
		fmt.Fprintln(os.Stderr, "Ran post-create script:", res.ScriptRan)
	}

	if worktreeTmux {
		sessionName := tmuxSessionName(res.RepoName, branch)
		if !tmux.SessionExists(sessionName) {
			if err := tmux.CreateSession(sessionName, res.WorktreePath); err != nil {
				return fmt.Errorf("create tmux session: %w", err)
			}
		}
		if err := tmux.SwitchSession(sessionName); err != nil {
			return fmt.Errorf("switch tmux session: %w", err)
		}
		return nil
	}

	// No --tmux: print the path on stdout so callers can `cd $(tmux-powertools worktree ...)`
	fmt.Println(res.WorktreePath)
	return nil
}

func tmuxSessionName(repo, branch string) string {
	name := repo + "-" + strings.ReplaceAll(branch, "/", "-")
	return strings.ReplaceAll(name, ".", "_")
}
