//! Delimited text as a table: the parser behind the CSV view (SPEC §65).
//!
//! A CSV file is still a text file — the buffer, the undo history and the save
//! path are the document's, exactly as they are for source code. What is here
//! is a second *reading* of the same bytes: the fields a line is made of, and
//! how wide each column has to be drawn. Nothing in this module mutates
//! anything, so a table can be thrown away and parsed again whenever the
//! document changes, which is what keeps the view honest without a second copy
//! of the file to keep in step.
//!
//! The grammar is RFC 4180's, with the two liberties every real file takes: a
//! row may have more or fewer fields than the header, and a quote may open a
//! field that runs across line breaks. What it deliberately does *not* do is
//! guess — the delimiter and the quote are a `Dialect`, the status bar shows
//! which one is in use, and a file the sniffer read wrongly is two clicks from
//! being read right (ADR-062).
//!
//! Every field also remembers the `Span` of buffer it was read from, which is
//! what makes the grid writable: a cell is edited by overwriting exactly those
//! characters with the same value rendered back into the dialect, so the edit
//! reaches the document as an ordinary one and the parse is still the only
//! model of the file (ADR-063). Nothing here mutates a document — the spans and
//! the rendering are handed out, and `Document` does the writing.

use std::borrow::Cow;

use unicode_width::UnicodeWidthStr;

/// The narrowest a column is drawn, however short its contents. Below three
/// cells an ellipsis is wider than the text it stands in for.
pub const MIN_COL_WIDTH: usize = 3;

/// The widest a column is drawn before its cells are cut short.
///
/// A free-text column — a description, a URL, a JSON blob — is otherwise as
/// wide as its longest value, and one such column pushes every column after it
/// off the right edge of a terminal for the whole file. Thirty-two cells is
/// wide enough to recognise a value and narrow enough that several columns fit
/// beside it; the whole cell is still in the buffer, and the text view is one
/// keystroke away.
pub const MAX_COL_WIDTH: usize = 32;

/// The most rows a table holds.
///
/// A parse walks the whole document and keeps every field of it, so an
/// unbounded table is a second copy of a file that may be hundreds of
/// megabytes (SPEC §44). Past this the table says it was cut, the view says so
/// too, and the rest of the file is read in the text view — which streams out
/// of the rope and needs no such limit.
pub const MAX_ROWS: usize = 100_000;

/// What the parser is told about a file: what separates fields, and what quotes
/// them.
///
/// Both are questions the file itself cannot answer reliably — a semicolon file
/// with commas inside its fields looks exactly like a comma file with
/// semicolons inside its fields — so they are a value the user owns and the
/// status bar shows (ADR-058, ADR-062).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialect {
    pub delimiter: char,
    /// `None` for a file with no quoting at all, where a delimiter is a
    /// delimiter wherever it appears. Rare, but it is the only correct reading
    /// of a file whose fields contain unbalanced quotes — a text column of
    /// inches, or of apostrophes.
    pub quote: Option<char>,
}

impl Default for Dialect {
    fn default() -> Self {
        Self {
            delimiter: ',',
            quote: Some('"'),
        }
    }
}

/// The delimiters the picker offers, with the names they are shown under.
///
/// A tab has to be a menu entry rather than something typed: it cannot be
/// typed into a field that uses it to move focus, and a file separated by one
/// is the second most common kind there is.
pub const DELIMITERS: &[(char, &str)] = &[
    (',', "Comma"),
    (';', "Semicolon"),
    ('\t', "Tab"),
    ('|', "Pipe"),
    (':', "Colon"),
    (' ', "Space"),
];

/// The quote characters the picker offers. `None` is a row of its own: "no
/// quoting" is an answer, not the absence of one.
pub const QUOTES: &[(Option<char>, &str)] = &[
    (Some('"'), "Double quote"),
    (Some('\''), "Single quote"),
    (Some('`'), "Backtick"),
    (None, "None"),
];

impl Dialect {
    /// What the status bar calls the delimiter: the character itself where it
    /// is visible, and a name where it is not.
    pub fn delimiter_label(self) -> String {
        match DELIMITERS.iter().find(|(ch, _)| *ch == self.delimiter) {
            // A space and a tab are both drawn as nothing at all, so they are
            // the two that have to be named rather than shown.
            Some((' ', name)) | Some(('\t', name)) => (*name).to_string(),
            _ => self.delimiter.to_string(),
        }
    }

    pub fn quote_label(self) -> String {
        match self.quote {
            Some(quote) => quote.to_string(),
            None => "none".to_string(),
        }
    }

