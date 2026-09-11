//! UI: the CSV table view (SPEC §65).
//!
//! The editor pane, drawn for a tab whose file is being read as columns rather
//! than as lines: a header row of names, a rule under it, and one row per
//! record. The parse happened once, in `app::table::TableView::sync`
//! (ARCHITECTURE invariant 4), so this file decides columns and colours and
//! nothing else.
//!
//! A cell being typed into is drawn over the grid by `ui::field`, the same
//! one-line editor a dialog prompt and the search bar use: the terminal's own
//! cursor sits in it, so the caret in a cell blinks and is found by a screen
//! reader exactly as the caret in the text is (SPEC §65, ADR-063).
//!
//! Cells are clipped, never wrapped. A wrapped cell makes rows of different
//! heights, and a grid whose rows are different heights is one where the eye
//! can no longer follow a column down the screen — which is the only thing the
//! table view has over the text it replaced. The whole value is still in the
//! buffer, one keystroke away in the text view.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::table::{CellEditor, TableView, CELL_GAP, HEADER_ORDINAL, HEADER_ROWS};
use crate::app::App;
use crate::editor::csv::Record;
use crate::ui::field;
use crate::ui::scrollbar;
use crate::ui::theme::Theme;
use unicode_width::UnicodeWidthStr;

/// Drawn between one column and the next, and between the row numbers and the
/// first column.
const SEPARATOR: &str = "│";

/// Put in the last cell of a value too wide for its column.
const ELLIPSIS: char = '…';

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(tab) = app.active() else { return };
    let Some(view) = tab.table.as_ref() else {
        return;
    };
    // The editor is drawn underneath, so the ground has to be taken back before
    // anything is written on it. `Clear` takes it back to the *terminal's*
    // default rather than to the theme's, so the ground is repainted after it —
    // otherwise the grid keeps the terminal's own colours whatever theme is
    // chosen, which is what the whole frame is painted for in `ui::render`.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::new().style(Style::new().bg(theme.background).fg(theme.foreground)),
        area,
    );
    if area.height == 0 || area.width == 0 {
        return;
    }

    // The rightmost column is the scrollbar's, exactly as it is for the text
    // (ADR-052), and it is reserved whether or not a bar is drawn in it.
    let track = Rect::new(area.right().saturating_sub(1), area.y, 1, area.height);
    let body = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    let focused = app.focus == FocusTarget::Editor;

    if view.table.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  Nothing to show as a table",
            Style::new().fg(theme.dim),
        )));
        frame.render_widget(empty, body);
        return;
    }

    let gutter = view.gutter();
    let width = (body.width as usize).saturating_sub(gutter);
    let columns = drawn_columns(view, width);

    let mut lines = vec![
        header(view, &columns, gutter, focused, theme),
        rule(&columns, gutter, width, theme),
    ];
    let height = TableView::pane_rows(body.height as usize);
    for row in view.scroll..(view.scroll + height).min(view.table.len()) {
        lines.push(record(view, &columns, gutter, row, focused, theme));
    }
    frame.render_widget(Paragraph::new(lines), body);
    // Over the grid, and after it: the cell being typed into is the one place
    // on the pane where the buffer is not what is drawn.
    if let Some(editing) = view.editor.as_ref() {
        editing_cell(frame, view, editing, &columns, body, gutter, focused, theme);
    }

    scrollbar::render(
        frame,
        track,
        theme,
        focused,
        view.table.len(),
        height,
        view.scroll,
    );
}

/// The cell a point of the drawn pane is over: a row of the *window* — `None`
/// for the header — and a column of the whole table.
///
/// It rebuilds the same columns `render` drew, from the same rect, for the same
/// reason `ui::statusbar::zones` does: what is on screen and what a click lands
/// on are then computed by one function and cannot drift apart. The header is a
/// record of the file and is clicked like one (SPEC §65); the rule under it and
/// the row numbers are chrome, and a click on them selects nothing.
pub fn cell_at(app: &App, area: Rect, at: Position) -> Option<(Option<usize>, usize)> {
    let view = app.active()?.table.as_ref()?;
    let body = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if !body.contains(at) {
        return None;
    }
    let row = match (at.y - body.y) as usize {
        0 => None,
        // The rule between the names and the values belongs to neither.
        1 => return None,
        row => Some(row - HEADER_ROWS),
    };
    let gutter = view.gutter();
    let width = (body.width as usize).saturating_sub(gutter);
    let mut x = (at.x - body.x) as usize;
    x = x.checked_sub(gutter)?;
    let mut used = 0;
    for (index, cell) in drawn_columns(view, width) {
        // The separator drawn in front of the column counts as the column: a
        // click on the rule between two of them has to choose one, and the one
        // to its right is the one the pointer is heading for.
        used += cell + CELL_GAP;
        if x < used {
            return Some((row, index));
        }
    }
    None
}

