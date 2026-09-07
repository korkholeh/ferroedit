//! UI: the one-line text field, drawn for a dialog's prompt and for the search
//! bar's two rows.
//!
//! The caret is the terminal's own cursor, as it is in the editor: it blinks
//! the way the user configured it to, and a screen reader follows it. A value
//! longer than the box scrolls so the caret stays visible — a file name is
//! short, but a pasted path is not.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::input_field::InputField;

/// Draws `field` into one row, putting the terminal cursor on its caret when
/// `focused`.
///
/// Only the focused field gets the cursor: with the replace bar open there are
/// two fields on screen, and two carets would be one too many.
pub fn render(frame: &mut Frame, field: &InputField, row: Rect, style: Style, focused: bool) {
    if row.width == 0 || row.height == 0 {
        return;
    }

    let before: String = field.value.chars().take(field.cursor.0).collect();
    let caret_col = before.width();
    // Scroll by whole cells: the caret is kept inside the box, and the text
    // slides under it.
    let offset = caret_col.saturating_sub(row.width as usize - 1);
    let visible: String = field
        .value
        .chars()
        .scan(0usize, |col, ch| {
            let at = *col;
            *col += ch.to_string().width();
            Some((at, ch))
        })
        .filter(|(at, _)| *at >= offset && *at < offset + row.width as usize)
        .map(|(_, ch)| ch)
        .collect();

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(visible, style))).style(style),
        row,
    );
    if focused {
        frame.set_cursor_position((row.x + (caret_col - offset) as u16, row.y));
    }
}