    /// A value written back as a field of this dialect: quoted where it has to
    /// be, and `None` where it cannot be written at all (ADR-063).
    ///
    /// Quotes are added only where leaving them out would change the reading —
    /// a value holding the delimiter, the quote character or a line break — so
    /// a file whose fields were never quoted still has none after an edit, and
    /// a diff of one edited cell is one field long.
    ///
    /// `None` is the one case with no honest answer: a file read with no
    /// quoting at all (`quote` is `None`) cannot hold a delimiter inside a
    /// field, and silently dropping the character or silently splitting the
    /// row would both be worse than refusing.
    pub fn render_field(self, value: &str) -> Option<String> {
        let needs_quotes = value.contains(self.delimiter)
            || value.contains(['\n', '\r'])
            || self.quote.is_some_and(|quote| value.contains(quote));
        if !needs_quotes {
            return Some(value.to_string());
        }
        let quote = self.quote?;
        Some(format!(
            "{quote}{}{quote}",
            value.replace(quote, &format!("{quote}{quote}"))
        ))
    }

    /// The dialect a file is opened with: the extension where it settles the
    /// question, and otherwise whichever candidate divides the first lines into
    /// the most columns, consistently.
    ///
    /// Consistency is the whole test. A delimiter that appears eleven times in
    /// one line and twice in the next is punctuation inside the fields; the one
    /// that appears the same number of times in every line is the delimiter,
    /// even when it appears only once. Ties go to the earlier entry in
    /// `DELIMITERS`, which is the order they are common in.
    pub fn sniff<S: AsRef<str>>(extension: Option<&str>, lines: impl Iterator<Item = S>) -> Self {
        // `.tsv` is not a guess: the extension *is* the declaration, and a tab
        // file whose first line happens to hold two commas must not be read as
        // a comma file.
        if extension.is_some_and(|ext| ext.eq_ignore_ascii_case("tsv")) {
            return Self {
                delimiter: '\t',
                quote: Some('"'),
            };
        }
        let sample: Vec<String> = lines
            .take(SNIFF_LINES)
            .map(|line| line.as_ref().to_string())
            .filter(|line| !line.trim().is_empty())
            .collect();
        let quote = Some('"');
        let delimiter = SNIFFED
            .iter()
            .copied()
            .filter_map(|candidate| {
                let dialect = Self {
                    delimiter: candidate,
                    quote,
                };
                score(&sample, dialect).map(|score| (candidate, score))
            })
            // `max_by_key` keeps the *last* maximum, and the candidates arrive
            // in preference order, so the comparison is deliberately strict:
            // the first candidate with a given score wins.
            .fold(
                None,
                |best: Option<(char, usize)>, (candidate, score)| match best {
                    Some((_, best_score)) if best_score >= score => best,
                    _ => Some((candidate, score)),
                },
            )
            .map(|(candidate, _)| candidate)
            .unwrap_or(',');
        Self { delimiter, quote }
    }
}

/// Lines the sniffer reads. Enough to see whether a count is stable, few enough
/// that opening a large file does not walk it.
const SNIFF_LINES: usize = 20;

/// The delimiters the sniffer will guess, in preference order.
///
/// Shorter than `DELIMITERS`, and the difference is the point: prose splits
/// evenly on spaces and timestamps split evenly on colons, so a sniffer offered
/// those two reads an ordinary text file as a wide table. Both stay in the
/// picker — a file really separated by them is a file the user can say so
/// about — but neither is ever guessed.
const SNIFFED: &[char] = &[',', ';', '\t', '|'];

/// How well `dialect` explains `sample`: the number of fields per line, when
/// every line agrees on it and there is more than one.
///
/// `None` says this candidate is not a delimiter here — it splits nothing, or
/// it splits different lines differently.
fn score(sample: &[String], dialect: Dialect) -> Option<usize> {
    let mut counts = sample.iter().map(|line| fields(line, dialect).len());
    let first = counts.next()?;
    if first < 2 {
        return None;
    }
    counts.all(|count| count == first).then_some(first)
}

/// One line split into fields, for the sniffer.
///
/// It is the row parser without the state that carries across lines: a quoted
/// field with a newline in it is not something a sniffer can see anyway, and a
/// file where the first twenty lines are one such field is a file whose
/// delimiter the user will have to choose.
fn fields(line: &str, dialect: Dialect) -> Vec<String> {
    let mut parser = Parser::new(dialect);
    parser.push_line(0, line);
    parser.finish().pop().unwrap_or_default().fields
}

