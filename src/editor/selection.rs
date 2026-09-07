//! Anchor/head selection over document coordinates.
//!
//! A selection is two positions in the same `(line, char_in_line)` space the
//! cursor lives in, so every motion the cursor already knows how to make is
//! also a way to extend a selection (SPEC §14). Only linear selection exists;
//! column/block selection is explicitly out of the MVP.
//!
//! `Document` stores the anchor alone and the head *is* the cursor, so the two
//! can never drift apart. This module is what turns that pair into the ordered
//! range that deleting, copying and rendering all want.

use crate::editor::coords::CharIdx;

/// A place in a document: which line, and how many `char`s into it.
///
/// Ordering is the reading order of the text — line first, then column — which
/// is what makes `min`/`max` the start and end of a selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub column: CharIdx,
}

impl Position {
    pub fn new(line: usize, column: CharIdx) -> Self {
        Self { line, column }
    }
}

/// The selected span, anchored where the user started and headed where the
/// cursor is now. The head may be *before* the anchor: selecting backwards is
/// the same object with the two ends swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Position,
    pub head: Position,
}

/// The part of one line a selection covers, in char indices.
///
/// `to_line_end` marks a line whose newline is inside the selection: rendering
/// draws one extra cell for it, which is how a multi-line selection shows that
/// the line break itself is included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpan {
    pub start: CharIdx,
    pub end: CharIdx,
    pub to_line_end: bool,
}

impl Selection {
    pub fn new(anchor: Position, head: Position) -> Self {
        Self { anchor, head }
    }

    /// An anchor and head at the same place — a caret, not a selection.
    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    /// The earlier of the two ends, in reading order.
    pub fn start(self) -> Position {
        self.anchor.min(self.head)
    }

    /// The later of the two ends, in reading order.
    pub fn end(self) -> Position {
        self.anchor.max(self.head)
    }

    /// What of `line` is selected, given that line's length in chars.
    ///
    /// `None` for a line outside the selection, so a renderer can ask this of
    /// every visible line without first working out which ones are involved.
    pub fn line_span(self, line: usize, line_len: CharIdx) -> Option<LineSpan> {
        if self.is_empty() {
            return None;
        }
        let (start, end) = (self.start(), self.end());
        if line < start.line || line > end.line {
            return None;
        }
        Some(LineSpan {
            start: if line == start.line {
                start.column.min(line_len)
            } else {
                CharIdx(0)
            },
            end: if line == end.line {
                end.column.min(line_len)
            } else {
                line_len
            },
            to_line_end: line < end.line,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: usize, column: usize) -> Position {
        Position::new(line, CharIdx(column))
    }

    fn selection(from: Position, to: Position) -> Selection {
        Selection::new(from, to)
    }

    #[test]
    fn positions_order_by_line_then_column() {
        assert!(
            at(1, 0) > at(0, 99),
            "a later line wins whatever the column"
        );
        assert!(at(0, 3) > at(0, 2));
    }

    #[test]
    fn a_backwards_selection_has_the_same_range_as_a_forwards_one() {
        let forwards = selection(at(1, 2), at(3, 4));
        let backwards = selection(at(3, 4), at(1, 2));
        assert_eq!(forwards.start(), backwards.start());
        assert_eq!(forwards.end(), backwards.end());
        assert_eq!(backwards.start(), at(1, 2));
    }

    #[test]
    fn an_anchor_on_the_head_is_not_a_selection() {
        assert!(selection(at(2, 5), at(2, 5)).is_empty());
        assert_eq!(selection(at(2, 5), at(2, 5)).line_span(2, CharIdx(9)), None);
    }

    #[test]
    fn a_selection_inside_one_line_spans_only_that_line() {
        let sel = selection(at(1, 2), at(1, 6));
        assert_eq!(
            sel.line_span(1, CharIdx(10)),
            Some(LineSpan {
                start: CharIdx(2),
                end: CharIdx(6),
                to_line_end: false,
            })
        );
        assert_eq!(sel.line_span(0, CharIdx(10)), None);
        assert_eq!(sel.line_span(2, CharIdx(10)), None);
    }

    #[test]
    fn a_multi_line_selection_covers_whole_middle_lines() {
        let sel = selection(at(1, 3), at(3, 2));
        // First line: from the anchor to its end, newline included.
        assert_eq!(
            sel.line_span(1, CharIdx(8)),
            Some(LineSpan {
                start: CharIdx(3),
                end: CharIdx(8),
                to_line_end: true,
            })
        );
        // Middle line: all of it.
        assert_eq!(
            sel.line_span(2, CharIdx(5)),
            Some(LineSpan {
                start: CharIdx(0),
                end: CharIdx(5),
                to_line_end: true,
            })
        );
        // Last line: up to the head, and the newline is *not* selected.
        assert_eq!(
            sel.line_span(3, CharIdx(9)),
            Some(LineSpan {
                start: CharIdx(0),
                end: CharIdx(2),
                to_line_end: false,
            })
        );
    }

    #[test]
    fn a_span_never_runs_past_the_line_it_is_on() {
        // The head's column belongs to a longer line; a shorter one clamps.
        let sel = selection(at(0, 0), at(0, 40));
        assert_eq!(
            sel.line_span(0, CharIdx(4)),
            Some(LineSpan {
                start: CharIdx(0),
                end: CharIdx(4),
                to_line_end: false,
            })
        );
    }
}
