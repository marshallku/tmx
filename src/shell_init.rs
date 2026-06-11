use anyhow::{Result, bail};

/// Body of the `twt` function shared by zsh and bash — kept to the POSIX-ish
/// subset both shells execute identically (`local`, arrays, `${@:2}`, `(( ))`).
const TWT_POSIX_BODY: &str = r#"    local sub="${1:-}"
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
"#;

const FISH_INIT: &str = r#"# tmx shell integration (fish)
function twt
    set -l sub ""
    if test (count $argv) -ge 1
        set sub $argv[1]
    end
    switch "$sub"
        case "" list
            set -l rest $argv[2..-1]
            # `list --plain` is a text dump → pass through and never cd.
            if contains -- --plain $rest
                command tmx worktree list $rest
                return $status
            end
            # Picker (or `list <target>`) → cd into the printed path.
            set -l out (command tmx worktree list $rest)
            set -l rc $status
            test $rc -ne 0; and return $rc
            test -z "$out"; and return 0
            cd "$out"
        case rm create help -h --help
            command tmx worktree $argv
        case '*'
            # Implicit `create <branch>` shorthand; capture stdout only when
            # --keep-current (or -p) is set, mirroring the zsh wrapper.
            set -l print_only 0
            set -l keep_current 0
            set -l args
            for a in $argv
                switch "$a"
                    case -p --print
                        set print_only 1
                        set keep_current 1
                    case --keep-current
                        set keep_current 1
                        set args $args $a
                    case '*'
                        set args $args $a
                end
            end
            if test $keep_current -eq 1
                if not contains -- --keep-current $args
                    set args $args --keep-current
                end
                set -l out (command tmx worktree create $args)
                set -l rc $status
                test $rc -ne 0; and return $rc
                test -z "$out"; and return 0
                if test $print_only -eq 1
                    printf '%s\n' "$out"
                    return 0
                end
                cd "$out"
                return 0
            end
            command tmx worktree create $args
    end
end
"#;

fn zsh_init() -> String {
    format!("# tmx shell integration (zsh)\ntwt() {{\n    emulate -L zsh\n{TWT_POSIX_BODY}}}\n")
}

fn bash_init() -> String {
    format!("# tmx shell integration (bash)\ntwt() {{\n{TWT_POSIX_BODY}}}\n")
}

pub fn emit(shell: &str) -> Result<String> {
    match shell {
        "zsh" => Ok(zsh_init()),
        "bash" => Ok(bash_init()),
        "fish" => Ok(FISH_INIT.to_string()),
        other => bail!("unsupported shell: {other} (supported: zsh, bash, fish)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_zsh_contains_function_definition() {
        let out = emit("zsh").unwrap();
        assert!(out.contains("twt()"));
        assert!(out.contains("emulate -L zsh"));
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
    fn emit_bash_matches_zsh_minus_emulate() {
        let out = emit("bash").unwrap();
        assert!(out.contains("twt()"));
        assert!(!out.contains("emulate"), "bash has no zsh emulate builtin");
        assert!(out.contains("tmx worktree create"));
        assert!(out.contains("rm|create|help"));
        assert!(out.contains("--keep-current"));
    }

    #[test]
    fn emit_fish_contains_function_definition() {
        let out = emit("fish").unwrap();
        assert!(out.contains("function twt"));
        assert!(out.contains("command tmx worktree list"));
        assert!(out.contains("command tmx worktree create"));
        assert!(out.contains("case rm create help -h --help"));
        assert!(out.contains("--plain"));
        assert!(out.contains("--keep-current"));
        // No bash-isms may leak into the fish script.
        assert!(!out.contains("[["));
        assert!(!out.contains("local "));
    }

    #[test]
    fn emit_unknown_shell_errors() {
        let err = emit("powershell").unwrap_err();
        assert!(err.to_string().contains("unsupported shell"));
        assert!(err.to_string().contains("powershell"));
        assert!(err.to_string().contains("zsh, bash, fish"));
    }
}
