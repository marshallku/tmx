pub mod project_selector;
pub mod session_switcher;
pub mod theme;

pub use project_selector::{open_project, run_project_selector};
pub use session_switcher::run_session_switcher;

/// Subsequence-style fuzzy match: every char of `query` must appear in `s`
/// in order, case-insensitively. Mirrors the Go version's behaviour but is
/// Unicode-safe rather than byte-based.
pub fn fuzzy_match(s: &str, query: &str) -> bool {
    let s = s.to_lowercase();
    let q = query.to_lowercase();
    let mut q_chars = q.chars();
    let Some(mut needle) = q_chars.next() else {
        return true;
    };
    for c in s.chars() {
        if c == needle {
            match q_chars.next() {
                Some(next) => needle = next,
                None => return true,
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_empty_query_always_matches() {
        assert!(fuzzy_match("anything", ""));
        assert!(fuzzy_match("", ""));
    }

    #[test]
    fn fuzzy_match_subsequence_hits() {
        assert!(fuzzy_match("tmux-powertools", "tmx"));
        assert!(fuzzy_match("tmux-powertools", "tpt"));
        assert!(fuzzy_match("Hello World", "hw"));
    }

    #[test]
    fn fuzzy_match_misses() {
        assert!(!fuzzy_match("abc", "abcd"));
        assert!(!fuzzy_match("abc", "zz"));
        assert!(!fuzzy_match("", "x"));
    }

    #[test]
    fn fuzzy_match_is_case_insensitive() {
        assert!(fuzzy_match("AbCdEf", "ace"));
        assert!(fuzzy_match("abcdef", "ACE"));
    }
}
