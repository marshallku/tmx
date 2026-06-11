use anyhow::{Context, Result};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::project::Project;
use crate::tmux;
use crate::ui::picker::{PickerConfig, PickerItem, cursor_prefix, run_picker};
use crate::ui::theme;

impl PickerItem for Project {
    fn fuzzy_key(&self) -> &str {
        &self.name
    }

    fn render(&self, is_cursor: bool) -> Line<'static> {
        let (cursor_marker, name_style) = cursor_prefix(is_cursor);

        let mut spans = vec![
            cursor_marker,
            Span::styled(self.name.clone(), name_style),
            Span::raw(" "),
            Span::styled(
                format!("[{}]", self.project_type.as_str()),
                theme::type_style(self.project_type),
            ),
            Span::raw(" "),
            Span::styled(self.git_branch.clone(), theme::branch_style()),
        ];

        if self.git_dirty {
            spans.push(Span::styled(" ●", theme::dirty_style()));
        }
        if self.git_ahead > 0 {
            spans.push(Span::styled(
                format!(" ↑{}", self.git_ahead),
                Style::default().fg(theme::GREEN),
            ));
        }
        if self.git_behind > 0 {
            spans.push(Span::styled(
                format!(" ↓{}", self.git_behind),
                Style::default().fg(theme::RED),
            ));
        }

        Line::from(spans)
    }
}

pub fn run_project_selector(projects: Vec<Project>) -> Result<Option<Project>> {
    run_picker(
        projects,
        PickerConfig {
            title: "tmx",
            search_placeholder: "Search projects...",
            empty_message: "No projects found",
            item_noun: "projects",
        },
    )
}

pub fn open_project(project: &Project) -> Result<()> {
    let session_name = tmux::sanitize_session_name(&project.name);
    if tmux::session_exists(&session_name) {
        return tmux::switch_session(&session_name).context("switch tmux session");
    }
    let path_str = project.path.to_string_lossy();
    tmux::create_session(&session_name, &path_str).context("create tmux session")?;
    tmux::apply_layout(&session_name, project.project_type.as_str()).ok();
    tmux::switch_session(&session_name).context("switch tmux session")
}
