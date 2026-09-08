//! The window of the document the editor pane shows, and the scrolling that
//! keeps the cursor inside it.
//!
//! Headless like the rest of `editor/`: the caller passes the size of the pane
//! in cells as a `wrap::Layout`, so this module never has to know what a
//! terminal is.
//!
//! The window is measured in *rows* and not in lines (ADR-057): with wrapping
//! on, one line can be forty of them, and a top-of-window that could only name
//! a line would not be able to show the middle of such a line at all. A pane
//! that does not wrap is the same arithmetic with one row per line.

use crate::editor::coords::{self, VisualCol};
use crate::editor::document::Document;
use crate::editor::wrap::{self, Layout, Row};

/// Cells between the line number and the first column of text.
const GUTTER_PADDING: usize = 2;

/// Columns one press of the horizontal scroll moves the window by.
///
/// A cell at a time would be a key held down for a minified line; a whole pane
/// would lose the reader's place. Eight is a step that stays readable — the
/// same one the diff viewer uses.
pub const HORIZONTAL_STEP: usize = 8;

/// How many cells the line-number gutter takes for a document of `line_count`
/// lines, including the padding.
///
/// Rendering and mouse hit-testing both call this, so a click and the character
/// under it cannot disagree about where the text starts. The minimum of two
/// digits stops the gutter from jumping as a file grows past nine lines.
pub fn gutter_width(line_count: usize) -> usize {
    let mut digits = 1;
    let mut n = line_count.max(1);
    while n >= 10 {
        n /= 10;
        digits += 1;
    }
    digits.max(2) + GUTTER_PADDING
}

/// One row of the pane: which line it belongs to, which of that line's rows it
/// is, and the columns it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleRow {
    pub line: usize,
    /// The row's place within its line. Only the first one is given a line
    /// number in the gutter — a continuation row is the same line.
    pub index: usize,
    pub row: Row,
}

impl VisibleRow {
    pub fn is_first(&self) -> bool {
        self.index == 0
    }
}

/// The scroll offsets of one editor pane: the row at the top of the window,
/// and how far it has been moved sideways.
///
/// One per tab rather than one per app: switching back to a tab should show it
/// where it was left, not where the other file happened to be scrolled to.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub top_line: usize,
    /// Which row *of* `top_line` is at the top of the window. Always 0 when
    /// lines do not wrap, because then a line is one row.
    pub top_row: usize,
    pub left_col: VisualCol,
}

impl Viewport {
    fn top(&self) -> (usize, usize) {
        (self.top_line, self.top_row)
    }

    fn set_top(&mut self, (line, row): (usize, usize)) {
        self.top_line = line;
        self.top_row = row;
    }

    /// The rows the pane draws, top to bottom.
    ///
    /// The renderer and the mouse both go through this, so a click and the
    /// character drawn under it cannot disagree about which row is which.
    pub fn visible(&self, document: &Document, layout: Layout) -> Vec<VisibleRow> {
        let mut out = Vec::with_capacity(layout.height);
        let mut line = self.top_line;
        let mut skip = self.top_row;
        while out.len() < layout.height && line < document.line_count() {
            let text = document.line(line);
            for (index, row) in layout
                .rows(&text, document.tab_width())
                .enumerate()
                .skip(skip)
            {
                if out.len() == layout.height {
                    break;
                }
                out.push(VisibleRow { line, index, row });
            }
            skip = 0;
            line += 1;
        }
        out
    }

    /// Scrolls by the smallest amount that brings the cursor into view.
    ///
    /// Minimal scrolling is what makes arrow keys feel right: a cursor already
    /// on screen must not move the text at all, and one just off the edge must
    /// move it by exactly one row or column.
    pub fn follow_cursor(&mut self, document: &Document, layout: Layout) {
        self.clamp(document, layout);
        let cursor = document.cursor();
        let col = document.cursor_visual_col();
        let line = document.line(cursor.line);
        let (index, _) = layout.row_at_col(&line, document.tab_width(), col);
        let target = (cursor.line, index);

        if layout.height > 0 {
            if target < self.top() {
                self.set_top(target);
            } else {
                // The last row the window can show from where it stands. A
                // cursor past it pulls the window down by the difference,
                // which is what "scroll by the minimum" means in rows.
                let bottom = self.step(document, layout, self.top(), layout.height as isize - 1);
                if target > bottom {
                    let top = self.step(document, layout, target, 1 - layout.height as isize);
                    self.set_top(top);
                }
            }
        }

        // A wrapped pane has no sideways axis: every column of every line is
        // already on screen.
        if layout.wraps() {
            self.left_col = VisualCol(0);
            return;
        }
        if layout.width > 0 {
            self.left_col = VisualCol(self.left_col.0.min(col.0));
            self.left_col = VisualCol(
                self.left_col
                    .0
                    .max((col.0 + 1).saturating_sub(layout.width)),
            );
        }
    }

