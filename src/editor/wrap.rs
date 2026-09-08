//! Breaking a line into the rows it is drawn on (SPEC §58).
//!
//! Headless like the rest of `editor/`: a row is a range of the line and the
//! display columns it covers, and nothing here knows that a terminal exists.
//!
//! The columns a row reports are the *line's* columns, not the screen's: the
//! second row of a wrapped line starts at column 80 and not at column 0. That
//! is what lets the renderer draw a wrapped row with the same function it uses
//! for a scrolled one — it is the same window over the same line — and it is
//! what keeps a tab elastic to the tab stop it would have had unwrapped.

use unicode_segmentation::UnicodeSegmentation;

use crate::editor::coords::{self, CharIdx, VisualCol};

/// One drawn row of a logical line: the text on it, and the columns it covers.
///
/// `end`/`end_col` are exclusive, and are where the next row of the same line
/// begins. A line that is not wrapped is one row spanning all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub start: CharIdx,
    pub end: CharIdx,
    pub start_col: VisualCol,
    pub end_col: VisualCol,
}

impl Row {
    /// Cells the row occupies on screen.
    pub fn width(&self) -> usize {
        self.end_col.0.saturating_sub(self.start_col.0)
    }

    /// Whether `col` falls on this row.
    pub fn holds(&self, col: VisualCol) -> bool {
        col.0 >= self.start_col.0 && col.0 < self.end_col.0
    }
}

/// How the editor pane lays a document out: how many rows of it are on screen,
/// how wide a row is, and whether a line longer than that is broken into
/// several rows or left running off the right edge (ADR-057).
///
/// The width matters either way: wrapped it is where a line breaks, and
/// unwrapped it is how far the pane can be scrolled sideways before the cursor
/// leaves it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Layout {
    pub height: usize,
    pub width: usize,
    pub wrap: bool,
}

impl Layout {
    /// A pane that does not wrap: every line is one row, however long it is.
    pub fn plain(height: usize, width: usize) -> Self {
        Self {
            height,
            width,
            wrap: false,
        }
    }

    /// A pane that breaks every line at `width` columns.
    pub fn wrapping(height: usize, width: usize) -> Self {
        Self {
            height,
            width,
            wrap: true,
        }
    }

    /// Whether lines are actually broken here.
    ///
    /// A width of zero is not a pane a line can be broken into, so it never
    /// wraps: a terminal narrower than its own gutter has to draw something
    /// rather than loop.
    pub fn wraps(&self) -> bool {
        self.wrap && self.width > 0
    }

    /// The rows `line` is drawn on under this layout.
    pub fn rows<'a>(&self, line: &'a str, tab_width: usize) -> Rows<'a> {
        Rows::new(line, self.wraps().then_some(self.width), tab_width)
    }

    /// How many rows `line` takes. Never zero: an empty line is still a row.
    pub fn row_count(&self, line: &str, tab_width: usize) -> usize {
        self.rows(line, tab_width).count().max(1)
    }

    /// The row `col` falls on, with its index — the last row when the column
    /// is past the end of the line, which is where a cursor at the end of a
    /// full row sits.
    pub fn row_at_col(&self, line: &str, tab_width: usize, col: VisualCol) -> (usize, Row) {
        let mut last = (0, self.empty_row());
        for (index, row) in self.rows(line, tab_width).enumerate() {
            if row.holds(col) {
                return (index, row);
            }
            last = (index, row);
        }
        last
    }

    /// The row at `index`, clamped to the last one the line has.
    pub fn row_at(&self, line: &str, tab_width: usize, index: usize) -> (usize, Row) {
        let mut last = (0, self.empty_row());
        for (at, row) in self.rows(line, tab_width).enumerate() {
            last = (at, row);
            if at == index {
                break;
            }
        }
        last
    }

    /// The row an empty line is drawn on — the answer for a document with
    /// nothing in it, which still has a cursor in it.
    fn empty_row(&self) -> Row {
        Row {
            start: CharIdx(0),
            end: CharIdx(0),
            start_col: VisualCol(0),
            end_col: VisualCol(0),
        }
    }
}