/// Where a character sits in the document: a line, and a `char` offset into it.
///
/// The same coordinates `Document` edits in, so a span goes back to the buffer
/// without a conversion — which is the point of keeping them (ADR-063).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// The raw text one field was read from, quotes included: what an edit to that
/// cell overwrites.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    /// Whether the field lies on a single line, which is the only kind the grid
    /// writes back to: a value quoted across line breaks is edited in the text
    /// view, where the line breaks are visible.
    pub fn single_line(self) -> bool {
        self.start.line == self.end.line
    }
}

/// Which record of the file a cell is in.
///
/// The header is a record like any other — it is the file's first line — and it
/// is edited like any other, because renaming a column is the second thing
/// anybody does to a spreadsheet (SPEC §65).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Record {
    Header,
    Row(usize),
}

/// What an edit to one cell does to the buffer: the characters of one line to
/// overwrite, and the text to put there.
///
/// Line and columns rather than a `Span` because that is the shape `Document`
/// replaces text in, and because a plan that could not be applied — a field
/// spanning lines, a value needing quotes a dialect has not got — is a
/// `WriteError` instead of a plan (ADR-063).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Why a cell could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// The cell is a quoted field with a line break in it. Editing it inline
    /// would mean a caret in a value that is drawn as one row and stored as
    /// several, so the text view has it instead.
    Multiline,
    /// The value needs quoting and the file is being read with none.
    Unquotable,
    /// The record is not in the table — a row number past the end of it.
    NoRecord,
}

/// A parsed file: the header row, the rows under it, and how wide each column
/// has to be drawn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    /// The first row of the file. SPEC §65 makes it the header unconditionally:
    /// a file whose first row is data is a file the user can read in the text
    /// view, and a heuristic that is right most of the time is one that
    /// silently hides a row the rest of the time.
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Display width of each column, clamped to `MIN_COL_WIDTH..=MAX_COL_WIDTH`.
    pub widths: Vec<usize>,
    /// Whether each column holds numbers, and so is read from the right
    /// (SPEC §65, ADR-081). Parallel to `widths`.
    pub numeric: Vec<bool>,
    /// Set when the file had more rows than `MAX_ROWS`. Said in the view, so
    /// "where is the rest of it?" has an answer on screen.
    pub truncated: bool,
    /// Where each field was read from, parallel to `header` and to `rows`.
    ///
    /// Private, because a span is only meaningful beside the parse it came from
    /// and this is the parse: `write` is the only way in, and it is handed the
    /// value to put there so the two can never be combined wrongly.
    header_spans: Vec<Span>,
    row_spans: Vec<Vec<Span>>,
}

impl Table {
    /// Parses a whole document, one line at a time.
    ///
    /// Lines rather than a string because that is what a rope hands out
    /// cheaply, and a table of a large file must not begin by making a second
    /// copy of it (SPEC §44). The parser carries its own state across the
    /// boundary, so a quoted field containing line breaks is one field.
    pub fn parse<S: AsRef<str>>(lines: impl Iterator<Item = S>, dialect: Dialect) -> Self {
        let mut parser = Parser::new(dialect);
        for (index, line) in lines.enumerate() {
            parser.push_line(index, line.as_ref());
            if parser.rows.len() > MAX_ROWS {
                break;
            }
        }
        Self::from_records(parser.finish())
    }

    fn from_records(mut records: Vec<Raw>) -> Self {
        // A file that ends in a newline yields a final empty record, which is
        // the terminator and not a row: a table with a phantom blank line at
        // the bottom of every file would be the view disagreeing with the
        // editor about how long the file is.
        if records
            .last()
            .is_some_and(|row| row.fields == [String::new()])
        {
            records.pop();
        }
        let truncated = records.len() > MAX_ROWS;
        records.truncate(MAX_ROWS);
        let mut records = records.into_iter();
        let head = records.next().unwrap_or_default();
        let (header, header_spans) = (head.fields, head.spans);
        let (rows, row_spans): (Vec<Vec<String>>, Vec<Vec<Span>>) =
            records.map(|row| (row.fields, row.spans)).unzip();
        let columns = rows
            .iter()
            .map(Vec::len)
            .chain(std::iter::once(header.len()))
            .max()
            .unwrap_or(0);
        let widths = (0..columns)
            .map(|column| {
                let cells = rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .chain(header.get(column));
                cells
                    .map(|cell| cell.width())
                    .max()
                    .unwrap_or(0)
                    .clamp(MIN_COL_WIDTH, MAX_COL_WIDTH)
            })
            .collect();
        let numeric = (0..columns)
            .map(|column| {
                // The header is left out: it is a name, and a column called
                // `2024` is not what makes the values under it numbers.
                Self::is_numeric_column(rows.iter().filter_map(|row| row.get(column)))
            })
            .collect();
        Self {
            header,
            rows,
            widths,
            numeric,
            truncated,
            header_spans,
            row_spans,
        }
    }