/// The record whose *number* a point is on, for a click in the row-number
/// gutter — the gesture that selects a whole row (SPEC §65).
///
/// `None` when the point is not in the gutter, or is on the two rows of chrome
/// above the records: the gutter is blank there, and a click on blank chrome
/// selects nothing.
pub fn record_at(app: &App, area: Rect, at: Position) -> Option<Option<usize>> {
    let view = app.active()?.table.as_ref()?;
    let body = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if !body.contains(at) {
        return None;
    }
    if ((at.x - body.x) as usize) >= view.gutter() {
        return None;
    }
    let row = ((at.y - body.y) as usize).checked_sub(HEADER_ROWS)?;
    Some(Some(row))
}

/// Draws the cell being typed into, over the grid it belongs to.
///
/// A cell scrolled out of the window draws nothing: the value is still being
/// edited, and the moment the selection is brought back to it the field is on
/// screen again.
#[allow(clippy::too_many_arguments)]
fn editing_cell(
    frame: &mut Frame,
    view: &TableView,
    editing: &CellEditor,
    columns: &[(usize, usize)],
    body: Rect,
    gutter: usize,
    focused: bool,
    theme: &Theme,
) {
    let row = match editing.record {
        Record::Header => 0,
        Record::Row(row) if row >= view.scroll => (row - view.scroll) + HEADER_ROWS,
        Record::Row(_) => return,
    };
    if row >= body.height as usize {
        return;
    }
    let mut x = gutter;
    for (index, width) in columns {
        // The separator sits in front of the cell, so the value starts after it.
        x += CELL_GAP;
        if *index == editing.column {
            let rect = Rect::new(
                body.x + x as u16,
                body.y + row as u16,
                (*width as u16).min(body.width.saturating_sub(x as u16)),
                1,
            );
            field::render(frame, &editing.field, rect, theme.selection, focused);
            return;
        }
        x += width;
    }
}

/// The columns that fit in `width`, starting at the one the view is scrolled
/// to, with the cells each of them is drawn in.
///
/// The last one is kept even when it only partly fits: a grid that dropped a
/// half-visible column would leave a band of empty cells at the right edge, and
/// the half column is what tells the user there is more to the right.
fn drawn_columns(view: &TableView, width: usize) -> Vec<(usize, usize)> {
    let mut columns = Vec::new();
    let mut used = 0;
    for (index, cell) in view.table.widths.iter().enumerate().skip(view.first_column) {
        if used >= width {
            break;
        }
        let room = (width - used).saturating_sub(CELL_GAP);
        columns.push((index, (*cell).min(room)));
        used += cell + CELL_GAP;
    }
    columns
}

/// The header row: the first record of the file, drawn as the names of the
/// columns under it (SPEC §65).
fn header(
    view: &TableView,
    columns: &[(usize, usize)],
    gutter: usize,
    focused: bool,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(gutter))];
    for (index, width) in columns {
        spans.push(separator(theme));
        let name = view.table.column_name(*index).into_owned();
        // Bold and in the panel-title colour: the header is the file's own
        // first row drawn as a heading, so it reads as one rather than as the
        // first record — until the selection is on it, when it is a cell like
        // any other and is marked like one (SPEC §65).
        //
        // The name of the column the cursor is *in* is drawn in the ordinary
        // text colour instead, still bold: a wide grid is read by looking up
        // from a cell to ask what it is, and every heading looking the same
        // made that a count along the row. It is a lift out of the dim, not a
        // second highlight — the cell itself is what is marked (ADR-081).
        let heading = if *index == view.column {
            theme
                .panel_title
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD)
        } else {
            theme.panel_title.add_modifier(Modifier::BOLD)
        };
        let style = match cell_style(view, HEADER_ORDINAL, *index, focused, theme) {
            Some(style) => style.add_modifier(Modifier::BOLD),
            None => heading,
        };
        // The heading takes its column's alignment, so a numeric column's name
        // sits over its digits rather than away from them.
        spans.push(Span::styled(fit(&name, *width, align(view, *index)), style));
    }
    Line::from(spans)
}

/// The rule under the header, drawn the full width of the columns so the eye
/// has a line to start reading the records from.
fn rule(columns: &[(usize, usize)], gutter: usize, width: usize, theme: &Theme) -> Line<'static> {
    let drawn: usize = columns
        .iter()
        .map(|(_, cell)| cell + CELL_GAP)
        .sum::<usize>()
        .min(width);
    Line::from(vec![
        Span::raw(" ".repeat(gutter)),
        Span::styled("─".repeat(drawn), Style::new().fg(theme.border)),
    ])
}

/// One record: its number in the gutter, then a cell per drawn column.
fn record(
    view: &TableView,
    columns: &[(usize, usize)],
    gutter: usize,
    row: usize,
    focused: bool,
    theme: &Theme,
) -> Line<'static> {
    let selected_row = row == view.row && !view.on_header;
    let mut spans = vec![Span::styled(
        format!("{:>width$}  ", row + 1, width = gutter.saturating_sub(2)),
        Style::new().fg(theme.line_number),
    )];
    for (index, width) in columns {
        spans.push(separator(theme));
        let text = fit(view.table.cell(row, *index), *width, align(view, *index));
        // The cell the user is on is the selection; the rest of its row is
        // marked more faintly, because a wide table is read along a row and a
        // row with nothing on it is one the eye loses between two screenfuls.
        let style = match cell_style(view, row as isize, *index, focused, theme) {
            Some(style) => style,
            // The rest of the selected row is marked more faintly: a wide table
            // is read along a row, and a row with nothing on it is one the eye
            // loses between two screenfuls.
            None if selected_row => theme.selection_unfocused,
            None => Style::new().fg(theme.foreground),
        };
        spans.push(Span::styled(text, style));
    }
    Line::from(spans)
}

