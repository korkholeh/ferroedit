//! The CSV view's state: which dialect, which cell, and where the grid is
//! scrolled (SPEC §65).
//!
//! It hangs off the editor tab rather than being a tab of its own, which is the
//! difference between this and the diff viewer (ADR-053): a diff has no file
//! behind it, while a CSV *is* the file — the same buffer, the same undo
//! history, the same `Ctrl+S`. The table is a second reading of that buffer,
//! rebuilt from it whenever it changes, so nothing here is a copy anybody has
//! to keep in step by hand (ADR-062).
//!
//! The cell the user is on is this struct's `record` and `column`, not the
//! document's cursor: a cell is a range of a line, sometimes of several, and a
//! caret in the text under it is not where the eye is. Typing into a cell opens
//! a `CellEditor` — a one-line field over the cell, exactly the one a dialog
//! prompt uses — and committing it writes the value back through the span the
//! parser recorded, as an ordinary edit of the document (ADR-063).

use crate::app::input_field::InputField;
use crate::editor::csv::{Dialect, Record, Table};
use crate::editor::document::Document;
use crate::editor::viewport::gutter_width;

/// Columns of padding between the row-number gutter and the first cell, and
/// between one cell and the next: a single separator column each.
pub const CELL_GAP: usize = 1;

/// Rows the header takes at the top of the pane: the column names, and the rule
/// under them.
///
/// It is here rather than in `ui::table` because it is what the *scrolling*
/// arithmetic is measured against, and a pane whose renderer and whose page-down
/// disagreed about how tall it is would scroll past a row on every page.
pub const HEADER_ROWS: usize = 2;

impl TableView {
    /// Data rows a pane `height` cells tall can show.
    pub fn pane_rows(height: usize) -> usize {
        height.saturating_sub(HEADER_ROWS)
    }
}

/// The record ordinal the header sits at: one above the first data row.
///
/// The selection is made in these coordinates so that a rectangle can cover the
/// header and the rows under it in one range — the header is a record of the
/// file, and a selection that had to stop above it would be the grid disagreeing
/// with the file about what its first line is (SPEC §65).
pub const HEADER_ORDINAL: isize = -1;

/// A rectangle of cells: records from `first_record` to `last_record`, columns
/// from `first_column` to `last_column`, both ends included.
///
/// Normalised — first is never past last — so a selection made upwards and one
/// made downwards are the same rectangle, which is what the renderer, the
/// clipboard and the status bar all want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    pub first_record: isize,
    pub last_record: isize,
    pub first_column: usize,
    pub last_column: usize,
}

impl CellRange {
    pub fn records(self) -> usize {
        (self.last_record - self.first_record + 1) as usize
    }

    pub fn columns(self) -> usize {
        self.last_column - self.first_column + 1
    }

    /// Whether it covers more than the one cell the cursor is on, which is what
    /// makes it worth drawing and worth reporting.
    pub fn is_range(self) -> bool {
        self.records() > 1 || self.columns() > 1
    }

    pub fn contains(self, record: isize, column: usize) -> bool {
        (self.first_record..=self.last_record).contains(&record)
            && (self.first_column..=self.last_column).contains(&column)
    }
}

/// Where the selection goes when a cell edit is saved (SPEC §65).
///
/// The two directions are the two keys: `Enter` down a column, `Tab` along a
/// row, exactly as they move in every grid anyone has typed into before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStep {
    Stay,
    Down,
    Right,
}

