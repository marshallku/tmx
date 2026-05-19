//! Classify Claude/Codex's UI state from a tmux pane snapshot.
//!
//! We can't query Claude Code's internal state directly, so we read what
//! the user sees: the visible portion of the tmux pane. The Claude TUI is
//! distinctive enough that a tail-of-pane regex hits each state cleanly:
//!
//!   * **AwaitingDecision** — selection / permission dialog up. Most
//!     visible cue is the universal "Enter to select · ↑/↓ to navigate ·
//!     Esc to cancel" hint at the bottom.
//!   * **Working** — Claude is composing or running a tool. The bottom
//!     line carries an "esc to interrupt" hint and the input box is
//!     replaced by an active spinner glyph (✻/✱/✷/✸/✹/✺).
//!   * **Ready** — Claude is parked at the chat prompt. The hallmark is a
//!     bare `❯ ` line followed by the model footer.
//!   * **Unknown** — pane has no Claude UI markers we recognise. Caller
//!     decides how to map that (typically `Idle`).
//!
//! Patterns intentionally bias toward false negatives over false
//! positives — when in doubt we'd rather show "idle" than mis-claim
//! "decision".

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeUiState {
    Working,
    AwaitingDecision,
    Ready,
    Unknown,
}

/// Spinner glyphs Claude Code uses in its "thinking" lines. Presence of
/// any in the last few lines is a strong working signal when paired with
/// the absence of a free `❯ ` prompt.
const SPINNER_CHARS: &[char] = &['✱', '✻', '✷', '✸', '✹', '✺'];

/// Strings that unambiguously signal an active selection / permission
/// dialog blocking on the user. Match wins over everything else.
const DECISION_MARKERS: &[&str] = &[
    "Enter to select",
    "to navigate ·",
    "Do you want to proceed",
    "Allow this tool",
];

/// Strings that signal Claude is mid-turn and the input prompt is
/// suppressed. The "esc to interrupt" hint is the most reliable one
/// because it only appears while a turn is in flight.
const WORKING_MARKERS: &[&str] = &["esc to interrupt"];

/// Classify the visible pane content. Looks at roughly the last 30 lines —
/// enough to cover the input box plus footer without scanning the entire
/// scrollback.
pub fn classify(pane_content: &str) -> ClaudeUiState {
    let tail = tail_lines(pane_content, 30);

    if DECISION_MARKERS.iter().any(|m| tail.contains(m)) {
        return ClaudeUiState::AwaitingDecision;
    }

    if WORKING_MARKERS.iter().any(|m| tail.contains(m)) {
        return ClaudeUiState::Working;
    }

    if has_ready_prompt(&tail) {
        return ClaudeUiState::Ready;
    }

    // Spinner without an "esc to interrupt" hint is rare but possible —
    // treat it as working too. Done after the explicit hint check so we
    // don't classify decorative sparkles in a chat message as working.
    if tail.chars().any(|c| SPINNER_CHARS.contains(&c)) && !has_ready_prompt(&tail) {
        return ClaudeUiState::Working;
    }

    ClaudeUiState::Unknown
}

/// True when the tail shows a free `❯` prompt — i.e. a line whose first
/// non-space character is `❯` and the next non-`❯` character (if any) is
/// whitespace or end-of-line. Rejects `❯ 1.` style (selection cursor on a
/// numbered list), since that belongs to AwaitingDecision.
fn has_ready_prompt(tail: &str) -> bool {
    tail.lines().any(|line| {
        let mut chars = line.trim_start().chars();
        if chars.next() != Some('❯') {
            return false;
        }
        match chars.next() {
            None => true, // bare "❯"
            Some(c) if c.is_whitespace() => !chars
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| c.is_ascii_digit()),
            _ => false,
        }
    })
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ready_state_from_real_capture() {
        let pane = "\
  /cross-review now, or (c) build anti-pattern hook.

─────────────────────────────────────────────
❯
─────────────────────────────────────────────
   Opus 4.7 (1M context) dotfiles master | +1008-68
   ━━━━╌╌╌╌╌╌╌╌╌╌╌ 32% | $19.08 | 6h14m | 5h14% 7d35%
  ⏵⏵ bypass permissions on (shift+tab to cycle) · ← for agents
                                                             new task? /clear
";
        assert_eq!(classify(pane), ClaudeUiState::Ready);
    }

    #[test]
    fn classify_decision_state_from_real_capture() {
        let pane = "\
  3. 스크린샷 공유해주면 확인 가능
  4. Type something.
───────────────────────────────────────────────────────────
  5. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";
        assert_eq!(classify(pane), ClaudeUiState::AwaitingDecision);
    }

    #[test]
    fn classify_working_state_with_interrupt_hint() {
        let pane = "\
✱ Baking the response...
(esc to interrupt)
";
        assert_eq!(classify(pane), ClaudeUiState::Working);
    }

    #[test]
    fn classify_working_state_with_spinner_only() {
        // No interrupt hint but a spinner is present and no ❯ prompt.
        let pane = "\
✻ Baked for 1m 26s
  ⎿  Some intermediate output
";
        assert_eq!(classify(pane), ClaudeUiState::Working);
    }

    #[test]
    fn classify_unknown_for_plain_shell() {
        let pane = "\
$ ls
file1  file2  file3
$
";
        assert_eq!(classify(pane), ClaudeUiState::Unknown);
    }

    #[test]
    fn has_ready_prompt_rejects_numbered_selection_cursor() {
        let tail = "❯ 1. First option\n  2. Second option";
        assert!(!has_ready_prompt(tail));
    }

    #[test]
    fn has_ready_prompt_accepts_empty_chat_prompt() {
        assert!(has_ready_prompt("❯ "));
        assert!(has_ready_prompt("  ❯ "));
        assert!(has_ready_prompt("❯ some draft text"));
    }

    #[test]
    fn classify_decision_wins_over_spinner() {
        // A pane that happens to have a ✻ in a chat message but is
        // actively showing a decision should still classify as decision.
        let pane = "\
  ✻ Mentioned in a message
  3. Option three
Enter to select · ↑/↓ to navigate · Esc to cancel
";
        assert_eq!(classify(pane), ClaudeUiState::AwaitingDecision);
    }

    #[test]
    fn tail_lines_returns_last_n() {
        let s = "a\nb\nc\nd\ne";
        assert_eq!(tail_lines(s, 2), "d\ne");
        assert_eq!(tail_lines(s, 10), "a\nb\nc\nd\ne");
    }
}
