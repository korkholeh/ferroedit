//! UI: status bar.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::notifications::NotificationKind;
use crate::app::App;
use crate::editor::document::LineEnding;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    frame.render_widget(Block::new().style(theme.status_bar), area);

    // A live notification takes over the left half; the file/cursor readout on
    // the right stays put so it never moves under the user's eye.
    let left = match app.notifications.current() {
        Some(notification) => Line::from(Span::styled(
            format!(" {}", notification.message),
            theme.status_bar.fg(kind_color(theme, notification.kind)),
        )),
        None => Line::from(vec![
            Span::styled(
                format!(" {}", app.active().map_or("—", |t| t.document.title())),
                theme.status_bar,
            ),
            Span::styled(
                if app.active().is_some_and(|t| t.document.is_dirty()) {
                    " ●"
                } else {
                    ""
                },
                theme.status_bar.fg(theme.tab_dirty),
            ),
        ]),
    };

    let document = app.active().map(|t| &t.document);
    // The grammar the highlighter actually chose, not a guess from the
    // extension: it is the one that also reads file names and shebangs, and a
    // status bar that disagreed with the colours would be worse than none.
    let language = app
        .active()
        .map_or("Plain Text", |tab| tab.highlights.language());
    // A file that is too large to colour still says what it is, with a marker
    // so that "why is this not highlighted?" has an answer on screen and not
    // only in a notification that has already expired.
    let plain = match app.active().and_then(|tab| tab.highlights.disabled()) {
        Some(_) => " (plain)",
        None => "",
    };
    let branch = app.git.branch_label();
    // One-based, and counted in user-perceived characters rather than in chars
    // or cells, because that is the number a human arrives at (SPEC §38).
    let line = document.map_or(1, |d| d.cursor().line + 1);
    let column = document.map_or(1, |d| d.cursor_display_col().0);
    // The line ending is shown only when it is not the default one: it matters
    // when it is CRLF, and the status bar is already crowded at 60 columns.
    let ending = match document.map(|d| d.line_ending()) {
        Some(LineEnding::Crlf) => "CRLF   ",
        _ => "",
    };
    // Shown only while something is selected: an always-present "Sel 0" would
    // cost three columns of an already crowded bar for no information.
    let selected = match document.map_or(0, |d| d.selected_len()) {
        0 => String::new(),
        count => format!("Sel {count}   "),
    };
    // The leading space keeps the readout off a clipped notification.
    let right = format!(
        " Ln {line}, Col {column}   {selected}UTF-8   {ending}{language}{plain}   {branch}   {} ",
        app.focus.label()
    );

    // The two halves are separate widgets over one row, so the row is split
    // rather than overdrawn: on a narrow terminal the notification is clipped
    // instead of being written over by the readout.
    let right_width = (right.width() as u16).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);

    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            right,
            theme.status_bar.fg(theme.dim),
        )))
        .right_aligned(),
        right_area,
    );
}

fn kind_color(theme: &Theme, kind: NotificationKind) -> ratatui::style::Color {
    match kind {
        NotificationKind::Info => theme.info,
        NotificationKind::Warning => theme.warning,
        NotificationKind::Error => theme.error,
    }
}