/// A cell being typed into: the field, and the cell it was opened over.
///
/// The cell is carried rather than read back from the view when the edit is
/// committed, because a reparse between the two can move the selection — and a
/// value typed into one cell must never land in another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellEditor {
    pub field: InputField,
    pub record: Record,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableView {
    pub dialect: Dialect,
    pub table: Table,
    /// The document revision `table` was parsed from, or `None` when it has
    /// never been parsed — which is the state a freshly opened tab is in, and
    /// what makes the first `sync` do the work.
    parsed: Option<u64>,
    /// The selected cell: a data row, and a column across the whole table.
    pub row: usize,
    pub column: usize,
    /// Whether the selection is on the header rather than on `row`.
    ///
    /// The header is a record of the file like any other and is edited like one
    /// — renaming a column is an edit a CSV needs — so it is a row the
    /// selection can reach, above the first, rather than chrome (SPEC §65).
    pub on_header: bool,
    /// The cell being typed into, when one is.
    pub editor: Option<CellEditor>,
    /// The cell a selection was started from, in `HEADER_ORDINAL` coordinates.
    ///
    /// The anchor of the document's own selection, in the grid's terms: the
    /// selection is the rectangle between it and the cursor cell, and a plain
    /// move drops it exactly as a plain move drops a text selection (SPEC §65).
    anchor: Option<(isize, usize)>,
    /// First data row drawn, under the header.
    pub scroll: usize,
    /// First column drawn. Whole columns only: a grid scrolled to the middle of
    /// a cell is one whose header no longer sits over its own values.
    pub first_column: usize,
}

