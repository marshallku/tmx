use anyhow::{Result, bail};

const ZSH_INIT: &str = r#"# tmux-powertools shell integration (zsh)
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
        assert!(out.contains("tmux-powertools worktree"));
    }

    #[test]
    fn emit_unknown_shell_errors() {
        let err = emit("fish").unwrap_err();
        assert!(err.to_string().contains("unsupported shell"));
        assert!(err.to_string().contains("fish"));
    }
}