/// The position `delta` drawn rows away from `from`, clamped to the ends of a
/// document of `line_count` lines.
///
/// `(line, row)` is how both the viewport's top and the cursor travel once
/// lines can wrap, and they travel the same way — so the walk lives here
/// rather than once in each of them. `rows_in` says how many rows a line
/// takes; it is a callback because the two callers reach their text
/// differently, and because a walk of four rows must measure four lines and
/// not the document.
pub fn step_rows(
    from: (usize, usize),
    delta: isize,
    line_count: usize,
    rows_in: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let last_line = line_count.saturating_sub(1);
    let (mut line, mut row) = from;
    for _ in 0..delta.unsigned_abs() {
        if delta > 0 {
            if row + 1 < rows_in(line) {
                row += 1;
            } else if line < last_line {
                line += 1;
                row = 0;
            } else {
                break;
            }
        } else if row > 0 {
            row -= 1;
        } else if line > 0 {
            line -= 1;
            row = rows_in(line).saturating_sub(1);
        } else {
            break;
        }
    }
    (line, row)
}

/// The rows of one line, produced as they are needed.
///
/// Lazy rather than a `Vec` because a single line can be megabytes long: the
/// renderer wants the four rows under the viewport and must not pay for the
/// forty thousand below them.
pub struct Rows<'a> {
    line: &'a str,
    width: Option<usize>,
    tab_width: usize,
    /// Where the next row starts: byte offset, char offset and column, which
    /// have to travel together because the three coordinate systems part
    /// company on the first non-ASCII character (ARCHITECTURE §4).
    byte: usize,
    char: usize,
    col: usize,
    done: bool,
}

impl<'a> Rows<'a> {
    fn new(line: &'a str, width: Option<usize>, tab_width: usize) -> Self {
        Self {
            line,
            width,
            tab_width,
            byte: 0,
            char: 0,
            col: 0,
            done: false,
        }
    }
}