impl TableView {
    pub fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            table: Table::default(),
            parsed: None,
            row: 0,
            column: 0,
            on_header: false,
            editor: None,
            anchor: None,
            scroll: 0,
            first_column: 0,
        }
    }

    /// The record the cursor is on.
    pub fn record(&self) -> Record {
        self.record_at(self.ordinal())
    }

    /// The cursor's record as a number, the header included.
    pub fn ordinal(&self) -> isize {
        if self.on_header {
            HEADER_ORDINAL
        } else {
            self.row as isize
        }
    }

    /// The record a number stands for.
    pub fn record_at(&self, ordinal: isize) -> Record {
        if ordinal < 0 {
            Record::Header
        } else {
            Record::Row(ordinal as usize)
        }
    }

    /// The rectangle of cells that is selected, or `None` when nothing beyond
    /// the cursor cell is.
    pub fn selection(&self) -> Option<CellRange> {
        let (record, column) = self.anchor?;
        let cursor = self.ordinal();
        Some(CellRange {
            first_record: record.min(cursor),
            last_record: record.max(cursor),
            first_column: column.min(self.column),
            last_column: column.max(self.column),
        })
    }

    /// Every selected cell, in reading order — records down, columns across.
    ///
    /// The order matters to the two callers: the clipboard writes rows in it,
    /// and a multi-cell write has to reach the buffer in ascending order to be
    /// applied back to front (`Document::replace_matches`).
    pub fn selected_cells(&self) -> Vec<(Record, usize)> {
        let range = self.selection().unwrap_or(CellRange {
            first_record: self.ordinal(),
            last_record: self.ordinal(),
            first_column: self.column,
            last_column: self.column,
        });
        (range.first_record..=range.last_record)
            .flat_map(|record| {
                (range.first_column..=range.last_column).map(move |column| (record, column))
            })
            .map(|(record, column)| (self.record_at(record), column))
            .collect()
    }

    /// Starts a selection at the cursor, if one is not already open. Called by
    /// every extending move, so `Shift` is what makes a selection and nothing
    /// else has to remember to.
    pub fn anchor_here(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some((self.ordinal(), self.column));
        }
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Every cell of the table: the header, the rows, and all the columns.
    pub fn select_all(&mut self) {
        self.anchor = Some((HEADER_ORDINAL, 0));
        self.on_header = false;
        self.row = self.table.len().saturating_sub(1);
        self.column = self.table.columns().saturating_sub(1);
    }

    /// The record the cursor is on, across every column.
    pub fn select_record(&mut self) {
        self.anchor = Some((self.ordinal(), 0));
        self.column = self.table.columns().saturating_sub(1);
    }

    /// The column the cursor is in, from the header to the last record.
    ///
    /// The cursor ends on the *header* and the anchor on the last record, which
    /// is the way round that keeps both things the header is for: the block is
    /// the whole column, ready for `Ctrl+C`, and `F2` still opens the column's
    /// name rather than the value at the bottom of the file (SPEC §65).
    pub fn select_column(&mut self) {
        self.anchor = Some((self.last_ordinal(), self.column));
        self.on_header = true;
        self.scroll = 0;
    }

    /// The last record's number, or the header's when the file has no records:
    /// a one-line file is a header and nothing else.
    fn last_ordinal(&self) -> isize {
        if self.table.rows.is_empty() {
            HEADER_ORDINAL
        } else {
            self.table.len() as isize - 1
        }
    }

    /// The value in the selected cell.
    pub fn value(&self) -> &str {
        self.table.field(self.record(), self.column)
    }

    /// Opens the cell for typing, over `value`.
    ///
    /// The caret lands at the end of it, which is where a user who pressed
    /// `Enter` to correct a value wants it; typing a character instead replaces
    /// the value outright, and passes the replacement in here.
    pub fn begin_edit(&mut self, value: &str) {
        self.editor = Some(CellEditor {
            field: InputField::new(value),
            record: self.record(),
            column: self.column,
        });
    }

    /// Gives up the edit in progress, if there is one.
    pub fn cancel_edit(&mut self) -> bool {
        self.editor.take().is_some()
    }

    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// Re-reads the document if anything it was parsed from has changed.
    ///
    /// Called once a frame from the run loop, next to `sync_highlight` and for
    /// the same reason: an edit, an undo, a reload and a change of dialect can
    /// all invalidate the table, and doing it here means none of them has to
    /// remember to.
    pub fn sync(&mut self, document: &Document) {
        if self.parsed == Some(document.revision()) {
            return;
        }
        let lines = (0..document.line_count()).map(|index| document.line(index));
        self.table = Table::parse(lines, self.dialect);
        self.parsed = Some(document.revision());
        self.clamp_selection();
    }

    /// Changes how the file is read, which invalidates the table.
    pub fn set_dialect(&mut self, dialect: Dialect) {
        if self.dialect == dialect {
            return;
        }
        self.dialect = dialect;
        // The columns are about to be different ones, so the place in them is
        // not worth keeping: the row is, because it is the same record however
        // the record is split.
        self.column = 0;
        self.first_column = 0;
        self.parsed = None;
    }

    /// Moves the selected cell up or down, and brings the window with it.
    ///
    /// The header sits at −1, one above the first record: moving up off the top
    /// of the data lands on it rather than stopping short, and moving down off
    /// it lands back on the first row.
    pub fn move_row(&mut self, delta: isize, height: usize) {
        let last = self.table.len().saturating_sub(1) as isize;
        let current = if self.on_header {
            -1
        } else {
            self.row as isize
        };
        let target = (current + delta).clamp(-1, last);
        self.on_header = target < 0;
        self.row = target.max(0) as usize;
        self.follow_row(height);
    }

    pub fn move_column(&mut self, delta: isize, width: usize) {
        let last = self.table.columns().saturating_sub(1);
        self.column = (self.column as isize + delta).clamp(0, last as isize) as usize;
        self.follow_column(width);
    }

    /// The top-left cell of the table, which is where `Ctrl+Home` goes: the
    /// header's first column, because the header is the first record.
    pub fn home(&mut self) {
        self.row = 0;
        self.column = 0;
        self.on_header = true;
        self.scroll = 0;
        self.first_column = 0;
    }

    pub fn end(&mut self, height: usize) {
        self.row = self.table.len().saturating_sub(1);
        self.on_header = false;
        self.follow_row(height);
    }

    /// Moves the window without moving the selection — the wheel, and the keys
    /// that shift the grid sideways.
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let last = self.last_scroll(height);
        self.scroll = (self.scroll as isize + delta).clamp(0, last as isize) as usize;
    }

    pub fn scroll_columns(&mut self, delta: isize) {
        let last = self.table.columns().saturating_sub(1);
        self.first_column =
            (self.first_column as isize + delta).clamp(0, last.max(0) as isize) as usize;
    }

    /// Keeps the selected row inside the window.
    pub fn follow_row(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        // The header is drawn above the window whatever the window holds, so
        // reaching it means scrolling to the top rather than moving to a row.
        if self.on_header {
            self.scroll = 0;
            return;
        }
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if self.row >= self.scroll + height {
            self.scroll = self.row + 1 - height;
        }
        self.scroll = self.scroll.min(self.last_scroll(height));
    }

    /// Keeps the selected column inside the window, whole.
    ///
    /// The window is moved a column at a time until the selected one fits,
    /// rather than being set from a running total: a column wider than the pane
    /// would otherwise be unreachable, and here it simply becomes the first one
    /// drawn and is clipped at the right edge like any other.
    pub fn follow_column(&mut self, width: usize) {
        if self.column < self.first_column {
            self.first_column = self.column;
            return;
        }
        while self.first_column < self.column && self.span(self.first_column, self.column) > width {
            self.first_column += 1;
        }
    }

    /// Cells of the pane that columns `from..=to` take, separators included.
    fn span(&self, from: usize, to: usize) -> usize {
        if self.table.widths.is_empty() {
            return 0;
        }
        self.table.widths[from..=to.min(self.table.widths.len() - 1)]
            .iter()
            .map(|width| width + CELL_GAP)
            .sum()
    }

    /// The furthest the window scrolls: the last row can reach the top of the
    /// pane, exactly as the last line of a diff can (ADR-053).
    fn last_scroll(&self, height: usize) -> usize {
        self.table.len().saturating_sub(height.max(1))
    }

    /// Puts the selection back on a cell that exists, after a parse that may
    /// have made the table shorter or narrower.
    fn clamp_selection(&mut self) {
        if let Some((record, column)) = self.anchor {
            let last = self.table.len().saturating_sub(1) as isize;
            self.anchor = Some((
                record.clamp(HEADER_ORDINAL, last),
                column.min(self.table.columns().saturating_sub(1)),
            ));
        }
        if self.table.rows.is_empty() {
            // Nothing to stand on but the header, which every non-empty file
            // has: a table of a file with one line is that line.
            self.on_header = !self.table.header.is_empty();
        }
        self.row = self.row.min(self.table.len().saturating_sub(1));
        self.column = self.column.min(self.table.columns().saturating_sub(1));
        self.first_column = self.first_column.min(self.column);
        self.scroll = self.scroll.min(self.row);
    }

    /// The width of the row-number gutter, which is the line-number gutter's:
    /// one column of digits reads the same whether it is counting lines or
    /// records.
    pub fn gutter(&self) -> usize {
        gutter_width(self.table.len())
    }

    /// `Row 3/128 · country`, the status bar's readout for a table.
    ///
    /// One-based, like the cursor position it stands in for, and it names the
    /// column rather than numbering it: the number is on screen already, in the
    /// header the selection is under.
    pub fn position(&self) -> String {
        let column = self.table.column_name(self.column);
        if self.on_header {
            return format!("Header · {column}");
        }
        let rows = self.table.len();
        let row = if rows == 0 { 0 } else { self.row + 1 };
        format!("Row {row}/{rows} · {column}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(text: &str) -> TableView {
        let mut view = TableView::new(Dialect::default());
        view.sync(&Document::from_text(text, None));
        view
    }

    /// A header and `rows` data rows of two columns.
    fn grid(rows: usize) -> TableView {
        let mut text = String::from("a,b\n");
        for i in 0..rows {
            text.push_str(&format!("{i},x\n"));
        }
        view(&text)
    }

    #[test]
    fn the_first_sync_parses_and_a_second_one_does_not_reparse() {
        let document = Document::from_text("a,b\n1,2\n", None);
        let mut view = TableView::new(Dialect::default());
        view.sync(&document);
        assert_eq!(view.table.header, vec!["a", "b"]);
        let parsed = view.table.clone();
        view.sync(&document);
        assert_eq!(view.table, parsed);
    }

    #[test]
    fn an_edit_is_picked_up_on_the_next_sync() {
        let mut document = Document::from_text("a,b\n1,2\n", None);
        let mut view = TableView::new(Dialect::default());
        view.sync(&document);
        document.insert_text("3,4\n");
        view.sync(&document);
        assert_eq!(view.table.len(), 2);
    }

    #[test]
    fn changing_the_dialect_reparses_and_goes_back_to_the_first_column() {
        let document = Document::from_text("a;b\n1;2\n", None);
        let mut view = TableView::new(Dialect::default());
        view.sync(&document);
        assert_eq!(view.table.columns(), 1, "commas do not split this file");
        view.column = 0;
        view.set_dialect(Dialect {
            delimiter: ';',
            quote: Some('"'),
        });
        view.sync(&document);
        assert_eq!(view.table.header, vec!["a", "b"]);
        assert_eq!(view.first_column, 0);
    }

    #[test]
    fn the_selection_stops_at_the_edges_of_the_table() {
        let mut view = grid(5);
        view.move_row(-3, 4);
        assert_eq!(view.row, 0);
        view.move_row(100, 4);
        assert_eq!(view.row, 4);
        view.move_column(-1, 40);
        assert_eq!(view.column, 0);
        view.move_column(9, 40);
        assert_eq!(view.column, 1, "two columns, so the second is the last");
    }

    #[test]
    fn the_window_follows_the_selection_down_and_back_up() {
        let mut view = grid(20);
        view.move_row(9, 5);
        assert_eq!(view.scroll, 5, "the selected row is the last of the window");
        view.move_row(-9, 5);
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn the_last_row_can_reach_the_top_of_the_window() {
        let mut view = grid(20);
        view.end(5);
        assert_eq!(view.row, 19);
        view.scroll_by(100, 5);
        assert_eq!(view.scroll, 15);
    }

    #[test]
    fn a_table_shorter_than_the_pane_does_not_scroll() {
        let mut view = grid(3);
        view.scroll_by(10, 10);
        assert_eq!(view.scroll, 0);
        view.end(10);
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn the_horizontal_window_moves_a_whole_column_at_a_time() {
        // Four columns, each clamped to the three-cell minimum, so a column and
        // its separator is four cells.
        let mut view = view("a,b,c,d\n1,2,3,4\n");
        view.move_column(3, 8);
        assert_eq!(view.column, 3);
        assert_eq!(view.first_column, 2, "two columns fit in eight cells");
        view.move_column(-3, 8);
        assert_eq!(view.first_column, 0);
    }

    #[test]
    fn a_column_wider_than_the_pane_is_still_reachable() {
        let mut view = view("a,b\n1,xxxxxxxxxxxxxxxxxxxx\n");
        view.move_column(1, 4);
        assert_eq!(view.column, 1);
        assert_eq!(view.first_column, 1, "it becomes the first and is clipped");
    }

    #[test]
    fn a_parse_that_shrank_the_table_pulls_the_selection_back() {
        let mut document = Document::from_text("a,b\n1,2\n3,4\n5,6\n", None);
        let mut view = TableView::new(Dialect::default());
        view.sync(&document);
        view.move_row(2, 2);
        assert_eq!(view.row, 2);
        document.select_all();
        document.delete_selection();
        document.insert_text("a,b\n1,2\n");
        view.sync(&document);
        assert_eq!(view.row, 0);
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn the_readout_names_the_row_and_the_column() {
        let mut view = grid(9);
        view.move_row(2, 5);
        assert_eq!(view.position(), "Row 3/9 · a");
        view.move_column(1, 40);
        assert_eq!(view.position(), "Row 3/9 · b");
    }

    #[test]
    fn the_header_sits_one_row_above_the_first_record() {
        let mut view = grid(3);
        assert!(!view.on_header);
        view.move_row(-1, 4);
        assert!(view.on_header, "up from the first row lands on the header");
        assert_eq!(view.record(), Record::Header);
        view.move_row(-1, 4);
        assert!(view.on_header, "and stops there");
        view.move_row(1, 4);
        assert_eq!((view.on_header, view.row), (false, 0));
        // Ctrl+Home is the top-left cell of the file, which is the header's.
        view.move_row(2, 4);
        view.home();
        assert!(view.on_header);
        // Ctrl+End is a record, so it leaves the header.
        view.end(4);
        assert_eq!((view.on_header, view.row), (false, 2));
    }

    #[test]
    fn the_readout_names_the_header_rather_than_numbering_it() {
        let mut view = grid(3);
        view.move_row(-1, 4);
        assert_eq!(view.position(), "Header · a");
    }

    #[test]
    fn a_cell_editor_remembers_the_cell_it_was_opened_over() {
        let mut view = grid(3);
        view.move_row(1, 4);
        view.move_column(1, 40);
        view.begin_edit(view.value().to_string().as_str());
        let editor = view.editor.as_ref().expect("a cell is open");
        assert_eq!(editor.record, Record::Row(1));
        assert_eq!(editor.column, 1);
        assert_eq!(editor.field.value, "x");
        // The caret lands at the end, where a correction is typed.
        assert_eq!(editor.field.cursor.0, 1);

        assert!(view.cancel_edit());
        assert!(!view.is_editing());
        assert!(!view.cancel_edit(), "and there is nothing left to cancel");
    }

    #[test]
    fn a_block_is_the_rectangle_between_the_anchor_and_the_cursor() {
        let mut view = grid(5);
        assert!(
            view.selection().is_none(),
            "nothing is selected to begin with"
        );

        view.anchor_here();
        view.move_row(2, 5);
        view.move_column(1, 40);
        let range = view.selection().expect("a block");
        assert_eq!((range.first_record, range.last_record), (0, 2));
        assert_eq!((range.first_column, range.last_column), (0, 1));
        assert_eq!((range.records(), range.columns()), (3, 2));
        assert!(range.is_range());
        assert!(range.contains(1, 1));
        assert!(!range.contains(3, 1));

        // Made upwards it is the same rectangle.
        view.clear_selection();
        view.anchor_here();
        view.move_row(-2, 5);
        let range = view.selection().unwrap();
        assert_eq!((range.first_record, range.last_record), (0, 2));

        view.clear_selection();
        assert!(view.selection().is_none());
    }

    #[test]
    fn a_block_of_one_cell_is_not_a_range() {
        let mut view = grid(3);
        view.anchor_here();
        let range = view.selection().unwrap();
        assert!(
            !range.is_range(),
            "the cursor cell alone is not a selection"
        );
    }

    #[test]
    fn a_row_a_column_and_everything_are_each_a_rectangle() {
        let mut view = view("a,b,c\n1,2,3\n4,5,6\n");
        view.select_record();
        let range = view.selection().unwrap();
        assert_eq!((range.records(), range.columns()), (1, 3));

        view.select_column();
        let range = view.selection().unwrap();
        assert_eq!(
            (range.first_record, range.records()),
            (HEADER_ORDINAL, 3),
            "a column runs from the header to the last record"
        );

        view.select_all();
        let range = view.selection().unwrap();
        assert_eq!((range.records(), range.columns()), (3, 3));
    }

    #[test]
    fn the_cells_of_a_block_come_out_in_reading_order() {
        let mut view = view("a,b\n1,2\n3,4\n");
        view.anchor_here();
        view.move_row(1, 5);
        view.move_column(1, 40);
        assert_eq!(
            view.selected_cells(),
            vec![
                (Record::Row(0), 0),
                (Record::Row(0), 1),
                (Record::Row(1), 0),
                (Record::Row(1), 1),
            ]
        );
    }

    #[test]
    fn a_parse_that_shrank_the_table_pulls_the_anchor_in_with_the_cursor() {
        let mut document = Document::from_text("a,b\n1,2\n3,4\n5,6\n", None);
        let mut view = TableView::new(Dialect::default());
        view.sync(&document);
        view.move_row(2, 5);
        view.anchor_here();
        view.move_row(-2, 5);
        document.select_all();
        document.delete_selection();
        document.insert_text("a,b\n1,2\n");
        view.sync(&document);
        let range = view.selection().unwrap();
        assert!(
            range.last_record <= 0,
            "the anchor cannot outlive the row it was on: {range:?}"
        );
    }

    #[test]
    fn a_file_of_one_line_leaves_the_selection_on_the_header() {
        let view = view("only,a,header\n");
        assert_eq!(view.table.len(), 0);
        assert!(view.on_header, "there is no data row to stand on");
    }

    #[test]
    fn an_empty_table_reports_no_row_and_does_not_panic() {
        let mut view = view("");
        assert_eq!(view.position(), "Row 0/0 · Column 1");
        view.move_row(1, 5);
        view.move_column(1, 5);
        view.end(0);
        assert_eq!((view.row, view.column), (0, 0));
    }
}