    /// Whether a column's values are quantities, and so line up on their last
    /// digit rather than on their first character (ADR-081).
    ///
    /// Deliberately unanimous and deliberately narrow. A column is numeric only
    /// when *every* value in it that is not blank is a plain number, because
    /// the alignment is a claim about the column and one string in it that is
    /// not a quantity is the counter-example. Blanks are ignored — a missing
    /// measurement is not evidence either way — and a column with no values at
    /// all is not numeric, because there is nothing to have found out.
    pub fn is_numeric_column<'a>(values: impl Iterator<Item = &'a String>) -> bool {
        let mut seen = false;
        for value in values {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if !is_number(value) {
                return false;
            }
            seen = true;
        }
        seen
    }

    pub fn columns(&self) -> usize {
        self.widths.len()
    }

    /// Whether this column is read from the right.
    pub fn is_numeric(&self, column: usize) -> bool {
        self.numeric.get(column).copied().unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.header.is_empty()
    }

    /// The header of a column, or its one-based number when the header row is
    /// short of it.
    ///
    /// A ragged file is the normal case, not the broken one, and a nameless
    /// column still has to be pointed at in the status bar.
    pub fn column_name(&self, column: usize) -> Cow<'_, str> {
        match self.header.get(column).filter(|name| !name.is_empty()) {
            Some(name) => Cow::Borrowed(name),
            None => Cow::Owned(format!("Column {}", column + 1)),
        }
    }

    /// One cell, empty for a row that stops before this column.
    pub fn cell(&self, row: usize, column: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|row| row.get(column))
            .map_or("", String::as_str)
    }

    /// One cell of any record, the header included.
    pub fn field(&self, record: Record, column: usize) -> &str {
        match record {
            Record::Header => self.header.get(column).map_or("", String::as_str),
            Record::Row(row) => self.cell(row, column),
        }
    }

    /// Whether a record exists — a row number the table is long enough for.
    pub fn has(&self, record: Record) -> bool {
        match record {
            Record::Header => !self.header_spans.is_empty(),
            Record::Row(row) => row < self.rows.len(),
        }
    }

    /// The first and last line of the buffer a record occupies.
    ///
    /// The same line twice for the ordinary record; two different ones for a
    /// record with a quoted line break in it, which is why inserting a row
    /// after this one is measured from the *last* of them.
    pub fn lines(&self, record: Record) -> Option<(usize, usize)> {
        let spans = self.spans(record)?;
        Some((spans.first()?.start.line, spans.last()?.end.line))
    }

    /// What writing `value` into one cell does to the buffer (ADR-063).
    ///
    /// The plan is the field's own characters and nothing else: the rest of the
    /// record keeps the bytes it had, so an edit to one cell cannot re-quote a
    /// neighbour that was written by hand. A column the record stops short of
    /// is reached by adding the delimiters it is missing, because a ragged file
    /// is the normal kind and refusing to fill one in would be the grid saying
    /// no to the edit that fixes it.
    pub fn write(
        &self,
        dialect: Dialect,
        record: Record,
        column: usize,
        value: &str,
    ) -> Result<Write, WriteError> {
        let spans = self.spans(record).ok_or(WriteError::NoRecord)?;
        let last = *spans.last().ok_or(WriteError::NoRecord)?;
        let text = dialect.render_field(value).ok_or(WriteError::Unquotable)?;
        match spans.get(column) {
            Some(span) if span.single_line() => Ok(Write {
                line: span.start.line,
                start: span.start.col,
                end: span.end.col,
                text,
            }),
            Some(_) => Err(WriteError::Multiline),
            None => {
                let missing = column - spans.len() + 1;
                Ok(Write {
                    line: last.end.line,
                    start: last.end.col,
                    end: last.end.col,
                    text: format!("{}{text}", dialect.delimiter.to_string().repeat(missing)),
                })
            }
        }
    }

    /// An empty record of this table's width: the delimiters and nothing
    /// between them, which is what a new row is made of.
    pub fn blank_record(&self, dialect: Dialect) -> String {
        dialect
            .delimiter
            .to_string()
            .repeat(self.columns().saturating_sub(1))
    }

    /// Where one cell's raw text sits, for a caller that edits several at once
    /// and has to send them to the buffer in one step.
    pub fn span(&self, record: Record, column: usize) -> Option<Span> {
        self.spans(record)?.get(column).copied()
    }

    fn spans(&self, record: Record) -> Option<&[Span]> {
        let spans = match record {
            Record::Header => &self.header_spans,
            Record::Row(row) => self.row_spans.get(row)?,
        };
        (!spans.is_empty()).then_some(spans.as_slice())
    }
}

