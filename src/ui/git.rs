//! UI: git sidebar panel.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::{App, GitStatusCode};
use crate::ui::explorer::selection_style;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let focused = app.focus == FocusTarget::GitPanel;
    let title = match &app.git.branch {
        Some(branch) => format!(" Git — {branch} "),
        // Phase 10 replaces this with the real "not a repository" state.
        None => " Git — no repository ".to_string(),
    };
    let block = Block::new()
        .borders(Borders::TOP | Borders::RIGHT)
        .border_style(theme.border_for(focused))
        .title(Span::styled(title, theme.panel_title));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.git.entries.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            " working tree clean",
            Style::new().fg(theme.dim),
        )));
        frame.render_widget(empty, inner);
        return;
    }

    let rows: Vec<Line> = app
        .git
        .entries
        .iter()
        .enumerate()
        .skip(app.git.scroll)
        .take(inner.height as usize)
        .map(|(index, entry)| {
            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", entry.status.symbol()),
                    Style::new().fg(status_color(theme, entry.status)),
                ),
                Span::raw(entry.path.clone()),
            ]);
            if index == app.git.selected {
                line.style(selection_style(theme, focused))
            } else {
                line
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(rows), inner);
}

fn status_color(theme: &Theme, status: GitStatusCode) -> ratatui::style::Color {
    match status {
        GitStatusCode::Modified => theme.git_modified,
        GitStatusCode::Added => theme.git_added,
        GitStatusCode::Deleted => theme.git_deleted,
        GitStatusCode::Untracked => theme.git_untracked,
    }
}