    /// Scrolls vertically without touching the cursor — what the mouse wheel
    /// does, and the reason the cursor may legitimately be off screen.
    pub fn scroll_rows(&mut self, delta: i16, document: &Document, layout: Layout) {
        let top = self.step(document, layout, self.top(), delta as isize);
        self.set_top(top);
    }

    /// Moves the window sideways, for the lines that are wider than the pane.
    ///
    /// Bounded by the widest line *on screen* rather than by the widest in the
    /// document: measuring a five-megabyte file on every press of a key is not
    /// a thing a scroll may cost, and a window that stops where the text it is
    /// showing ends is the behaviour the user can see anyway.
    pub fn scroll_columns(&mut self, delta: i16, document: &Document, layout: Layout) {
        if layout.wraps() {
            self.left_col = VisualCol(0);
            return;
        }
        let target = self.left_col.0 as isize + delta as isize * HORIZONTAL_STEP as isize;
        let limit = self.widest_visible(document, layout).saturating_sub(1);
        self.left_col = VisualCol((target.max(0) as usize).min(limit));
    }

    /// Clamps after an edit that shortened the document, so a deleted tail
    /// cannot leave the pane scrolled past the end.
    pub fn clamp(&mut self, document: &Document, layout: Layout) {
        self.top_line = self.top_line.min(document.line_count().saturating_sub(1));
        if layout.wraps() {
            let rows = layout.row_count(&document.line(self.top_line), document.tab_width());
            self.top_row = self.top_row.min(rows - 1);
            self.left_col = VisualCol(0);
        } else {
            self.top_row = 0;
        }
    }

    /// The position `delta` rows away, clamped to the ends of the document.
    fn step(
        &self,
        document: &Document,
        layout: Layout,
        from: (usize, usize),
        delta: isize,
    ) -> (usize, usize) {
        // Without wrapping a line is a row, so the walk is arithmetic and the
        // document is never measured.
        if !layout.wraps() {
            let last_line = document.line_count().saturating_sub(1) as isize;
            let line = (from.0 as isize + delta).clamp(0, last_line.max(0)) as usize;
            return (line, 0);
        }
        wrap::step_rows(from, delta, document.line_count(), |line| {
            layout.row_count(&document.line(line), document.tab_width())
        })
    }