/// One record as the parser built it: the values, and where each was read from.
#[derive(Debug, Clone, Default)]
struct Raw {
    fields: Vec<String>,
    spans: Vec<Span>,
}

/// The state machine, kept between lines so a quoted newline is a character in
/// a field rather than the end of a row.
struct Parser {
    dialect: Dialect,
    rows: Vec<Raw>,
    row: Raw,
    field: String,
    /// Where the field being built began, quotes included.
    field_start: Pos,
    /// Where the character being read sits. A field ends *at* the delimiter, so
    /// this is the end of the field the delimiter closes.
    at: Pos,
    /// Just past the last character of the line read last, which is where a
    /// record ends when a line break rather than a delimiter closes it.
    line_end: Pos,
    /// Inside a quoted field: a delimiter and a line break are both literal
    /// text until the closing quote.
    quoted: bool,
    /// The previous character was the closing quote of a quoted field, so a
    /// second quote here is an escaped one (RFC 4180's `""`) rather than the
    /// start of a new one.
    closing: bool,
    /// Whether the next line is the first. A row ends where the *previous* line
    /// did, so the boundary is drawn before a line rather than after it — which
    /// is what lets `finish` close the last row whether or not the file ended
    /// in a newline.
    first: bool,
}

impl Parser {
    fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            rows: Vec::new(),
            row: Raw::default(),
            field: String::new(),
            field_start: Pos::default(),
            at: Pos::default(),
            line_end: Pos::default(),
            quoted: false,
            closing: false,
            first: true,
        }
    }

    /// Feeds one line, without its terminator, at its number in the document.
    ///
    /// The number is what makes the spans usable: a cell knows the line it was
    /// read from, so writing to it is an ordinary edit at ordinary coordinates
    /// (ADR-063).
    fn push_line(&mut self, index: usize, line: &str) {
        // A quote that closed at the end of a line closed the field with it:
        // the line break after it is the row's, not the field's.
        if self.quoted && self.closing {
            self.quoted = false;
            self.closing = false;
        }
        // A line break inside a quoted field is part of the field. The rope
        // holds `\n` alone whatever the file was written with (see
        // `Document`), so that is what goes back in.
        if self.quoted {
            self.field.push('\n');
        } else {
            if !self.first {
                self.end_row(self.line_end);
            }
            // The first field of the record begins where the line does.
            self.field_start = Pos::new(index, 0);
        }
        self.first = false;
        let mut length = 0;
        for (col, ch) in line.chars().enumerate() {
            self.at = Pos::new(index, col);
            self.push_char(ch);
            length = col + 1;
        }
        self.line_end = Pos::new(index, length);
    }

    fn push_char(&mut self, ch: char) {
        let quote = self.dialect.quote;
        if self.quoted {
            match (quote, self.closing) {
                // `""` inside a quoted field is one literal quote.
                (Some(q), true) if ch == q => {
                    self.field.push(q);
                    self.closing = false;
                }
                (Some(q), false) if ch == q => self.closing = true,
                // The quote closed the field; whatever follows is read
                // unquoted, which is how a stray `"` mid-field stays readable.
                (_, true) => {
                    self.quoted = false;
                    self.closing = false;
                    self.push_char(ch);
                }
                _ => self.field.push(ch),
            }
            return;
        }
        if ch == self.dialect.delimiter {
            self.end_field(self.at);
            // The next field starts after the delimiter that ended this one.
            self.field_start = Pos::new(self.at.line, self.at.col + 1);
        } else if quote == Some(ch) && self.field.is_empty() {
            // Only at the start of a field: a quote in the middle of one is a
            // character, as it is in `12" pipe`.
            self.quoted = true;
        } else {
            self.field.push(ch);
        }
    }

    fn end_field(&mut self, end: Pos) {
        self.row.fields.push(std::mem::take(&mut self.field));
        self.row.spans.push(Span {
            start: self.field_start,
            end,
        });
        self.quoted = false;
        self.closing = false;
    }

    fn end_row(&mut self, end: Pos) {
        self.end_field(end);
        self.rows.push(std::mem::take(&mut self.row));
    }

    fn finish(mut self) -> Vec<Raw> {
        self.end_row(self.line_end);
        self.rows
    }
}

