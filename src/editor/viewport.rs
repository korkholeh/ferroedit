//! The window of the document the editor pane shows, and the scrolling that
//! keeps the cursor inside it.
//!
//! Headless like the rest of `editor/`: the caller passes the size of the pane
//! in cells, so this module never has to know what a terminal is.

use crate::editor::coords::VisualCol;

/// Cells between the line number and the first column of text.
const GUTTER_PADDING: usize = 2;

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

/// The scroll offsets of one editor pane, in lines and display columns.
///
/// One per tab rather than one per app: switching back to a tab should show it
/// where it was left, not where the other file happened to be scrolled to.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub top_line: usize,
    pub left_col: VisualCol,
}

impl Viewport {
    /// Scrolls by the smallest amount that brings the cursor into view.
    ///
    /// Minimal scrolling is what makes arrow keys feel right: a cursor already
    /// on screen must not move the text at all, and one just off the edge must
    /// move it by exactly one line or column.
    pub fn follow_cursor(&mut self, line: usize, col: VisualCol, height: usize, width: usize) {
        if height > 0 {
            self.top_line = self.top_line.min(line);
            self.top_line = self.top_line.max((line + 1).saturating_sub(height));
        }
        if width > 0 {
            self.left_col = VisualCol(self.left_col.0.min(col.0));
            self.left_col = VisualCol(self.left_col.0.max((col.0 + 1).saturating_sub(width)));
        }
    }

    /// Scrolls vertically without touching the cursor — what the mouse wheel
    /// does, and the reason the cursor may legitimately be off screen.
    pub fn scroll_lines(&mut self, delta: i16, last_line: usize) {
        self.top_line = if delta < 0 {
            self.top_line.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (self.top_line + delta as usize).min(last_line)
        };
    }

    /// Clamps after an edit that shortened the document, so a deleted tail
    /// cannot leave the pane scrolled past the end.
    pub fn clamp(&mut self, line_count: usize) {
        self.top_line = self.top_line.min(line_count.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut viewport = Viewport {
            top_line: 10,
            left_col: VisualCol(4),
        };
        viewport.follow_cursor(15, VisualCol(6), 20, 40);
        assert_eq!(viewport.top_line, 10);
        assert_eq!(viewport.left_col, VisualCol(4));
    }

    #[test]
    fn scrolling_up_to_the_cursor_moves_by_the_minimum() {
        let mut viewport = Viewport {
            top_line: 10,
            left_col: VisualCol(0),
        };
        viewport.follow_cursor(9, VisualCol(0), 20, 40);
        assert_eq!(viewport.top_line, 9);
    }

    #[test]
    fn scrolling_down_keeps_the_cursor_on_the_last_row() {
        let mut viewport = Viewport::default();
        viewport.follow_cursor(19, VisualCol(0), 20, 40);
        assert_eq!(viewport.top_line, 0, "the last visible row");
        viewport.follow_cursor(20, VisualCol(0), 20, 40);
        assert_eq!(viewport.top_line, 1);
    }

    #[test]
    fn a_long_line_scrolls_horizontally_and_comes_back() {
        let mut viewport = Viewport::default();
        viewport.follow_cursor(0, VisualCol(100), 20, 40);
        assert_eq!(viewport.left_col, VisualCol(61));
        viewport.follow_cursor(0, VisualCol(0), 20, 40);
        assert_eq!(viewport.left_col, VisualCol(0));
    }

    #[test]
    fn a_zero_sized_pane_is_left_alone_rather_than_underflowing() {
        let mut viewport = Viewport {
            top_line: 3,
            left_col: VisualCol(3),
        };
        viewport.follow_cursor(0, VisualCol(0), 0, 0);
        assert_eq!(viewport.top_line, 3);
        assert_eq!(viewport.left_col, VisualCol(3));
    }

    #[test]
    fn the_wheel_stops_at_the_last_line() {
        let mut viewport = Viewport::default();
        viewport.scroll_lines(100, 7);
        assert_eq!(viewport.top_line, 7);
        viewport.scroll_lines(-100, 7);
        assert_eq!(viewport.top_line, 0);
    }

    #[test]
    fn clamping_pulls_the_view_back_into_a_shortened_document() {
        let mut viewport = Viewport {
            top_line: 40,
            left_col: VisualCol(0),
        };
        viewport.clamp(5);
        assert_eq!(viewport.top_line, 4);
    }
}
