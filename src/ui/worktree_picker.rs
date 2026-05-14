use anyhow::Result;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::ui::picker::{PickerConfig, PickerItem, run_picker};
use crate::ui::theme;
use crate::worktree::{WorktreeEntry, short_branch};

impl PickerItem for WorktreeEntry {
    fn fuzzy_key(&self) -> &str {
        // Prefer short branch name for the fuzzy key; fall back to path.
        if let Some(b) = self.branch.as_deref() {
            short_branch(b)
        } else {
            // path may not be valid utf8 in pathological cases; PickerItem
            // requires &str so we lean on the lossy view via a stash.
            // We can't easily return a &str into a temporary String here, so
            // tradeoff: fall back to a static placeholder when path isn't utf8.
            self.path.to_str().unwrap_or("(detached)")
        }
    }

    fn render(&self, is_cursor: bool) -> Line<'static> {
        let (cursor_marker, name_style) = if is_cursor {
            (
                Span::styled("▸ ", Style::default().fg(theme::VIOLET)),
                theme::selected_style(),
            )
        } else {
            (Span::raw("  "), theme::normal_style())
        };

        let label = match self.branch.as_deref() {
            Some(b) => short_branch(b).to_string(),
            None => "(detached)".to_string(),
        };

        let mut spans = vec![
            cursor_marker,
            Span::styled(label, name_style),
            Span::styled(format!("  {}", self.path.display()), theme::muted_style()),
        ];
        if self.locked {
            spans.push(Span::styled(
                " [locked]",
                Style::default().fg(theme::VIOLET),
            ));
        }
        if self.prunable {
            spans.push(Span::styled(" [prunable]", theme::muted_style()));
        }
        Line::from(spans)
    }
}

pub fn run_worktree_picker(entries: Vec<WorktreeEntry>) -> Result<Option<WorktreeEntry>> {
    run_picker(
        entries,
        PickerConfig {
            title: "git worktrees",
            search_placeholder: "Search worktrees...",
            empty_message: "No worktrees",
            item_noun: "worktrees",
        },
    )
}
