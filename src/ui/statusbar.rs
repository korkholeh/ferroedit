//! UI: status bar.
//!
//! The right-hand readout is built as a list of pieces with a drop order
//! rather than as one format string, because the bar is the one row whose
//! contents outgrow a narrow terminal. At 60 columns the fixed readout used to
//! clip the notification; now the pieces nobody is reading — the encoding, the
//! focus label — leave first and the sentence gets its room back (ADR-039).

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

    let right = readout(app, area.width);

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

/// The left half never shrinks below this. A bar that is all readout and no
/// file name — or no notification — has stopped being a status bar, so the
/// readout gives way rather than the sentence next to it.
///
/// Twenty is `Opened src/main.rs` and a space at either end: the shortest
/// sentence the editor actually says about a file it has just opened.
const MIN_LEFT: u16 = 20;

/// Separator between the readout's pieces.
const GAP: &str = "   ";

/// A piece of the readout, and how readily it goes.
///
/// Lower drops later: the cursor position is what the bar is *for* and is never
/// dropped, and the encoding is the piece that is the same on every file the
/// MVP opens, so it is the first to go.
struct Piece {
    text: String,
    drop_order: u8,
}

fn piece(text: String, drop_order: u8) -> Piece {
    Piece { text, drop_order }
}

/// The right-hand readout, trimmed to what the width allows.
fn readout(app: &App, width: u16) -> String {
    let document = app.active().map(|t| &t.document);
    // One-based, and counted in user-perceived characters rather than in chars
    // or cells, because that is the number a human arrives at (SPEC §38).
    let line = document.map_or(1, |d| d.cursor().line + 1);
    let column = document.map_or(1, |d| d.cursor_display_col().0);
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

    let mut pieces = vec![piece(format!("Ln {line}, Col {column}"), 0)];
    // Shown only while something is selected: an always-present "Sel 0" would
    // cost three columns of an already crowded bar for no information.
    if let Some(count) = document.map(|d| d.selected_len()).filter(|n| *n > 0) {
        pieces.push(piece(format!("Sel {count}"), 1));
    }
    pieces.push(piece("UTF-8".to_string(), 6));
    // The line ending is shown only when it is not the default one: it matters
    // when it is CRLF, and it is the surprising one, so it outlives the pieces
    // that are the same on every file.
    if document.map(|d| d.line_ending()) == Some(LineEnding::Crlf) {
        pieces.push(piece("CRLF".to_string(), 2));
    }
    pieces.push(piece(format!("{language}{plain}"), 4));
    pieces.push(piece(app.git.branch_label().to_string(), 3));
    pieces.push(piece(app.focus.label().to_string(), 5));

    // The leading and trailing spaces keep the readout off a clipped
    // notification and off the right edge.
    let budget = width.saturating_sub(MIN_LEFT) as usize;
    let mut worst = 6;
    loop {
        let text = format!(
            " {} ",
            pieces
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join(GAP)
        );
        if text.width() <= budget || worst == 0 {
            return text;
        }
        pieces.retain(|p| p.drop_order != worst);
        worst -= 1;
    }
}

fn kind_color(theme: &Theme, kind: NotificationKind) -> ratatui::style::Color {
    match kind {
        NotificationKind::Info => theme.info,
        NotificationKind::Warning => theme.warning,
        NotificationKind::Error => theme.error,
    }
}