impl Iterator for Rows<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        if self.done {
            return None;
        }
        let start = CharIdx(self.char);
        let start_col = VisualCol(self.col);

        // Unwrapped: the whole line is the row, whatever it costs to measure.
        let Some(width) = self.width else {
            self.done = true;
            return Some(Row {
                start,
                end: coords::char_len(self.line),
                start_col,
                end_col: coords::line_width(self.line, self.tab_width),
            });
        };

        let mut byte = self.byte;
        let mut chr = self.char;
        let mut col = self.col;
        // Where the row would break on a word boundary: the first cluster of
        // the word being read, when there is a word before it on this row.
        let mut candidate: Option<(usize, usize, usize)> = None;
        let mut after_space = false;
        let mut broke = false;

        for (offset, cluster) in self.line[self.byte..].grapheme_indices(true) {
            let at = self.byte + offset;
            let cell = coords::cluster_width(cluster, VisualCol(col), self.tab_width);
            let space = cluster.chars().all(char::is_whitespace);
            if !space && after_space && col > start_col.0 {
                candidate = Some((at, chr, col));
            }
            // A row always takes at least one cluster: a pane two cells wide
            // must still draw a three-cell character rather than break for
            // ever in front of it.
            if col > start_col.0 && col + cell > start_col.0 + width {
                broke = true;
                if let Some((word_byte, word_char, word_col)) = candidate {
                    byte = word_byte;
                    chr = word_char;
                    col = word_col;
                }
                break;
            }
            after_space = space;
            byte = at + cluster.len();
            chr += cluster.chars().count();
            col += cell;
        }

        self.byte = byte;
        self.char = chr;
        self.col = col;
        self.done = !broke;
        Some(Row {
            start,
            end: CharIdx(chr),
            start_col,
            end_col: VisualCol(col),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::coords::DEFAULT_TAB_WIDTH as TAB;

    /// The rows as the text on them, which is what the wrapping is about.
    fn wrapped(line: &str, width: usize) -> Vec<String> {
        Layout::wrapping(10, width)
            .rows(line, TAB)
            .map(|row| {
                line.chars()
                    .skip(row.start.0)
                    .take(row.end.0 - row.start.0)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_line_that_fits_is_one_row() {
        assert_eq!(wrapped("fn main() {", 40), vec!["fn main() {"]);
    }

    #[test]
    fn an_empty_line_is_still_a_row() {
        assert_eq!(wrapped("", 40), vec![""]);
    }

    #[test]
    fn an_unwrapped_layout_never_breaks_a_line() {
        let line = "a".repeat(500);
        let rows: Vec<_> = Layout::plain(10, 80).rows(&line, TAB).collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].end_col, VisualCol(500));
    }

    #[test]
    fn a_break_lands_between_words_and_keeps_the_space_on_the_row_it_ends() {
        assert_eq!(
            wrapped("the quick brown fox", 10),
            vec!["the quick ", "brown fox"]
        );
    }

    #[test]
    fn a_word_longer_than_the_row_is_broken_where_the_row_ends() {
        assert_eq!(
            wrapped("aa suuuuuuperlong", 6),
            vec!["aa ", "suuuuu", "uperlo", "ng"]
        );
    }

    #[test]
    fn the_columns_a_row_reports_are_the_lines_own() {
        let rows: Vec<_> = Layout::wrapping(10, 10)
            .rows("the quick brown fox", TAB)
            .collect();
        assert_eq!(rows[0].start_col, VisualCol(0));
        assert_eq!(rows[0].end_col, VisualCol(10));
        assert_eq!(rows[1].start_col, VisualCol(10), "not back at zero");
        assert_eq!(rows[1].end_col, VisualCol(19));
    }

    #[test]
    fn a_tab_keeps_the_stop_it_would_have_had_unwrapped() {
        // The tab at column 5 runs to the next stop at 8, and the word after it
        // is where the row breaks — so the second row starts at column 8 and
        // its text is drawn at the columns it would have had unwrapped.
        let rows: Vec<_> = Layout::wrapping(10, 10).rows("abcde\tfghij", TAB).collect();
        assert_eq!(rows[0].end_col, VisualCol(8));
        assert_eq!(rows[1].start_col, VisualCol(8));
        assert_eq!(rows[1].start, CharIdx(6), "the tab stayed on the first row");
    }

    #[test]
    fn a_wide_character_that_would_straddle_the_edge_goes_to_the_next_row() {
        // Each of these is two cells: four of them fill a row of nine.
        assert_eq!(wrapped("日本語漢字", 9), vec!["日本語漢", "字"]);
    }

    #[test]
    fn a_row_takes_one_cluster_however_narrow_the_pane_is() {
        assert_eq!(wrapped("日本", 1), vec!["日", "本"]);
    }

    #[test]
    fn a_line_that_exactly_fills_a_row_does_not_grow_an_empty_one() {
        assert_eq!(wrapped("abcdefghij", 10), vec!["abcdefghij"]);
    }

    #[test]
    fn the_row_a_column_falls_on_is_the_one_that_covers_it() {
        let layout = Layout::wrapping(10, 10);
        let line = "the quick brown fox";
        assert_eq!(layout.row_at_col(line, TAB, VisualCol(0)).0, 0);
        assert_eq!(layout.row_at_col(line, TAB, VisualCol(9)).0, 0);
        assert_eq!(layout.row_at_col(line, TAB, VisualCol(10)).0, 1);
        // Past the end of the line: the cursor at the end of the last row.
        assert_eq!(layout.row_at_col(line, TAB, VisualCol(99)).0, 1);
    }

    #[test]
    fn the_row_at_an_index_is_clamped_to_the_last_one() {
        let layout = Layout::wrapping(10, 10);
        let line = "the quick brown fox";
        assert_eq!(layout.row_at(line, TAB, 0).1.start, CharIdx(0));
        assert_eq!(layout.row_at(line, TAB, 1).1.start, CharIdx(10));
        assert_eq!(layout.row_at(line, TAB, 99), layout.row_at(line, TAB, 1));
    }

    /// Three lines, two rows each, walked in both directions.
    #[test]
    fn stepping_by_rows_crosses_lines_and_stops_at_both_ends() {
        let rows_in = |_line: usize| 2;
        assert_eq!(step_rows((0, 0), 1, 3, rows_in), (0, 1));
        assert_eq!(step_rows((0, 1), 1, 3, rows_in), (1, 0));
        assert_eq!(step_rows((1, 0), -1, 3, rows_in), (0, 1));
        assert_eq!(step_rows((0, 0), 3, 3, rows_in), (1, 1));
        assert_eq!(step_rows((0, 0), 99, 3, rows_in), (2, 1), "the last row");
        assert_eq!(step_rows((2, 1), -99, 3, rows_in), (0, 0), "and the first");
    }

    #[test]
    fn a_row_count_is_never_zero() {
        assert_eq!(Layout::wrapping(10, 10).row_count("", TAB), 1);
        assert_eq!(Layout::plain(10, 80).row_count("", TAB), 1);
        assert_eq!(
            Layout::wrapping(10, 10).row_count("the quick brown fox", TAB),
            2
        );
    }

    /// The rows have to add up to the line: no cluster dropped, none drawn
    /// twice, whatever the widths involved.
    #[test]
    fn the_rows_of_a_line_are_the_line() {
        let line = "let s = \"Привіт, світе\";\tfn\tmain() 日本語 aaaaaaaaaaaaaaaaaaaa";
        for width in 1..30 {
            let rejoined: String = wrapped(line, width).concat();
            assert_eq!(rejoined, line, "at width {width}");
        }
    }
}
