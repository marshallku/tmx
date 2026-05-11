use ratatui::style::{Color, Modifier, Style};

use crate::project::ProjectType;

// Catppuccin Mocha palette.
pub const VIOLET: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
pub const PURPLE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
pub const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
pub const ORANGE: Color = Color::Rgb(0xfa, 0xb3, 0x87);
pub const MUTED: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub const SURFACE: Color = Color::Rgb(0x31, 0x32, 0x44);
pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);

pub fn title_style() -> Style {
    Style::default().fg(VIOLET).add_modifier(Modifier::BOLD)
}

pub fn selected_style() -> Style {
    Style::default().fg(VIOLET).add_modifier(Modifier::BOLD)
}

pub fn normal_style() -> Style {
    Style::default().fg(TEXT)
}

pub fn muted_style() -> Style {
    Style::default().fg(MUTED)
}

pub fn branch_style() -> Style {
    Style::default().fg(PURPLE)
}

pub fn dirty_style() -> Style {
    Style::default().fg(YELLOW)
}

pub fn status_bar_style() -> Style {
    Style::default().fg(MUTED).bg(SURFACE)
}

pub fn type_style(project_type: ProjectType) -> Style {
    let color = match project_type {
        ProjectType::Go => BLUE,
        ProjectType::Node => GREEN,
        ProjectType::Rust => ORANGE,
        ProjectType::Python => YELLOW,
        ProjectType::Generic => MUTED,
    };
    Style::default().fg(color)
}
