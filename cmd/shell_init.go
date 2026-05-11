package cmd

import (
	"fmt"

	"github.com/spf13/cobra"
)

var shellInitCmd = &cobra.Command{
	Use:   "shell-init <shell>",
	Short: "Emit shell integration code (defines the 'twt' wrapper)",
	Long: `Emit shell initialization code that defines a 'twt' function
wrapping 'tmux-powertools worktree' to cd into the new worktree by default.

Add to your shell rc:

    eval "$(tmux-powertools shell-init zsh)"

Then:

    twt feat-x           # create worktree and cd into it
    twt feat-x -p        # create worktree and print path (no cd)
    twt feat-x --tmux    # create worktree and switch to a new tmux session

Supported shells: zsh.`,
	Args:          cobra.ExactArgs(1),
	RunE:          runShellInit,
	SilenceUsage:  true,
	SilenceErrors: true,
}

func init() {
	rootCmd.AddCommand(shellInitCmd)
}

const zshInit = `# tmux-powertools shell integration (zsh)
twt() {
    emulate -L zsh
    local print_only=0
    local args=()
    local a
    for a in "$@"; do
        case "$a" in
            -p|--print) print_only=1 ;;
            *) args+=("$a") ;;
        esac
    done
    local out
    out=$(command tmux-powertools worktree "${args[@]}")
    local rc=$?
    if (( rc != 0 )); then
        return $rc
    fi
    [[ -z "$out" ]] && return 0
    if (( print_only )); then
        printf '%s\n' "$out"
        return 0
    fi
    cd "$out"
}
`

func runShellInit(cmd *cobra.Command, args []string) error {
	switch args[0] {
	case "zsh":
		fmt.Print(zshInit)
		return nil
	default:
		return fmt.Errorf("unsupported shell: %s (supported: zsh)", args[0])
	}
}