    /// The widest line the window is over, in display columns.
    fn widest_visible(&self, document: &Document, layout: Layout) -> usize {
        let end = self
            .top_line
            .saturating_add(layout.height.max(1))
            .min(document.line_count());
        (self.top_line..end)
            .map(|line| coords::line_width(&document.line(line), document.tab_width()).0)
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> Document {
        Document::from_text(text, None)
    }

    /// `count` numbered lines, for the vertical cases.
    fn lines(count: usize) -> Document {
        let text = (0..count)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        document(&text)
    }

    #[test]
    fn the_gutter_never_narrows_below_two_digits() {
        assert_eq!(gutter_width(0), 4);
        assert_eq!(gutter_width(9), 4);
        assert_eq!(gutter_width(99), 4);
        assert_eq!(gutter_width(100), 5);
        assert_eq!(gutter_width(12_345), 7);
    }

    #[test]
    fn a_cursor_already_in_view_does_not_move_the_text() {
        let mut document = lines(40);
        document.place_cursor(15, VisualCol(6));
        let mut viewport = Viewport {
            top_line: 10,
            top_row: 0,
            left_col: VisualCol(4),
        };
        viewport.follow_cursor(&document, Layout::plain(20, 40));
        assert_eq!(viewport.top_line, 10);
        assert_eq!(viewport.left_col, VisualCol(4));
    }

    #[test]
    fn scrolling_up_to_the_cursor_moves_by_the_minimum() {
        let mut document = lines(40);
        document.place_cursor(9, VisualCol(0));
        let mut viewport = Viewport {
            top_line: 10,
            ..Viewport::default()
        };
        viewport.follow_cursor(&document, Layout::plain(20, 40));
        assert_eq!(viewport.top_line, 9);
    }

    #[test]
    fn scrolling_down_keeps_the_cursor_on_the_last_row() {
        let mut document = lines(40);
        let mut viewport = Viewport::default();
        document.place_cursor(19, VisualCol(0));
        viewport.follow_cursor(&document, Layout::plain(20, 40));
        assert_eq!(viewport.top_line, 0, "the last visible row");
        document.place_cursor(20, VisualCol(0));
        viewport.follow_cursor(&document, Layout::plain(20, 40));
        assert_eq!(viewport.top_line, 1);
    }

    #[test]
    fn a_long_line_scrolls_horizontally_and_comes_back() {
        let mut document = document(&"x".repeat(200));
        let layout = Layout::plain(20, 40);
        let mut viewport = Viewport::default();
        document.place_cursor(0, VisualCol(100));
        viewport.follow_cursor(&document, layout);
        assert_eq!(viewport.left_col, VisualCol(61));
        document.place_cursor(0, VisualCol(0));
        viewport.follow_cursor(&document, layout);
        assert_eq!(viewport.left_col, VisualCol(0));
    }

    #[test]
    fn a_zero_sized_pane_is_left_alone_rather_than_underflowing() {
        let document = lines(40);
        let mut viewport = Viewport {
            top_line: 3,
            top_row: 0,
            left_col: VisualCol(3),
        };
        viewport.follow_cursor(&document, Layout::plain(0, 0));
        assert_eq!(viewport.top_line, 3);
        assert_eq!(viewport.left_col, VisualCol(3));
    }

    #[test]
    fn the_wheel_stops_at_the_last_line() {
        let document = lines(8);
        let layout = Layout::plain(4, 40);
        let mut viewport = Viewport::default();
        viewport.scroll_rows(100, &document, layout);
        assert_eq!(viewport.top_line, 7);
        viewport.scroll_rows(-100, &document, layout);
        assert_eq!(viewport.top_line, 0);
    }

    #[test]
    fn clamping_pulls_the_view_back_into_a_shortened_document() {
        let document = lines(5);
        let mut viewport = Viewport {
            top_line: 40,
            ..Viewport::default()
        };
        viewport.clamp(&document, Layout::plain(20, 40));
        assert_eq!(viewport.top_line, 4);
    }

    // --- wrapping -----------------------------------------------------------

    /// One paragraph of five rows at width 10, then a short line.
    fn paragraph() -> Document {
        document("aaaa bbbb cccc dddd eeee ffff\nshort\n")
    }

    #[test]
    fn a_wrapped_line_is_drawn_as_the_rows_it_takes() {
        let document = paragraph();
        let viewport = Viewport::default();
        let rows = viewport.visible(&document, Layout::wrapping(10, 10));
        assert_eq!(rows.len(), 5, "three rows of the paragraph, then two lines");
        assert_eq!(rows[0].line, 0);
        assert!(rows[0].is_first());
        assert_eq!(rows[1].line, 0);
        assert!(!rows[1].is_first(), "a continuation row of the same line");
        assert_eq!(rows[3].line, 1, "the short line");
    }

    #[test]
    fn the_window_can_start_in_the_middle_of_a_wrapped_line() {
        let document = paragraph();
        let viewport = Viewport {
            top_line: 0,
            top_row: 2,
            left_col: VisualCol(0),
        };
        let rows = viewport.visible(&document, Layout::wrapping(2, 10));
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].line, rows[0].index), (0, 2));
        assert_eq!((rows[1].line, rows[1].index), (1, 0));
    }

    #[test]
    fn the_wheel_walks_rows_and_not_lines_when_the_text_wraps() {
        let document = paragraph();
        let layout = Layout::wrapping(2, 10);
        let mut viewport = Viewport::default();
        viewport.scroll_rows(2, &document, layout);
        assert_eq!((viewport.top_line, viewport.top_row), (0, 2));
        viewport.scroll_rows(1, &document, layout);
        assert_eq!(
            (viewport.top_line, viewport.top_row),
            (1, 0),
            "the next line"
        );
        viewport.scroll_rows(-1, &document, layout);
        assert_eq!(
            (viewport.top_line, viewport.top_row),
            (0, 2),
            "and back into the middle of the wrapped one"
        );
    }

    #[test]
    fn the_cursor_at_the_end_of_a_wrapped_line_pulls_the_window_by_rows() {
        let mut document = paragraph();
        let layout = Layout::wrapping(2, 10);
        let mut viewport = Viewport::default();
        // The last column of the paragraph, which is on its third row.
        document.place_cursor(0, VisualCol(29));
        viewport.follow_cursor(&document, layout);
        assert_eq!(
            (viewport.top_line, viewport.top_row),
            (0, 1),
            "the last two rows of the paragraph"
        );
    }

    #[test]
    fn a_wrapped_pane_never_scrolls_sideways() {
        let mut document = document(&"x".repeat(200));
        let layout = Layout::wrapping(4, 10);
        let mut viewport = Viewport {
            left_col: VisualCol(40),
            ..Viewport::default()
        };
        viewport.scroll_columns(3, &document, layout);
        assert_eq!(viewport.left_col, VisualCol(0));
        document.place_cursor(0, VisualCol(150));
        viewport.follow_cursor(&document, layout);
        assert_eq!(viewport.left_col, VisualCol(0));
    }

    #[test]
    fn the_sideways_window_stops_at_the_widest_line_on_screen() {
        let document = document("short\n0123456789012345678901234\n");
        let layout = Layout::plain(4, 10);
        let mut viewport = Viewport::default();
        viewport.scroll_columns(1, &document, layout);
        assert_eq!(viewport.left_col, VisualCol(HORIZONTAL_STEP));
        viewport.scroll_columns(100, &document, layout);
        assert_eq!(
            viewport.left_col,
            VisualCol(24),
            "the last column of the file"
        );
        viewport.scroll_columns(-100, &document, layout);
        assert_eq!(viewport.left_col, VisualCol(0));
    }
}
