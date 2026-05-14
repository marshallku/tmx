use anyhow::{Result, bail};

const ZSH_INIT: &str = r#"# tmx shell integration (zsh)
twt() {
    emulate -L zsh
    local sub="${1:-}"
    case "$sub" in
        ""|list)
            # `list --plain` is a text dump → pass through and never cd, even
            # when only one entry exists (command substitution would strip the
            # trailing newline, defeating a multi-line heuristic).
            local arg
            for arg in "${@:2}"; do
                if [[ "$arg" == "--plain" ]]; then
                    command tmx worktree list "${@:2}"
                    return $?
                fi
            done
            # Picker (or `list <target>`) → cd into the printed path.
            local out
            out=$(command tmx worktree list "${@:2}")
            local rc=$?
            (( rc != 0 )) && return $rc
            [[ -z "$out" ]] && return 0
            cd "$out"
            ;;
        rm|create|help|-h|--help)
            command tmx worktree "$@"
            ;;
        *)
            # Implicit `create <branch>` shorthand. Default flow now spawns
            # and switches to a tmux session inside tmx itself, so the wrapper
            # only needs to capture stdout when --keep-current (or -p) is set.
            local print_only=0 keep_current=0 args=() a
            for a in "$@"; do
                case "$a" in
                    -p|--print)
                        print_only=1
                        keep_current=1
                        ;;
                    --keep-current)
                        keep_current=1
                        args+=("$a")
                        ;;
                    *) args+=("$a") ;;
                esac
            done
            if (( keep_current )); then
                # Ensure --keep-current is passed exactly once even if user
                # supplied only -p.
                local has_kc=0 a2
                for a2 in "${args[@]}"; do
                    [[ "$a2" == "--keep-current" ]] && has_kc=1
                done
                (( has_kc )) || args+=("--keep-current")
                local out
                out=$(command tmx worktree create "${args[@]}")
                local rc=$?
                (( rc != 0 )) && return $rc
                [[ -z "$out" ]] && return 0
                if (( print_only )); then
                    printf '%s\n' "$out"
                    return 0
                fi
                cd "$out"
                return 0
            fi
            # Default: tmx creates the worktree and switches into a new tmux
            # session itself; we don't need to cd.
            command tmx worktree create "${args[@]}"
            ;;
    esac
}
"#;

pub fn emit(shell: &str) -> Result<&'static str> {
    match shell {
        "zsh" => Ok(ZSH_INIT),
        other => bail!("unsupported shell: {other} (supported: zsh)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_zsh_contains_function_definition() {
        let out = emit("zsh").unwrap();
        assert!(out.contains("twt()"));
        assert!(out.contains("tmx worktree create"));
        assert!(out.contains("tmx worktree list"));
        // Subcommands should pass through unchanged, not be re-interpreted as a branch.
        assert!(out.contains("rm|create|help"));
        // --plain must short-circuit out of the cd path.
        assert!(out.contains("--plain"));
        // --keep-current opt-out and the bare -p alias must be recognised.
        assert!(out.contains("--keep-current"));
        assert!(out.contains("-p|--print"));
    }

    #[test]
    fn emit_unknown_shell_errors() {
        let err = emit("fish").unwrap_err();
        assert!(err.to_string().contains("unsupported shell"));
        assert!(err.to_string().contains("fish"));
    }
}
