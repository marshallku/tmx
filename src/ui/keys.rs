//! Configurable key bindings for the interactive surfaces.
//!
//! Two independent maps: the pickers (where plain characters type into the
//! search field, so only a few chrome keys are bindable without eating
//! input) and the agents dashboard (no text input — letters are fair game).
//! Overrides come from `[keys.picker]` / `[keys.agents]` in the config and
//! REPLACE the default list for that action.

use std::collections::HashMap;
use std::sync::OnceLock;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::KeysConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Select,
    Up,
    Down,
    /// Agents dashboard: jump the tmux client to the newest attention entry.
    JumpAttention,
    /// Agents dashboard: open the fzf attention picker.
    AttentionPicker,
}

impl Action {
    fn from_config_key(name: &str) -> Option<Self> {
        match name {
            "quit" => Some(Self::Quit),
            "select" => Some(Self::Select),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "jump-attention" => Some(Self::JumpAttention),
            "attention-picker" => Some(Self::AttentionPicker),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeyMap {
    bindings: Vec<(KeyCode, KeyModifiers, Action)>,
}

impl KeyMap {
    /// Resolve a key event to an action. SHIFT is ignored for character
    /// keys — terminals report `A` as `Char('A') + SHIFT`, and the char
    /// itself already carries the case.
    pub fn action(&self, key: &KeyEvent) -> Option<Action> {
        let mods = match key.code {
            KeyCode::Char(_) => key.modifiers.difference(KeyModifiers::SHIFT),
            _ => key.modifiers,
        };
        self.bindings
            .iter()
            .find(|(code, m, _)| *code == key.code && *m == mods)
            .map(|(_, _, action)| *action)
    }
}

/// Parse a key spec: a named key (`esc`, `enter`, `up`, `down`, `tab`,
/// `space`, `backspace`), a single character (`a`, `A`, `?`), or either
/// prefixed with `ctrl-` / `alt-`.
pub fn parse_key(spec: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = spec;
    loop {
        if let Some(r) = rest.strip_prefix("ctrl-") {
            mods |= KeyModifiers::CONTROL;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("alt-") {
            mods |= KeyModifiers::ALT;
            rest = r;
        } else {
            break;
        }
    }
    let code = match rest {
        "esc" => KeyCode::Esc,
        "enter" => KeyCode::Enter,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        single => {
            let mut chars = single.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(c)
        }
    };
    Some((code, mods))
}

const PICKER_DEFAULTS: &[(Action, &[&str])] = &[
    (Action::Quit, &["esc", "ctrl-c"]),
    (Action::Select, &["enter"]),
    (Action::Up, &["up", "ctrl-p"]),
    (Action::Down, &["down", "ctrl-n"]),
];

const AGENTS_DEFAULTS: &[(Action, &[&str])] = &[
    (Action::Quit, &["esc", "q", "ctrl-c"]),
    (Action::Select, &["enter"]),
    (Action::Up, &["up", "k"]),
    (Action::Down, &["down", "j"]),
    (Action::JumpAttention, &["a"]),
    (Action::AttentionPicker, &["A"]),
];

fn build_map(
    surface: &str,
    defaults: &[(Action, &[&str])],
    overrides: &HashMap<String, Vec<String>>,
) -> KeyMap {
    let mut bindings: Vec<(KeyCode, KeyModifiers, Action)> = Vec::new();
    for (action, default_specs) in defaults {
        let override_specs = overrides
            .iter()
            .find(|(name, _)| Action::from_config_key(name) == Some(*action))
            .map(|(_, specs)| specs.as_slice());
        let specs: Vec<&str> = match override_specs {
            Some(list) => list.iter().map(String::as_str).collect(),
            None => default_specs.to_vec(),
        };
        for spec in specs {
            match parse_key(spec) {
                Some((code, mods)) => bindings.push((code, mods, *action)),
                None => eprintln!("tmx: warning: invalid key spec \"{spec}\" in [keys.{surface}]"),
            }
        }
    }
    for name in overrides.keys() {
        if Action::from_config_key(name).is_none() {
            eprintln!(
                "tmx: warning: unknown action '{name}' in [keys.{surface}] (known: quit, select, up, down, jump-attention, attention-picker)"
            );
        }
    }
    KeyMap { bindings }
}

static PICKER_KEYS: OnceLock<KeyMap> = OnceLock::new();
static AGENTS_KEYS: OnceLock<KeyMap> = OnceLock::new();

/// Install the user key maps from `[keys]` config. First call wins;
/// without a call the defaults apply.
pub fn init(cfg: &KeysConfig) {
    PICKER_KEYS
        .set(build_map("picker", PICKER_DEFAULTS, &cfg.picker))
        .ok();
    AGENTS_KEYS
        .set(build_map("agents", AGENTS_DEFAULTS, &cfg.agents))
        .ok();
}

pub fn picker_map() -> &'static KeyMap {
    PICKER_KEYS.get_or_init(|| build_map("picker", PICKER_DEFAULTS, &HashMap::new()))
}

pub fn agents_map() -> &'static KeyMap {
    AGENTS_KEYS.get_or_init(|| build_map("agents", AGENTS_DEFAULTS, &HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parse_key_named_and_chars() {
        assert_eq!(parse_key("esc"), Some((KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(
            parse_key("enter"),
            Some((KeyCode::Enter, KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("a"),
            Some((KeyCode::Char('a'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("A"),
            Some((KeyCode::Char('A'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("ctrl-c"),
            Some((KeyCode::Char('c'), KeyModifiers::CONTROL))
        );
        assert_eq!(parse_key("alt-up"), Some((KeyCode::Up, KeyModifiers::ALT)));
        assert_eq!(
            parse_key("ctrl-alt-x"),
            Some((
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            ))
        );
    }

    #[test]
    fn parse_key_rejects_garbage() {
        assert_eq!(parse_key(""), None);
        assert_eq!(parse_key("notakey"), None);
        assert_eq!(parse_key("ctrl-"), None);
    }

    #[test]
    fn default_picker_map_matches_legacy_keys() {
        let map = build_map("picker", PICKER_DEFAULTS, &HashMap::new());
        let cases = [
            (KeyCode::Esc, KeyModifiers::NONE, Action::Quit),
            (KeyCode::Char('c'), KeyModifiers::CONTROL, Action::Quit),
            (KeyCode::Enter, KeyModifiers::NONE, Action::Select),
            (KeyCode::Up, KeyModifiers::NONE, Action::Up),
            (KeyCode::Char('p'), KeyModifiers::CONTROL, Action::Up),
            (KeyCode::Down, KeyModifiers::NONE, Action::Down),
            (KeyCode::Char('n'), KeyModifiers::CONTROL, Action::Down),
        ];
        for (code, mods, expected) in cases {
            assert_eq!(map.action(&press(code, mods)), Some(expected));
        }
        // Plain characters must stay unbound so they reach the search field.
        assert_eq!(
            map.action(&press(KeyCode::Char('k'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn default_agents_map_matches_legacy_keys() {
        let map = build_map("agents", AGENTS_DEFAULTS, &HashMap::new());
        assert_eq!(
            map.action(&press(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(Action::Down)
        );
        assert_eq!(
            map.action(&press(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Action::Quit)
        );
        assert_eq!(
            map.action(&press(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(Action::JumpAttention)
        );
        // Terminals report 'A' with SHIFT; the map must still match.
        assert_eq!(
            map.action(&press(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Some(Action::AttentionPicker)
        );
        // ctrl-a must NOT trigger the jump (modifier mismatch).
        assert_eq!(
            map.action(&press(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn overrides_replace_default_list_for_that_action() {
        let mut overrides = HashMap::new();
        overrides.insert("quit".to_string(), vec!["ctrl-q".to_string()]);
        let map = build_map("agents", AGENTS_DEFAULTS, &overrides);
        assert_eq!(
            map.action(&press(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        // The default 'q' is gone — replaced, not merged.
        assert_eq!(
            map.action(&press(KeyCode::Char('q'), KeyModifiers::NONE)),
            None
        );
        // Other actions keep their defaults.
        assert_eq!(
            map.action(&press(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(Action::Down)
        );
    }

    #[test]
    fn invalid_specs_are_skipped_not_fatal() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "quit".to_string(),
            vec!["bogus-key".to_string(), "esc".to_string()],
        );
        let map = build_map("picker", PICKER_DEFAULTS, &overrides);
        assert_eq!(
            map.action(&press(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Action::Quit)
        );
    }
}