/// Whether one value is a plain number.
///
/// Conservative on purpose (ADR-081). A version, a date, an identifier and a
/// zip code are all *made of digits* and none of them is a quantity: right-
/// aligning a column of them would line up the wrong end of a string that is
/// read from the left. So the grammar here is a sign, digits, and at most one
/// decimal point — and everything that is more than that is a string:
///
/// - `1.2.3` — two points, a version.
/// - `2026-09-09` — a `-` that is not the leading sign, a date.
/// - `12:30`, `1/2`, `1e9`, `10%`, `$4`, `1,234` — anything with a character
///   that is not a digit, a point or a leading sign in it.
/// - `007`, `01234` — a leading zero in front of more digits, which is how
///   part numbers, zip codes and phone extensions are written. `0` and `0.5`
///   are numbers; `0.` and `.5` are not, because a value that has to be
///   guessed at is a value to leave alone.
///
/// The cost of being wrong in the other direction is small: a column of
/// quantities the check refuses is drawn exactly as it was before this
/// existed.
fn is_number(value: &str) -> bool {
    let digits = value.strip_prefix(['-', '+']).unwrap_or(value);
    let mut parts = digits.split('.');
    let (Some(whole), fraction, None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(whole) {
        return false;
    }
    if whole.len() > 1 && whole.starts_with('0') {
        return false;
    }
    match fraction {
        Some(fraction) => all_digits(fraction),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str, dialect: Dialect) -> Table {
        Table::parse(text.split('\n'), dialect)
    }

    fn comma(text: &str) -> Table {
        table(text, Dialect::default())
    }

    #[test]
    fn the_first_row_is_the_header_and_the_rest_are_rows() {
        let table = comma("name,age\nada,36\ngrace,45\n");
        assert_eq!(table.header, vec!["name", "age"]);
        assert_eq!(table.rows, vec![vec!["ada", "36"], vec!["grace", "45"]]);
        assert_eq!(table.columns(), 2);
    }

    #[test]
    fn a_trailing_newline_is_a_terminator_and_not_an_empty_row() {
        assert_eq!(comma("a,b\n1,2\n").len(), 1);
        // Without one the last row is still a row.
        assert_eq!(comma("a,b\n1,2").len(), 1);
        // Two of them are: the second is a blank line in the file.
        assert_eq!(comma("a,b\n1,2\n\n").len(), 2);
    }

    #[test]
    fn a_quoted_field_keeps_its_delimiters() {
        let table = comma("a,b\n\"one,two\",three\n");
        assert_eq!(table.rows, vec![vec!["one,two", "three"]]);
    }

    #[test]
    fn a_doubled_quote_inside_a_quoted_field_is_one_quote() {
        let table = comma("a\n\"she said \"\"hi\"\"\"\n");
        assert_eq!(table.rows, vec![vec![r#"she said "hi""#]]);
    }

    #[test]
    fn a_quoted_field_can_span_lines() {
        let table = comma("a,b\n\"first\nsecond\",x\ntail,y\n");
        assert_eq!(
            table.rows,
            vec![vec!["first\nsecond", "x"], vec!["tail", "y"]]
        );
    }

    #[test]
    fn a_quote_in_the_middle_of_a_field_is_a_character() {
        let table = comma("size\n12\" pipe\n");
        assert_eq!(table.rows, vec![vec!["12\" pipe"]]);
    }

    #[test]
    fn a_ragged_row_is_kept_as_it_is_and_widens_the_table() {
        let table = comma("a,b\n1\n2,3,4\n");
        assert_eq!(table.columns(), 3, "the widest row decides");
        assert_eq!(
            table.cell(0, 1),
            "",
            "a short row reads as empty, not out of bounds"
        );
        assert_eq!(table.cell(1, 2), "4");
        assert_eq!(table.cell(99, 0), "");
    }

    #[test]
    fn quoting_can_be_turned_off_entirely() {
        let dialect = Dialect {
            delimiter: ',',
            quote: None,
        };
        let table = table("a,b\n\"x,y\n", dialect);
        assert_eq!(table.rows, vec![vec!["\"x", "y"]]);
    }

    #[test]
    fn columns_are_at_least_the_minimum_and_at_most_the_maximum_wide() {
        let long = "x".repeat(200);
        let table = comma(&format!("a,b\n1,{long}\n"));
        assert_eq!(table.widths, vec![MIN_COL_WIDTH, MAX_COL_WIDTH]);
    }

    #[test]
    fn a_column_is_as_wide_as_its_widest_cell_measured_in_cells() {
        // Four Cyrillic characters are four cells; the header is two.
        let table = comma("ім,b\nадам,2\n");
        assert_eq!(table.widths[0], 4);
    }

    #[test]
    fn a_nameless_column_is_named_by_its_number() {
        let table = comma("a,\n1,2,3\n");
        assert_eq!(table.column_name(0), "a");
        assert_eq!(table.column_name(1), "Column 2");
        assert_eq!(table.column_name(2), "Column 3");
    }

    #[test]
    fn an_empty_document_is_an_empty_table() {
        let table = comma("");
        assert!(table.is_empty());
        assert_eq!(table.columns(), 0);
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn the_sniffer_prefers_the_delimiter_that_splits_every_line_the_same_way() {
        let semicolons = "a;b;c\n1;2;3\n4;5;6";
        assert_eq!(
            Dialect::sniff(Some("csv"), semicolons.split('\n')).delimiter,
            ';'
        );
        let commas = "a,b\n1,2";
        assert_eq!(
            Dialect::sniff(Some("csv"), commas.split('\n')).delimiter,
            ','
        );
    }

    #[test]
    fn a_comma_inside_a_semicolon_file_does_not_win() {
        // Every line has one semicolon and a different number of commas.
        let text = "name;note\nada;a, b, c\ngrace;d";
        assert_eq!(Dialect::sniff(Some("csv"), text.split('\n')).delimiter, ';');
    }

    #[test]
    fn the_tsv_extension_settles_the_question_on_its_own() {
        let text = "a,b,c\n1,2,3";
        assert_eq!(
            Dialect::sniff(Some("TSV"), text.split('\n')).delimiter,
            '\t'
        );
    }

    #[test]
    fn a_file_with_no_delimiter_at_all_falls_back_to_the_comma() {
        let text = "just one column\nand another line";
        assert_eq!(Dialect::sniff(Some("csv"), text.split('\n')).delimiter, ',');
    }

    #[test]
    fn the_labels_name_what_cannot_be_seen_and_show_what_can() {
        let comma = Dialect::default();
        assert_eq!(comma.delimiter_label(), ",");
        assert_eq!(comma.quote_label(), "\"");
        let tabbed = Dialect {
            delimiter: '\t',
            quote: None,
        };
        assert_eq!(tabbed.delimiter_label(), "Tab");
        assert_eq!(tabbed.quote_label(), "none");
    }

    // --- the spans, and writing through them (ADR-063) ---------------------

    #[test]
    fn every_field_knows_the_characters_it_was_read_from() {
        let table = comma("name,age\nada,36\n");
        let span = table.span(Record::Row(0), 1).expect("the cell exists");
        assert_eq!(span.start, Pos { line: 1, col: 4 });
        assert_eq!(span.end, Pos { line: 1, col: 6 });
        assert!(span.single_line());
        // Quotes are part of the field's raw text: they are what an edit to it
        // replaces, and what it may have to write back.
        let quoted = comma("a\n\"x,y\"\n");
        let span = quoted.span(Record::Row(0), 0).unwrap();
        assert_eq!((span.start.col, span.end.col), (0, 5));
    }

    #[test]
    fn a_field_carried_across_a_line_break_spans_both_lines() {
        let table = comma("a,b\n\"one\ntwo\",x\n");
        let span = table.span(Record::Row(0), 0).unwrap();
        assert_eq!(span.start.line, 1);
        assert_eq!(span.end.line, 2);
        assert!(!span.single_line());
        assert_eq!(table.lines(Record::Row(0)), Some((1, 2)));
    }

    #[test]
    fn a_value_is_quoted_on_the_way_back_only_where_it_has_to_be() {
        let comma = Dialect::default();
        assert_eq!(comma.render_field("ada").unwrap(), "ada");
        assert_eq!(comma.render_field("").unwrap(), "");
        assert_eq!(comma.render_field("a,b").unwrap(), "\"a,b\"");
        assert_eq!(comma.render_field("one\ntwo").unwrap(), "\"one\ntwo\"");
        assert_eq!(comma.render_field("12\" pipe").unwrap(), "\"12\"\" pipe\"");
        // With no quote character there is nowhere to put a delimiter.
        let bare = Dialect {
            delimiter: ',',
            quote: None,
        };
        assert_eq!(bare.render_field("plain").unwrap(), "plain");
        assert_eq!(bare.render_field("a,b"), None);
    }

    #[test]
    fn writing_a_cell_replaces_that_field_and_nothing_else() {
        let table = comma("name,age\nada,36\n");
        let write = table
            .write(Dialect::default(), Record::Row(0), 0, "grace")
            .unwrap();
        assert_eq!(
            write,
            Write {
                line: 1,
                start: 0,
                end: 3,
                text: "grace".to_string(),
            }
        );
        // The header is a record like any other.
        let write = table
            .write(Dialect::default(), Record::Header, 1, "years")
            .unwrap();
        assert_eq!((write.line, write.start, write.end), (0, 5, 8));
    }

    #[test]
    fn writing_past_the_end_of_a_short_row_adds_the_delimiters_it_is_missing() {
        let table = comma("a,b,c\n1\n");
        let write = table
            .write(Dialect::default(), Record::Row(0), 2, "z")
            .unwrap();
        assert_eq!(write.start, write.end, "nothing is overwritten");
        assert_eq!(write.text, ",,z");
    }

    #[test]
    fn a_write_that_cannot_be_made_says_which_kind_it_is() {
        let across = comma("a,b\n\"one\ntwo\",x\n");
        assert_eq!(
            across.write(Dialect::default(), Record::Row(0), 0, "x"),
            Err(WriteError::Multiline)
        );
        let bare = Dialect {
            delimiter: ',',
            quote: None,
        };
        let table = table("a,b\n1,2\n", bare);
        assert_eq!(
            table.write(bare, Record::Row(0), 0, "x,y"),
            Err(WriteError::Unquotable)
        );
        assert_eq!(
            table.write(bare, Record::Row(9), 0, "x"),
            Err(WriteError::NoRecord)
        );
    }

    #[test]
    fn a_blank_record_is_as_many_empty_fields_as_the_table_is_wide() {
        assert_eq!(
            comma("a,b,c\n1,2,3\n").blank_record(Dialect::default()),
            ",,"
        );
        assert_eq!(comma("a\n1\n").blank_record(Dialect::default()), "");
    }

    #[test]
    fn a_file_longer_than_the_limit_is_cut_and_says_so() {
        let text: String = (0..MAX_ROWS + 10).map(|i| format!("{i},x\n")).collect();
        let table = comma(&text);
        assert!(table.truncated);
        assert_eq!(table.len(), MAX_ROWS - 1, "the header is one of them");
    }
    /// A quantity is a sign, digits and at most one decimal point. Anything
    /// else made of digits is a string that happens to look like one
    /// (ADR-081).
    #[test]
    fn only_plain_numbers_are_numbers() {
        for value in ["0", "7", "-3", "+3", "1234", "0.5", "-0.25", "12.0"] {
            assert!(is_number(value), "{value:?} is a number");
        }
        for value in [
            // A version, a date, a time, a fraction, an exponent.
            "1.2.3",
            "2026-09-09",
            "12:30",
            "1/2",
            "1e9", // A unit, a currency, a grouped number.
            "10%",
            "$4",
            "1,234",
            "4 kg", // Identifiers written with a leading zero.
            "007",
            "01234", // Half-written numbers nobody should guess at.
            "0.",
            ".5",
            "-",
            "",
            "1.2.",
            "--1",
        ] {
            assert!(!is_number(value), "{value:?} is not a number");
        }
    }

    /// The claim is about the column, so one value that is not a quantity is
    /// enough to settle it — and blanks settle nothing either way.
    #[test]
    fn a_column_is_numeric_only_when_every_value_in_it_is() {
        let column = |values: &[&str]| {
            let owned: Vec<String> = values.iter().map(|v| (*v).to_string()).collect();
            Table::is_numeric_column(owned.iter())
        };
        assert!(column(&["1", "22", "333"]));
        assert!(column(&["1", "", "333"]), "a blank is not evidence");
        assert!(!column(&["1", "n/a", "333"]));
        assert!(!column(&["1", "2", "1.2.3"]));
        assert!(!column(&[]), "an empty column has found nothing out");
        assert!(!column(&["", ""]), "and neither has a blank one");
    }

    /// The header is a name and not a value: a column of counts headed
    /// `2024` is still counts, and a column of names headed `id` is still
    /// names.
    #[test]
    fn a_table_reads_its_columns_and_not_its_header() {
        let table = Table::parse(
            [
                "version,downloads,released",
                "0.1.5,318,2026-09-09",
                "0.1.4,96,2026-09-08",
            ]
            .into_iter(),
            Dialect::default(),
        );
        assert!(!table.is_numeric(0), "a version is not a number");
        assert!(table.is_numeric(1), "a download count is");
        assert!(!table.is_numeric(2), "a date is not");
        assert!(
            !table.is_numeric(9),
            "and neither is a column that is not there"
        );
    }
}
