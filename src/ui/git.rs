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

/// The line SPEC §28 names for a workspace that is an ordinary directory.
const NOT_A_REPOSITORY: &str = "Not a Git repository";

/// The button under it (ADR-084).
const INIT_BUTTON: &str = "[ git init ]";

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

    // The repository is somewhere above the workspace, so the panel says whose
    // changes it is listing (ADR-085). A row and not the title, because the
    // title has no room for it at the width the sidebar actually is.
    let list = match &app.git.above {
        Some(name) => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(
                        " in {}",
                        elide_left(&format!("{name}/"), inner.width.saturating_sub(4) as usize)
                    ),
                    Style::new().fg(theme.dim),
                ))),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            Rect::new(
                inner.x,
                inner.y + 1,
                inner.width,
                inner.height.saturating_sub(1),
            )
        }
        None => inner,
    };
    if list.height == 0 {
        return;
    }

    // On the pane's own right border, like the explorer's above it (ADR-052).
    scrollbar::render(
        frame,
        Rect::new(area.right() - 1, list.y, 1, list.height),
        theme,
        focused,
        app.git.entries().len(),
        list.height as usize,
        app.git.scroll,
    );

    // A plain directory is a state the user can leave, so the panel offers the
    // way out instead of only naming the state (ADR-084).
    if app.git.availability == GitAvailability::NotARepository {
        render_no_repository(frame, inner, theme, focused);
        return;
    }

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

    let width = list.width as usize;
    let rows: Vec<Line> = app
        .git
        .entries()
        .iter()
        .enumerate()
        .skip(app.git.scroll)
        .take(list.height as usize)
        .map(|(index, entry)| {
            let selected = index == app.git.selected;
            let selection = selection_style(theme, focused);
            let path = elide_left(
                &entry.path.display().to_string(),
                width.saturating_sub(CODE_WIDTH + 1),
            );
            // A span's own foreground wins over the line's, so the status
            // letter has to be *told* the selection's text colour, exactly as
            // the explorer tells a directory name: the Retro theme marks the
            // row under the cursor with a green bar, and a yellow `M` on it is
            // 2.7:1. The letter still says which state the file is in.
            let code_style = match selection.fg.filter(|_| selected) {
                Some(fg) => Style::new().fg(fg),
                None => Style::new().fg(status_color(theme, entry.primary())),
            };
            let code = format!(" {} ", entry.codes());
            let mut spans = vec![
                Span::styled(code.clone(), code_style),
                Span::raw(path.clone()),
            ];
            if selected {
                // Out to the pane's edge, for the reason the explorer's is.
                spans.push(Span::raw(crate::ui::explorer::pad(
                    list.width as usize,
                    code.width() + path.width(),
                )));
                Line::from(spans).style(selection)
            } else {
                Line::from(spans)
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(rows), list);
}

/// Rows of chrome above the first change: the border the title sits on, and the
/// row naming the repository when there is one to name (ADR-085).
///
/// The one answer the renderer, the hit test and the scroll arithmetic all read,
/// because a panel whose click lands a row off from what it drew is worse than
/// one with no note at all.
pub fn header_rows(app: &App) -> u16 {
    1 + u16::from(app.git.above.is_some())
}

/// What the panel says instead of a list in a folder that is not a repository:
/// the sentence SPEC §28 asks for, and under it the one thing that can be done
/// about it.
///
/// `[ git init ]` and not `[ Initialize Repository ]`: the pane is sixteen cells
/// wide at its narrowest, the long label does not fit in it at any width the
/// sidebar actually takes, and the short one is the command the button runs —
/// which is the label a reader can check against what happened. The menu, which
/// has the room, spells it out.
fn render_no_repository(frame: &mut Frame, inner: Rect, theme: &Theme, focused: bool) {
    let width = inner.width as usize;
    let mut lines: Vec<Line> = wrap_words(NOT_A_REPOSITORY, width.saturating_sub(1))
        .into_iter()
        .map(|line| Line::from(Span::styled(format!(" {line}"), Style::new().fg(theme.dim))))
        .collect();
    // A blank row between the state and the button: without it the button
    // reads as the last line of the sentence.
    lines.push(Line::default());
    if inner.height as usize > lines.len() && width.saturating_sub(1) >= INIT_BUTTON.width() {
        lines.push(Line::from(Span::styled(
            format!(" {INIT_BUTTON}"),
            // The panel has one thing to press and no selection to move, so
            // the button is lit whenever the pane that owns it has focus.
            if focused {
                theme.dialog_button_selected
            } else {
                theme.dialog_button
            },
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Where the button landed, so that a click can be tested against the same
/// geometry that drew it — the rule `status_zones` follows for the status bar.
///
/// `None` when there is no button: inside a repository, or in a pane too small
/// to have drawn one.
pub fn init_button(app: &App, panel: Rect) -> Option<Rect> {
    if app.git.availability != GitAvailability::NotARepository {
        return None;
    }
    // The pane's inside: one row of border and title at the top, one column of
    // border and scrollbar at the right.
    let width = panel.width.saturating_sub(1);
    let height = panel.height.saturating_sub(1);
    let text_width = (width as usize).saturating_sub(1);
    if text_width < INIT_BUTTON.width() {
        return None;
    }
    let row = wrap_words(NOT_A_REPOSITORY, text_width).len() + 1;
    if row >= height as usize {
        return None;
    }
    Some(Rect::new(
        panel.x + 1,
        panel.y + 1 + row as u16,
        INIT_BUTTON.width() as u16,
        1,
    ))
}

/// Greedy word wrap, in cells.
///
/// Ours rather than `Wrap`, because the button under the sentence has to be at
/// a row both the drawing and the hit test can name, and a widget that wraps
/// inside `render` can only be asked where it put things by reading the frame
/// back out afterwards.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.width()
        } else {
            current.width() + 1 + word.width()
        };
        if !current.is_empty() && candidate > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// ` Git — main ↑1 (4) `: the head, how far it has drifted from its upstream,
/// and how many files have changed.
///
/// The count is in the title rather than on a "Changes" row of its own, which
/// is what SPEC §30 sketches: the panel is four to ten rows tall, and a header
/// would spend one of them on a word (ADR-031).
///
/// It does *not* name the repository when the workspace is inside one rather
/// than at its root: ` Git — Projects · main (73) ` is twenty-eight cells and
/// the pane is nineteen. That goes on a row of its own (ADR-085).
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
    let mut tail = String::new();
    // An unfinished operation outranks the ahead/behind counts: it is a state
    // the user has to finish, and it stays true after every conflicted file has
    // been staged (SPEC §35).
    if let Some(operation) = status.operation {
        tail.push_str(&format!(" [{}]", operation.label()));
    }
    if status.ahead > 0 {
        tail.push_str(&format!(" ↑{}", status.ahead));
    }
    if status.behind > 0 {
        tail.push_str(&format!(" ↓{}", status.behind));
    }
    match status.entries.len() {
        0 => {}
        count if status.truncated => tail.push_str(&format!(" ({count}+)")),
        count => tail.push_str(&format!(" ({count})")),
    }
    tail.push(' ');

    format!(" Git — {}{tail}", status.head_label())
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
