use std::collections::HashMap;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};

use crate::project::ProjectType;

/// Named color slots the UI draws from. Defaults to Catppuccin Mocha;
/// individual slots are overridable via the `[theme]` config section
/// (`violet = "#cba6f7"` …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub violet: Color,
    pub purple: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
    pub blue: Color,
    pub orange: Color,
    pub muted: Color,
    pub surface: Color,
    pub text: Color,
}

impl Default for Palette {
    fn default() -> Self {
        // Catppuccin Mocha palette.
        Self {
            violet: Color::Rgb(0xcb, 0xa6, 0xf7),
            purple: Color::Rgb(0x89, 0xb4, 0xfa),
            green: Color::Rgb(0xa6, 0xe3, 0xa1),
            red: Color::Rgb(0xf3, 0x8b, 0xa8),
            yellow: Color::Rgb(0xf9, 0xe2, 0xaf),
            blue: Color::Rgb(0x89, 0xb4, 0xfa),
            orange: Color::Rgb(0xfa, 0xb3, 0x87),
            muted: Color::Rgb(0x6c, 0x70, 0x86),
            surface: Color::Rgb(0x31, 0x32, 0x44),
            text: Color::Rgb(0xcd, 0xd6, 0xf4),
        }
    }
}

static PALETTE: OnceLock<Palette> = OnceLock::new();

/// Install the user palette from `[theme]` config overrides. First call
/// wins; without a call the default palette applies. Invalid values are
/// reported on stderr and skipped — a typo shouldn't blank the whole UI.
pub fn init(overrides: &HashMap<String, String>) {
    PALETTE.set(build_palette(overrides)).ok();
}

/// The active palette. Falls back to the default when [`init`] was never
/// called (unit tests, plain CLI paths that render nothing).
pub fn palette() -> &'static Palette {
    PALETTE.get_or_init(Palette::default)
}

fn build_palette(overrides: &HashMap<String, String>) -> Palette {
    let mut p = Palette::default();
    for (key, value) in overrides {
        let Some(color) = parse_hex_color(value) else {
            eprintln!(
                "tmx: warning: invalid theme color {key} = \"{value}\" (expected \"#rrggbb\")"
            );
            continue;
        };
        match key.as_str() {
            "violet" => p.violet = color,
            "purple" => p.purple = color,
            "green" => p.green = color,
            "red" => p.red = color,
            "yellow" => p.yellow = color,
            "blue" => p.blue = color,
            "orange" => p.orange = color,
            "muted" => p.muted = color,
            "surface" => p.surface = color,
            "text" => p.text = color,
            _ => eprintln!(
                "tmx: warning: unknown theme key '{key}' (known: violet, purple, green, red, yellow, blue, orange, muted, surface, text)"
            ),
        }
    }
    p
}

/// Parse `#rrggbb` into a Color. Returns `None` on any other shape.
fn parse_hex_color(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

pub fn title_style() -> Style {
    Style::default()
        .fg(palette().violet)
        .add_modifier(Modifier::BOLD)
}

pub fn selected_style() -> Style {
    Style::default()
        .fg(palette().violet)
        .add_modifier(Modifier::BOLD)
}

pub fn normal_style() -> Style {
    Style::default().fg(palette().text)
}

pub fn muted_style() -> Style {
    Style::default().fg(palette().muted)
}

pub fn branch_style() -> Style {
    Style::default().fg(palette().purple)
}

pub fn dirty_style() -> Style {
    Style::default().fg(palette().yellow)
}

pub fn status_bar_style() -> Style {
    Style::default().fg(palette().muted).bg(palette().surface)
}

pub fn type_style(project_type: ProjectType) -> Style {
    let p = palette();
    let color = match project_type {
        ProjectType::Go => p.blue,
        ProjectType::Node => p.green,
        ProjectType::Rust => p.orange,
        ProjectType::Python => p.yellow,
        ProjectType::Generic => p.muted,
    };
    Style::default().fg(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_color_valid() {
        assert_eq!(
            parse_hex_color("#cba6f7"),
            Some(Color::Rgb(0xcb, 0xa6, 0xf7))
        );
        assert_eq!(parse_hex_color("#000000"), Some(Color::Rgb(0, 0, 0)));
        assert_eq!(
            parse_hex_color("#FFFFFF"),
            Some(Color::Rgb(0xff, 0xff, 0xff))
        );
    }

    #[test]
    fn parse_hex_color_invalid_shapes() {
        assert_eq!(parse_hex_color("cba6f7"), None); // missing '#'
        assert_eq!(parse_hex_color("#fff"), None); // short form unsupported
        assert_eq!(parse_hex_color("#gggggg"), None); // non-hex
        assert_eq!(parse_hex_color(""), None);
        assert_eq!(parse_hex_color("#cba6f7aa"), None); // alpha unsupported
    }

    #[test]
    fn build_palette_applies_known_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("violet".to_string(), "#112233".to_string());
        overrides.insert("text".to_string(), "#445566".to_string());
        let p = build_palette(&overrides);
        assert_eq!(p.violet, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(p.text, Color::Rgb(0x44, 0x55, 0x66));
        // Untouched slots keep their defaults.
        assert_eq!(p.green, Palette::default().green);
    }

    #[test]
    fn build_palette_skips_invalid_and_unknown() {
        let mut overrides = HashMap::new();
        overrides.insert("violet".to_string(), "not-a-color".to_string());
        overrides.insert("nonexistent".to_string(), "#112233".to_string());
        assert_eq!(build_palette(&overrides), Palette::default());
    }
}
