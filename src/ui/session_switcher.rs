use anyhow::Result;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tmux::Session;
use crate::ui::picker::{PickerConfig, PickerItem, cursor_prefix, run_picker};
use crate::ui::theme;

impl PickerItem for Session {
    fn fuzzy_key(&self) -> &str {
        &self.name
    }

    fn render(&self, is_cursor: bool) -> Line<'static> {
        let (cursor_marker, name_style) = cursor_prefix(is_cursor);

        let mut spans = vec![
            cursor_marker,
            Span::styled(self.name.clone(), name_style),
            Span::styled(format!(" {} window(s)", self.windows), theme::muted_style()),
        ];
        if self.attached {
            spans.push(Span::styled(" ●", Style::default().fg(theme::GREEN)));
        }
        Line::from(spans)
    }
}

pub fn run_session_switcher(sessions: Vec<Session>) -> Result<Option<Session>> {
    run_picker(
        sessions,
        PickerConfig {
            title: "tmux sessions",
            search_placeholder: "Search sessions...",
            empty_message: "No sessions found",
            item_noun: "sessions",
        },
    )
}
