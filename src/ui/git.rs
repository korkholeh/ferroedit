//! UI: git sidebar panel.
//!
//! The two-column `XY` field of `git status` and the path, in a pane sixteen to
//! thirty-two cells wide (SPEC §30). Long paths are elided from the *left*,
//! because the file name is the part that identifies a row and the directories
//! in front of it are the part a reader can infer.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::git::GitAvailability;
use crate::app::App;
use crate::git::models::Change;
use crate::ui::explorer::selection_style;
use crate::ui::scrollbar;
use crate::ui::theme::Theme;
use unicode_width::UnicodeWidthStr;

/// `XY` plus the space after it.
const CODE_WIDTH: usize = 3;

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let focused = app.focus == FocusTarget::GitPanel;
    let block = Block::new()
        .borders(Borders::TOP | Borders::RIGHT)
        .border_style(theme.border_for(focused))
        .title(Span::styled(title(app), theme.panel_title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    // On the pane's own right border, like the explorer's above it (ADR-052).
    scrollbar::render(
        frame,
        Rect::new(area.right() - 1, inner.y, 1, inner.height),
        theme,
        focused,
        app.git.entries().len(),
        inner.height as usize,
        app.git.scroll,
    );

    if let Some(message) = empty_message(app) {
        // Wrapped, not clipped: the sidebar is sixteen cells wide at its
        // narrowest and "Not a Git repository" is twenty-one, so a single line
        // would leave the user with "Not a Git reposi".
        let note = Paragraph::new(Line::from(Span::styled(
            format!(" {message}"),
            Style::new().fg(theme.dim),
        )))
        .wrap(Wrap { trim: false });
        frame.render_widget(note, inner);
        return;
    }

    let width = inner.width as usize;
    let rows: Vec<Line> = app
        .git
        .entries()
        .iter()
        .enumerate()
        .skip(app.git.scroll)
        .take(inner.height as usize)
        .map(|(index, entry)| {
            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", entry.codes()),
                    Style::new().fg(status_color(theme, entry.primary())),
                ),
                Span::raw(elide_left(
                    &entry.path.display().to_string(),
                    width.saturating_sub(CODE_WIDTH + 1),
                )),
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

/// ` Git — main ↑1 (4) `: the head, how far it has drifted from its upstream,
/// and how many files have changed.
///
/// The count is in the title rather than on a "Changes" row of its own, which
/// is what SPEC §30 sketches: the panel is four to ten rows tall, and a header
/// would spend one of them on a word (ADR-031).
fn title(app: &App) -> String {
    if !app.git.is_repository() {
        return " Git ".to_string();
    }
    // SPEC §34 asks for `Pushing…` while a network operation runs. It goes in
    // the title rather than only on the status bar because a notification
    // expires after four seconds and a push over a slow link does not: the
    // title is the one place that can say "still running" for as long as it is
    // true (ADR-033).
    if let Some(progress) = app.git.busy() {
        return format!(" Git — {progress} ");
    }
    let status = &app.git.status;
    let mut title = format!(" Git — {}", status.head_label());
    // An unfinished operation outranks the ahead/behind counts: it is a state
    // the user has to finish, and it stays true after every conflicted file has
    // been staged (SPEC §35).
    if let Some(operation) = status.operation {
        title.push_str(&format!(" [{}]", operation.label()));
    }
    if status.ahead > 0 {
        title.push_str(&format!(" ↑{}", status.ahead));
    }
    if status.behind > 0 {
        title.push_str(&format!(" ↓{}", status.behind));
    }
    match status.entries.len() {
        0 => {}
        count if status.truncated => title.push_str(&format!(" ({count}+)")),
        count => title.push_str(&format!(" ({count})")),
    }
    title.push(' ');
    title
}

/// What the panel says instead of a list, and `None` when it has one to show.
fn empty_message(app: &App) -> Option<&str> {
    match &app.git.availability {
        // SPEC §28 names this string; it is what the panel must say when the
        // workspace is an ordinary directory.
        GitAvailability::NotARepository => Some("Not a Git repository"),
        GitAvailability::Unknown => Some("Reading…"),
        GitAvailability::Unavailable(why) => Some(why),
        GitAvailability::Repository if app.git.status.is_clean() => Some("working tree clean"),
        GitAvailability::Repository => None,
    }
}

/// Cuts a path down to `width` cells, keeping the end.
///
/// `…src/ui/git.rs` says more about which file a row is than
/// `src/commands/exe…` does, and the sidebar is too narrow to keep both ends.
/// The explorer's title borrows it for the same reason: the tail of a project
/// path is the part that names it.
pub(crate) fn elide_left(path: &str, width: usize) -> String {
    if path.width() <= width {
        return path.to_string();
    }
    if width <= 1 {
        return "…".repeat(width);
    }
    // Cells, not chars: a path can hold a wide character like any other name.
    let mut kept = String::new();
    let mut used = 1; // the ellipsis
    for grapheme in path.chars().rev() {
        let next = used + grapheme.to_string().width();
        if next > width {
            break;
        }
        used = next;
        kept.insert(0, grapheme);
    }
    format!("…{kept}")
}

fn status_color(theme: &Theme, change: Change) -> ratatui::style::Color {
    match change {
        Change::Modified | Change::TypeChanged => theme.git_modified,
        Change::Added | Change::Copied => theme.git_added,
        Change::Deleted => theme.git_deleted,
        Change::Renamed => theme.git_renamed,
        Change::Unmerged => theme.git_conflict,
        Change::Untracked | Change::Unmodified => theme.git_untracked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_path_is_left_alone() {
        assert_eq!(elide_left("src/main.rs", 20), "src/main.rs");
        assert_eq!(elide_left("src/main.rs", 11), "src/main.rs");
    }

    #[test]
    fn a_long_path_keeps_its_end() {
        assert_eq!(elide_left("src/commands/execute.rs", 10), "…xecute.rs");
        assert_eq!(elide_left("src/commands/execute.rs", 10).width(), 10);
    }

    #[test]
    fn a_wide_character_is_never_cut_in_half() {
        // Three cells for two ideographs would leave one cell over; the row
        // keeps whole characters and comes out one cell short instead.
        let elided = elide_left("日本語.txt", 4);
        assert!(elided.width() <= 4, "{elided:?}");
        assert!(elided.starts_with('…'));
    }

    #[test]
    fn a_panel_with_no_room_for_a_name_does_not_panic() {
        assert_eq!(elide_left("src/main.rs", 1), "…");
        assert_eq!(elide_left("src/main.rs", 0), "");
    }
}