/// Which end of the column a value in it is drawn against.
///
/// The parse decided this, once, when the file was read (ARCHITECTURE
/// invariant 4): a renderer that sniffed types as it drew would answer it for
/// every visible cell on every frame, and could answer differently for two
/// cells of one column.
fn align(view: &TableView, column: usize) -> Align {
    if view.table.is_numeric(column) {
        Align::Right
    } else {
        Align::Left
    }
}

/// How a cell is marked, or `None` when it is neither the cursor's nor part of
/// a selected block.
///
/// The cursor cell is the selection colour and the block around it is the one
/// selected *text* is drawn in — the same two the editor uses, so a block of
/// cells and a run of characters read as the same kind of thing (SPEC §65).
fn cell_style(
    view: &TableView,
    record: isize,
    column: usize,
    focused: bool,
    theme: &Theme,
) -> Option<Style> {
    if record == view.ordinal() && column == view.column {
        return Some(if focused {
            theme.selection
        } else {
            theme.selection_unfocused
        });
    }
    view.selection()
        .filter(|range| range.contains(record, column))
        .map(|_| theme.editor_selection)
}

fn separator(theme: &Theme) -> Span<'static> {
    Span::styled(SEPARATOR, Style::new().fg(theme.border))
}

/// Which end of its column a value is padded against (SPEC §65, ADR-081).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Right,
}

/// A value in exactly `width` cells: padded when it is short, cut with an
/// ellipsis when it is long.
///
/// Cells rather than characters throughout, so a column of Cyrillic or of CJK
/// text lines up with the ones beside it. A line break inside a quoted field is
/// drawn as `⏎`: the row is one row, and a value that put a newline through the
/// grid would break every column to the right of it.
///
/// A number is padded on the *left*, so a column of them lines up on its last
/// digit and two of them can be compared by their length — which is the whole
/// reason a spreadsheet does it (ADR-081). A value too wide for its column is
/// still cut from the right and still ends in an ellipsis, whichever way it is
/// aligned: the front of a number is the part that says how big it is.
fn fit(value: &str, width: usize, align: Align) -> String {
    if width == 0 {
        return String::new();
    }
    let value: String = value
        .chars()
        .map(|ch| match ch {
            '\n' => '⏎',
            '\t' => ' ',
            ch => ch,
        })
        .collect();
    if value.width() <= width {
        let pad = " ".repeat(width - value.width());
        return match align {
            Align::Left => format!("{value}{pad}"),
            Align::Right => format!("{pad}{value}"),
        };
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in value.chars() {
        let cell = ch.to_string().width();
        if used + cell > width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += cell;
    }
    out.push(ELLIPSIS);
    used += 1;
    // A wide character next to the ellipsis can leave the cell a column short.
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_value_is_padded_to_the_column() {
        assert_eq!(fit("ab", 5, Align::Left), "ab   ");
    }

    #[test]
    fn a_long_value_is_cut_with_an_ellipsis_and_still_fills_the_column() {
        let cut = fit("abcdefgh", 5, Align::Left);
        assert_eq!(cut, "abcd…");
        assert_eq!(cut.width(), 5);
    }

    #[test]
    fn a_wide_character_never_overflows_its_column() {
        // Each of these is two cells, so three of them do not fit in five.
        let cut = fit("日本語", 5, Align::Left);
        assert_eq!(cut.width(), 5, "{cut:?}");
        assert!(cut.ends_with('…') || cut.ends_with(' '), "{cut:?}");
    }

    #[test]
    fn a_newline_inside_a_field_is_drawn_as_one_cell() {
        assert_eq!(fit("a\nb", 3, Align::Left), "a⏎b");
    }

    /// A number lines up on its last digit, so two of them can be compared by
    /// their length (ADR-081).
    #[test]
    fn a_number_is_padded_on_the_left_and_a_string_on_the_right() {
        assert_eq!(fit("12", 5, Align::Right), "   12");
        assert_eq!(fit("12", 5, Align::Left), "12   ");
        assert_eq!(fit("abc", 5, Align::Right), "  abc");
    }

    /// A value too wide for its column is cut from the right whichever way it
    /// is aligned: the front of a number is what says how big it is.
    #[test]
    fn a_value_too_wide_for_its_column_is_cut_from_the_right_either_way() {
        assert_eq!(fit("123456", 4, Align::Right), "123…");
        assert_eq!(fit("123456", 4, Align::Left), "123…");
    }

    #[test]
    fn a_zero_width_column_draws_nothing() {
        assert_eq!(fit("abc", 0, Align::Left), "");
    }
}
