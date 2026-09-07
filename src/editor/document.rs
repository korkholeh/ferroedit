//! Rope-backed document, line-ending detection, load/save.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use ropey::{Rope, RopeSlice};
use thiserror::Error;

use crate::editor::coords::{self, ByteIdx, CharIdx, GraphemeIdx, VisualCol, DEFAULT_TAB_WIDTH};
use crate::editor::cursor::{Cursor, Motion};
use crate::editor::history::{EditOperation, History};
use crate::editor::search::{self, Match};
use crate::editor::selection::{Position, Selection};

/// What a file's lines are separated by on disk.
///
/// The rope always holds `\n` alone, whatever the file used: keeping `\r` in the
/// buffer would put a character on every line that the cursor can land on but
/// the user cannot see. The original ending is remembered here and written back
/// on save, which is the whole of SPEC §17's "do not spoil line endings".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
        }
    }

    /// Decided by the *first* terminator in the file. A mixed file is written
    /// back with whichever it opened with, rather than being silently
    /// normalised — rewriting untouched lines would swamp any real diff.
    fn detect(text: &str) -> Self {
        match text.find('\n') {
            Some(at) if text[..at].ends_with('\r') => Self::Crlf,
            _ => Self::Lf,
        }
    }
}

/// Everything that can go wrong between the user and a file, phrased the way it
/// is shown on the status bar (SPEC §45: no `unwrap` in runtime paths).
#[derive(Debug, Error)]
pub enum DocumentError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{path}: not valid UTF-8")]
    NotUtf8 { path: String },
    #[error("no file name")]
    NoPath,
}

impl DocumentError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

/// One open file: the text, where it came from, where the caret is in it, and
/// what is selected.
///
/// Every mutation goes through a single insert or remove on the rope, and each
/// of those is recorded on `history` as one `EditOperation`, so undo never
/// needs a snapshot of the buffer (SPEC §16).
///
/// Only the *anchor* is stored: the selection's head is the cursor itself, so
/// the two can never drift out of step, and a selection is simply an anchor
/// that has been left behind somewhere else.
#[derive(Debug)]
pub struct Document {
    path: Option<PathBuf>,
    rope: Rope,
    line_ending: LineEnding,
    cursor: Cursor,
    anchor: Option<Position>,
    dirty: bool,
    tab_width: usize,
    history: History,
    /// Bumped by every change to the buffer.
    ///
    /// The highlight cache learns about edits from `dirty_from` below, which it
    /// consumes; the search results need the same news without consuming it, so
    /// they compare a number instead. Two consumers, two mechanisms, and
    /// neither can starve the other.
    revision: u64,
    /// Lowest line whose *text* may have changed since the highlight cache
    /// last looked, or `usize::MAX` when nothing has.
    ///
    /// The parser's state at line N depends on every line before it, so an edit
    /// anywhere invalidates the cache from that line down — and the cache has
    /// no other way to learn about an edit, because it sees the document only
    /// once a frame and by then the rope looks the same either way.
    dirty_from: usize,
}

impl Document {
    /// Builds a document from text already in memory.
    pub fn from_text(text: &str, path: Option<PathBuf>) -> Self {
        let line_ending = LineEnding::detect(text);
        let rope = match line_ending {
            LineEnding::Lf => Rope::from_str(text),
            LineEnding::Crlf => Rope::from_str(&text.replace("\r\n", "\n")),
        };
        Self {
            path,
            rope,
            line_ending,
            cursor: Cursor::default(),
            anchor: None,
            dirty: false,
            tab_width: DEFAULT_TAB_WIDTH,
            history: History::new(),
            revision: 0,
            // A document nothing has highlighted yet is stale from its first
            // line, which is also what makes a freshly opened file get parsed.
            dirty_from: 0,
        }
    }

    /// Reads a file as UTF-8 (SPEC §17: UTF-8 is the only encoding in the MVP).
    ///
    /// Reading the bytes and converting explicitly, rather than
    /// `fs::read_to_string`, is what lets a binary or Latin-1 file be reported
    /// as such instead of arriving as a lossy mess the user might then save.
    pub fn open(path: &Path) -> Result<Self, DocumentError> {
        let bytes = std::fs::read(path).map_err(|err| DocumentError::io(path, err))?;
        let text = String::from_utf8(bytes).map_err(|_| DocumentError::NotUtf8 {
            path: path.display().to_string(),
        })?;
        let document = Self::from_text(&text, Some(path.to_path_buf()));
        log::info!(
            "opened {} ({} bytes, {} lines, {})",
            path.display(),
            document.len_bytes(),
            document.line_count(),
            document.line_ending.label()
        );
        Ok(document)
    }

    /// Opens `path`, or starts an empty buffer for it when nothing is there yet.
    ///
    /// Naming a file that does not exist is how a file gets created: the buffer
    /// is empty and clean, and nothing is written until the user saves, so a
    /// mistyped name costs nothing. Every other failure — a directory, a
    /// permission error, invalid UTF-8 — is still reported rather than
    /// swallowed into a blank screen.
    pub fn open_or_create(path: &Path) -> Result<Self, DocumentError> {
        match Self::open(path) {
            Err(DocumentError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                log::info!("new file {}", path.display());
                Ok(Self::from_text("", Some(path.to_path_buf())))
            }
            other => other,
        }
    }

    /// Writes the buffer back with the line ending it was opened with.
    pub fn save(&mut self) -> Result<(), DocumentError> {
        let path = self.path.clone().ok_or(DocumentError::NoPath)?;
        let file = File::create(&path).map_err(|err| DocumentError::io(&path, err))?;
        let mut writer = BufWriter::new(file);
        self.write_to(&mut writer)
            .and_then(|()| writer.flush())
            .map_err(|err| DocumentError::io(&path, err))?;
        self.dirty = false;
        self.history.mark_saved();
        log::info!("saved {} ({} bytes)", path.display(), self.len_bytes());
        Ok(())
    }

    /// Streams the rope out chunk by chunk — never `to_string()`, which would
    /// be an allocation the size of the file on every save (SPEC §44).
    fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        if self.line_ending == LineEnding::Lf {
            for chunk in self.rope.chunks() {
                writer.write_all(chunk.as_bytes())?;
            }
            return Ok(());
        }
        for line in self.rope.lines() {
            let (text, terminated) = split_terminator(line);
            for chunk in text.chunks() {
                writer.write_all(chunk.as_bytes())?;
            }
            if terminated {
                writer.write_all(self.line_ending.as_str().as_bytes())?;
            }
        }
        Ok(())
    }

    // --- properties --------------------------------------------------------

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Points the document at a different file, which is what a rename in the
    /// explorer does to an open tab (SPEC §20).
    ///
    /// Only the path changes: the buffer, the history and the save point are
    /// the same work, and a rename is not an edit.
    pub fn set_path(&mut self, path: PathBuf) {
        log::info!(
            "{} is now {}",
            self.path
                .as_ref()
                .map_or("an unnamed buffer".into(), |p| p.display().to_string()),
            path.display()
        );
        self.path = Some(path);
    }

    /// Tab label and status-bar name.
    pub fn title(&self) -> &str {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The lowest line an edit has touched since this was last called, and
    /// resets the watermark.
    ///
    /// It is a *take* rather than a read because the highlight cache is the
    /// single consumer: two callers would each clear the mark for the other,
    /// and the compiler cannot say so, which is why this is the only method on
    /// `Document` that needs `&mut self` without changing the buffer.
    pub fn take_dirty_from(&mut self) -> Option<usize> {
        (self.dirty_from != usize::MAX).then(|| std::mem::replace(&mut self.dirty_from, usize::MAX))
    }

    /// Marks the line containing `char_idx` — and everything after it — stale
    /// for the highlight cache. Must be called *before* the rope changes,
    /// while the index still means what it meant.
    fn touch(&mut self, char_idx: usize) {
        let line = self.rope.char_to_line(char_idx.min(self.rope.len_chars()));
        self.dirty_from = self.dirty_from.min(line);
        self.revision += 1;
    }

    /// Counts every change to the buffer. Anything caching a derived view of
    /// the text compares this to know whether its copy is still true.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn tab_width(&self) -> usize {
        self.tab_width
    }

    pub fn len_bytes(&self) -> ByteIdx {
        ByteIdx(self.rope.len_bytes())
    }

    /// Number of lines. A file ending in a newline has an empty last line, the
    /// same one the cursor can be moved to and typed on.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// One line, without its terminator.
    ///
    /// Borrows straight out of the rope whenever the line lives in a single
    /// chunk, which is the overwhelmingly common case; only a line straddling a
    /// chunk boundary allocates.
    pub fn line(&self, index: usize) -> Cow<'_, str> {
        if index >= self.line_count() {
            return Cow::Borrowed("");
        }
        let (text, _) = split_terminator(self.rope.line(index));
        match text.as_str() {
            Some(text) => Cow::Borrowed(text),
            None => Cow::Owned(text.to_string()),
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The cursor's display column — where the caret is drawn.
    pub fn cursor_visual_col(&self) -> VisualCol {
        coords::visual_col(&self.cursor_line(), self.cursor.column, self.tab_width)
    }

    /// The cursor's column as a human counts it: user-perceived characters,
    /// one-based, which is what the status bar shows (SPEC §38).
    pub fn cursor_display_col(&self) -> GraphemeIdx {
        let index = coords::grapheme_index(&self.cursor_line(), self.cursor.column);
        GraphemeIdx(index.0 + 1)
    }

    fn cursor_line(&self) -> Cow<'_, str> {
        self.line(self.cursor.line)
    }

    /// Char offset of the cursor in the whole rope.
    fn cursor_char(&self) -> usize {
        self.char_of(self.cursor_position())
    }

    /// Char offset of a document position in the whole rope.
    fn char_of(&self, at: Position) -> usize {
        let line = at.line.min(self.line_count().saturating_sub(1));
        let start = self.rope.line_to_char(line);
        let len = coords::char_len(&self.line(line));
        start + at.column.0.min(len.0)
    }

    pub fn cursor_position(&self) -> Position {
        Position::new(self.cursor.line, self.cursor.column)
    }

    // --- selection ---------------------------------------------------------

    /// What is selected, or `None` when the anchor is where the cursor is.
    ///
    /// Built rather than stored: the head is always the live cursor, so no
    /// movement can leave a selection describing a position the caret has
    /// already left.
    pub fn selection(&self) -> Option<Selection> {
        let anchor = self.anchor?;
        let selection = Selection::new(anchor, self.cursor_position());
        (!selection.is_empty()).then_some(selection)
    }

    /// How many `char`s are selected, without materialising the text.
    ///
    /// The status bar asks this every frame, so it must stay O(1)-ish: `Ctrl+A`
    /// on a 5 MB file must not copy the file into a counter (SPEC §44).
    pub fn selected_len(&self) -> usize {
        match self.selection() {
            Some(selection) => self.char_of(selection.end()) - self.char_of(selection.start()),
            None => 0,
        }
    }

    /// Drops the anchor without moving the caret — what every plain movement,
    /// and every edit, does to a selection.
    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Leaves an anchor at the caret if there is not one already, so that the
    /// motion which follows extends a selection rather than starting a new one.
    fn anchor_here(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor_position());
        }
    }

    /// The selected text, newlines included, or `None` when nothing is
    /// selected. Slices the rope; no `to_string()` of the buffer (SPEC §44).
    pub fn selected_text(&self) -> Option<String> {
        let selection = self.selection()?;
        let start = self.char_of(selection.start());
        let end = self.char_of(selection.end());
        Some(self.rope.slice(start..end).to_string())
    }

    /// Everything from the start of the document to its end.
    pub fn select_all(&mut self) {
        self.history.seal();
        let last_line = self.line_count().saturating_sub(1);
        self.anchor = Some(Position::default());
        self.cursor.line = last_line;
        self.cursor.column = coords::char_len(&self.line(last_line));
        self.remember_column();
    }

    /// Selects the word under a display position — the double-click.
    ///
    /// The clicked cell is resolved to a cluster first, so clicking the right
    /// half of a wide character selects the word that character belongs to.
    pub fn select_word_at(&mut self, line: usize, col: VisualCol) {
        self.history.seal();
        let line = line.min(self.line_count().saturating_sub(1));
        let text = self.line(line).into_owned();
        let at = coords::char_at_visual_col(&text, col, self.tab_width);
        let (start, end) = coords::word_bounds(&text, at);
        self.anchor = Some(Position::new(line, start));
        self.cursor.line = line;
        self.cursor.column = end;
        self.remember_column();
    }

    /// Applies a motion with the anchor left where it is — Shift+navigation.
    pub fn extend_cursor(&mut self, motion: Motion, page: usize) {
        self.anchor_here();
        self.step(motion, page);
    }

    /// Extends the selection to a display position — the mouse drag.
    pub fn extend_to(&mut self, line: usize, col: VisualCol) {
        self.anchor_here();
        self.set_cursor_at(line, col);
    }

    /// Removes the selection and leaves the caret where it started.
    ///
    /// Returns whether anything was removed, so a caller can tell "the
    /// selection went" from "there was nothing to cut".
    pub fn delete_selection(&mut self) -> bool {
        self.history.begin_edit(self.cursor);
        let removed = self.delete_selection_inner();
        self.history.end_edit(self.cursor);
        removed
    }

    fn delete_selection_inner(&mut self) -> bool {
        let Some(selection) = self.selection() else {
            return false;
        };
        let start = selection.start();
        let range = self.char_of(start)..self.char_of(selection.end());
        self.remove(range);
        self.cursor.line = start.line;
        self.cursor.column = start.column;
        self.clear_selection();
        self.remember_column();
        true
    }

    // --- movement ----------------------------------------------------------

    /// Applies a motion. `page` is the height of the editor viewport, which is
    /// why it is passed in rather than known here: `editor/` has no idea a
    /// terminal exists.
    pub fn move_cursor(&mut self, motion: Motion, page: usize) {
        // Moving without Shift is how a selection is dismissed, which is why
        // every plain motion goes through here and every extending one does not.
        self.clear_selection();
        self.step(motion, page);
    }

    /// The motion itself, with no opinion about the anchor.
    fn step(&mut self, motion: Motion, page: usize) {
        // Moving the caret ends the open undo step (ARCHITECTURE §5): coming
        // back and typing somewhere else is a new edit, not a longer one.
        self.history.seal();
        let last_line = self.line_count().saturating_sub(1);
        match motion {
            Motion::Left => self.step_left(),
            Motion::Right => self.step_right(),
            Motion::Up => self.step_vertical(-1, 1),
            Motion::Down => self.step_vertical(1, 1),
            Motion::PageUp => self.step_vertical(-1, page.max(1)),
            Motion::PageDown => self.step_vertical(1, page.max(1)),
            Motion::Home => self.cursor.column = CharIdx(0),
            Motion::End => self.cursor.column = coords::char_len(&self.cursor_line()),
            Motion::WordLeft => self.step_word(-1),
            Motion::WordRight => self.step_word(1),
            Motion::DocumentStart => {
                self.cursor.line = 0;
                self.cursor.column = CharIdx(0);
            }
            Motion::DocumentEnd => {
                self.cursor.line = last_line;
                self.cursor.column = coords::char_len(&self.line(last_line));
            }
        }
        if motion.is_horizontal() {
            self.remember_column();
        }
    }

    /// Places the cursor at a display column on a line — the mouse's entry
    /// point, and the only one that starts from a visual coordinate.
    pub fn place_cursor(&mut self, line: usize, col: VisualCol) {
        self.clear_selection();
        self.set_cursor_at(line, col);
    }

    /// The placing itself, with no opinion about the anchor — what a drag uses.
    fn set_cursor_at(&mut self, line: usize, col: VisualCol) {
        self.history.seal();
        self.cursor.line = line.min(self.line_count().saturating_sub(1));
        self.cursor.column = coords::char_at_visual_col(&self.cursor_line(), col, self.tab_width);
        self.remember_column();
    }

    /// Jumps to a one-based line number, for the CLI's `+42` argument.
    pub fn goto_line(&mut self, line: usize) {
        self.place_cursor(line.saturating_sub(1), VisualCol(0));
    }

    fn step_left(&mut self) {
        if self.cursor.column.0 > 0 {
            self.cursor.column = coords::prev_grapheme(&self.cursor_line(), self.cursor.column);
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.column = coords::char_len(&self.cursor_line());
        }
    }

    fn step_right(&mut self) {
        let len = coords::char_len(&self.cursor_line());
        if self.cursor.column < len {
            self.cursor.column = coords::next_grapheme(&self.cursor_line(), self.cursor.column);
        } else if self.cursor.line + 1 < self.line_count() {
            self.cursor.line += 1;
            self.cursor.column = CharIdx(0);
        }
    }

    /// Vertical movement lands on the preferred column, then snaps onto a
    /// cluster boundary so the caret can never sit inside a character.
    fn step_vertical(&mut self, direction: isize, distance: usize) {
        let last_line = self.line_count().saturating_sub(1);
        let target = if direction < 0 {
            self.cursor.line.saturating_sub(distance)
        } else {
            (self.cursor.line + distance).min(last_line)
        };
        self.cursor.line = target;
        let line = self.cursor_line();
        let column = coords::char_at_visual_col(&line, self.cursor.preferred_col, self.tab_width);
        self.cursor.column = coords::snap(&line, column);
    }

    fn step_word(&mut self, direction: isize) {
        let line = self.cursor_line().into_owned();
        if direction < 0 {
            if self.cursor.column.0 == 0 {
                self.step_left();
                return;
            }
            self.cursor.column = coords::prev_word(&line, self.cursor.column);
        } else {
            if self.cursor.column >= coords::char_len(&line) {
                self.step_right();
                return;
            }
            self.cursor.column = coords::next_word(&line, self.cursor.column);
        }
    }

    fn remember_column(&mut self) {
        self.cursor.preferred_col = self.cursor_visual_col();
    }

    // --- editing -----------------------------------------------------------

    pub fn insert_char(&mut self, ch: char) {
        let mut buffer = [0u8; 4];
        self.insert(ch.encode_utf8(&mut buffer));
    }

    pub fn insert_newline(&mut self) {
        self.insert("\n");
    }

    /// Inserts pasted text at the cursor, replacing the selection.
    ///
    /// The line endings of the pasted text are normalised to the `\n` the rope
    /// holds, exactly as they are when a file is opened: a paste out of a
    /// Windows editor must not sprinkle carriage returns the user cannot see
    /// into a buffer (ADR-009 — the file's own ending is still what is saved).
    ///
    /// It is one rope operation whatever the size, so a thousand-line paste
    /// costs one insert rather than a thousand keystrokes — which is also what
    /// makes it one undo step in Phase 4.
    pub fn insert_text(&mut self, text: &str) {
        if text.contains('\r') {
            self.insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
        } else {
            self.insert(text);
        }
    }

    /// Inserts text at the cursor and leaves the cursor after it.
    ///
    /// A selection is replaced rather than pushed aside: typing over selected
    /// text is what every editor does, and it is the same removal a cut makes
    /// (SPEC §15).
    fn insert(&mut self, text: &str) {
        // One logical edit, however many rope operations it takes: typing over
        // a selection is a remove and an insert, and must undo as one step.
        self.history.begin_edit(self.cursor);
        self.delete_selection();
        if !text.is_empty() {
            let at = self.cursor_char();
            self.touch(at);
            self.rope.insert(at, text);
            self.history.record(EditOperation::Insert {
                at: CharIdx(at),
                text: text.to_string(),
            });
            let end = at + text.chars().count();
            self.cursor.line = self.rope.char_to_line(end);
            self.cursor.column = CharIdx(end - self.rope.line_to_char(self.cursor.line));
            self.dirty = true;
            self.remember_column();
        }
        self.history.end_edit(self.cursor);
    }

    /// Deletes the selection, or the cluster before the cursor, or joins with
    /// the previous line.
    pub fn backspace(&mut self) {
        self.history.begin_edit(self.cursor);
        self.backspace_inner();
        self.history.end_edit(self.cursor);
    }

    fn backspace_inner(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor.column.0 > 0 {
            let target = coords::prev_grapheme(&self.cursor_line(), self.cursor.column);
            let start = self.rope.line_to_char(self.cursor.line) + target.0;
            let end = self.cursor_char();
            self.remove(start..end);
            self.cursor.column = target;
        } else if self.cursor.line > 0 {
            let at = self.cursor_char();
            self.cursor.line -= 1;
            self.cursor.column = coords::char_len(&self.cursor_line());
            self.remove(at - 1..at);
        } else {
            return;
        }
        self.remember_column();
    }

    /// Deletes the selection, or the cluster after the cursor, or joins with
    /// the next line.
    pub fn delete(&mut self) {
        self.history.begin_edit(self.cursor);
        self.delete_inner();
        self.history.end_edit(self.cursor);
    }

    fn delete_inner(&mut self) {
        if self.delete_selection() {
            return;
        }
        let at = self.cursor_char();
        let len = coords::char_len(&self.cursor_line());
        if self.cursor.column < len {
            let target = coords::next_grapheme(&self.cursor_line(), self.cursor.column);
            let end = self.rope.line_to_char(self.cursor.line) + target.0;
            self.remove(at..end);
        } else if at < self.rope.len_chars() {
            // Only a newline can follow the last column of a line.
            self.remove(at..at + 1);
        }
    }

    fn remove(&mut self, range: std::ops::Range<usize>) {
        if range.is_empty() {
            return;
        }
        // The removed text is the inverse of the removal, so it is read out
        // before the rope loses it — a slice, never a copy of the buffer.
        let text = self.rope.slice(range.clone()).to_string();
        self.touch(range.start);
        self.rope.remove(range.clone());
        self.history.record(EditOperation::Delete {
            at: CharIdx(range.start),
            text,
        });
        self.dirty = true;
    }

    // --- search ------------------------------------------------------------

    /// Every occurrence of `query` in the buffer, up to `limit`.
    ///
    /// The bool is whether the limit cut the list short: `e` in a five-megabyte
    /// file is half a million hits, and a vector of them is both a memory
    /// spike and a count nobody reads. The bar says `500+` and search still
    /// works — the matches past the limit are simply not offered as
    /// destinations.
    pub fn find_all(&self, query: &str, case_sensitive: bool, limit: usize) -> (Vec<Match>, bool) {
        let mut matches = Vec::new();
        if query.is_empty() {
            return (matches, false);
        }
        for line in 0..self.line_count() {
            for (start, end) in search::find_in_line(&self.line(line), query, case_sensitive) {
                if matches.len() == limit {
                    return (matches, true);
                }
                matches.push(Match { line, start, end });
            }
        }
        (matches, false)
    }

    /// Selects a match and leaves the caret at its end.
    ///
    /// The current hit *is* the selection, which is what makes `Ctrl+C` on it
    /// copy the match and what gives Replace something to act on without a
    /// second notion of "where the search is".
    pub fn select_match(&mut self, hit: Match) {
        self.history.seal();
        let line = hit.line.min(self.line_count().saturating_sub(1));
        let len = coords::char_len(&self.line(line));
        self.anchor = Some(Position::new(line, hit.start.min(len)));
        self.cursor.line = line;
        self.cursor.column = hit.end.min(len);
        self.remember_column();
    }

    /// Replaces `matches` with `replacement` as one undo step.
    ///
    /// Applied last to first, so the offsets of the hits still ahead of the
    /// edit are the ones they were found at — rewriting forwards would shift
    /// every remaining match by the length difference and need a running
    /// correction that is only ever right until it is not.
    ///
    /// Returns how many were replaced. The caret ends at the end of the first
    /// (topmost) replacement, so a Replace All leaves the user where the work
    /// started rather than at the bottom of the file.
    pub fn replace_matches(&mut self, matches: &[Match], replacement: &str) -> usize {
        if matches.is_empty() {
            return 0;
        }
        // Nothing may merge into the step before this one: a replace-all is one
        // action however many operations it takes (SPEC §23).
        self.history.seal();
        self.history.begin_edit(self.cursor);
        for hit in matches.iter().rev() {
            let start = self.char_of(Position::new(hit.line, hit.start));
            let end = self.char_of(Position::new(hit.line, hit.end));
            self.remove(start..end);
            if !replacement.is_empty() {
                self.touch(start);
                self.rope.insert(start, replacement);
                self.history.record(EditOperation::Insert {
                    at: CharIdx(start),
                    text: replacement.to_string(),
                });
                self.dirty = true;
            }
        }
        let first = matches[0];
        self.clear_selection();
        self.cursor.line = first.line;
        self.cursor.column = CharIdx(first.start.0 + replacement.chars().count());
        self.remember_column();
        self.history.end_edit(self.cursor);
        self.history.seal();
        matches.len()
    }

    // --- undo / redo -------------------------------------------------------

    /// Reverses the newest undo step and puts the caret back where it was
    /// before it. Returns whether there was anything to undo.
    pub fn undo(&mut self) -> bool {
        let Some(transaction) = self.history.take_undo() else {
            return false;
        };
        // Backwards: a later operation was applied to the buffer a earlier one
        // had already changed, so its offsets are only valid until it is gone.
        for op in transaction.ops.iter().rev() {
            self.revert(op);
        }
        self.restore_cursor(transaction.cursor_before);
        self.history.push_redo(transaction);
        self.dirty = !self.history.at_saved_point();
        true
    }

    /// Re-applies the step undone last. Returns whether there was one.
    pub fn redo(&mut self) -> bool {
        let Some(transaction) = self.history.take_redo() else {
            return false;
        };
        for op in &transaction.ops {
            self.reapply(op);
        }
        self.restore_cursor(transaction.cursor_after);
        self.history.push_undo(transaction);
        self.dirty = !self.history.at_saved_point();
        true
    }

    fn revert(&mut self, op: &EditOperation) {
        match op {
            EditOperation::Insert { at, text } => {
                self.touch(at.0);
                self.rope.remove(at.0..at.0 + text.chars().count());
            }
            EditOperation::Delete { at, text } => {
                self.touch(at.0);
                self.rope.insert(at.0, text);
            }
        }
    }

    fn reapply(&mut self, op: &EditOperation) {
        match op {
            EditOperation::Insert { at, text } => {
                self.touch(at.0);
                self.rope.insert(at.0, text);
            }
            EditOperation::Delete { at, text } => {
                self.touch(at.0);
                self.rope.remove(at.0..at.0 + text.chars().count());
            }
        }
    }

    /// Puts the caret back at a remembered position, clamped to the buffer the
    /// undo has just produced. A stored cursor is valid by construction; the
    /// clamp is what keeps a future bug from becoming a panic (SPEC §45).
    fn restore_cursor(&mut self, cursor: Cursor) {
        self.clear_selection();
        self.cursor.line = cursor.line.min(self.line_count().saturating_sub(1));
        let len = coords::char_len(&self.cursor_line());
        self.cursor.column = coords::snap(&self.cursor_line(), cursor.column.min(len));
        self.remember_column();
    }
}

/// Splits a rope line into its text and whether it was newline-terminated.
fn split_terminator(line: RopeSlice<'_>) -> (RopeSlice<'_>, bool) {
    let len = line.len_chars();
    if len > 0 && line.char(len - 1) == '\n' {
        (line.slice(..len - 1), true)
    } else {
        (line, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::coords::char_len;

    const FIXTURE: &str = include_str!("../../tests/fixtures/unicode.txt");

    fn doc(text: &str) -> Document {
        Document::from_text(text, Some(PathBuf::from("test.txt")))
    }

    /// The whole buffer as one string. Tests only — the app never does this.
    fn text(document: &Document) -> String {
        document.rope.to_string()
    }

    #[test]
    fn an_empty_document_has_one_empty_line() {
        let document = doc("");
        assert_eq!(document.line_count(), 1);
        assert_eq!(document.line(0), "");
        assert_eq!(document.cursor().line, 0);
    }

    #[test]
    fn a_trailing_newline_makes_a_last_empty_line() {
        let document = doc("a\n");
        assert_eq!(document.line_count(), 2);
        assert_eq!(document.line(0), "a");
        assert_eq!(document.line(1), "");
    }

    #[test]
    fn lines_are_returned_without_their_terminator() {
        let document = doc("one\ntwo\nthree");
        assert_eq!(document.line(0), "one");
        assert_eq!(document.line(2), "three");
        assert_eq!(document.line(9), "", "past the end is empty, not a panic");
    }

    #[test]
    fn crlf_is_detected_and_kept_out_of_the_buffer() {
        let document = doc("one\r\ntwo\r\n");
        assert_eq!(document.line_ending(), LineEnding::Crlf);
        assert_eq!(document.line(0), "one", "no stray carriage return");
        assert_eq!(text(&document), "one\ntwo\n");
    }

    #[test]
    fn a_file_without_a_terminator_is_lf() {
        assert_eq!(doc("no newline").line_ending(), LineEnding::Lf);
        assert_eq!(doc("").line_ending(), LineEnding::Lf);
    }

    #[test]
    fn typing_inserts_at_the_cursor_and_moves_it_along() {
        let mut document = doc("");
        for ch in "fn".chars() {
            document.insert_char(ch);
        }
        assert_eq!(text(&document), "fn");
        assert_eq!(document.cursor().column, CharIdx(2));
        assert!(document.is_dirty());
    }

    #[test]
    fn a_new_document_is_clean_until_it_is_edited() {
        let mut document = doc("x");
        assert!(!document.is_dirty());
        document.move_cursor(Motion::Right, 10);
        assert!(!document.is_dirty(), "moving is not editing");
        document.insert_char('y');
        assert!(document.is_dirty());
    }

    #[test]
    fn newline_splits_the_line_at_the_cursor() {
        let mut document = doc("abcd");
        document.move_cursor(Motion::Right, 10);
        document.move_cursor(Motion::Right, 10);
        document.insert_newline();
        assert_eq!(text(&document), "ab\ncd");
        assert_eq!(document.cursor().line, 1);
        assert_eq!(document.cursor().column, CharIdx(0));
    }

    #[test]
    fn backspace_removes_a_whole_grapheme_cluster() {
        // e + combining acute: one press must take both chars.
        let mut document = doc("e\u{301}");
        document.move_cursor(Motion::End, 10);
        document.backspace();
        assert_eq!(text(&document), "");
    }

    #[test]
    fn backspace_at_the_start_of_a_line_joins_it_to_the_previous_one() {
        let mut document = doc("one\ntwo");
        document.move_cursor(Motion::Down, 10);
        document.move_cursor(Motion::Home, 10);
        document.backspace();
        assert_eq!(text(&document), "onetwo");
        assert_eq!(document.cursor().line, 0);
        assert_eq!(document.cursor().column, CharIdx(3));
    }

    #[test]
    fn backspace_at_the_start_of_the_document_does_nothing() {
        let mut document = doc("abc");
        document.backspace();
        assert_eq!(text(&document), "abc");
        assert!(!document.is_dirty());
    }

    #[test]
    fn delete_removes_a_whole_grapheme_cluster_forwards() {
        let mut document = doc("👨‍👩‍👧x");
        document.delete();
        assert_eq!(text(&document), "x", "the whole ZWJ sequence goes at once");
    }

    #[test]
    fn delete_at_the_end_of_a_line_pulls_the_next_one_up() {
        let mut document = doc("one\ntwo");
        document.move_cursor(Motion::End, 10);
        document.delete();
        assert_eq!(text(&document), "onetwo");
    }

    #[test]
    fn delete_at_the_end_of_the_document_does_nothing() {
        let mut document = doc("abc");
        document.move_cursor(Motion::DocumentEnd, 10);
        document.delete();
        assert_eq!(text(&document), "abc");
        assert!(!document.is_dirty());
    }

    #[test]
    fn left_and_right_cross_line_boundaries() {
        let mut document = doc("ab\ncd");
        document.move_cursor(Motion::End, 10);
        document.move_cursor(Motion::Right, 10);
        assert_eq!((document.cursor().line, document.cursor().column.0), (1, 0));
        document.move_cursor(Motion::Left, 10);
        assert_eq!((document.cursor().line, document.cursor().column.0), (0, 2));
    }

    #[test]
    fn vertical_movement_keeps_the_preferred_column_over_a_short_line() {
        let mut document = doc("abcdef\n\nabcdef");
        document.move_cursor(Motion::End, 10);
        assert_eq!(document.cursor_visual_col(), VisualCol(6));
        document.move_cursor(Motion::Down, 10);
        assert_eq!(document.cursor().column, CharIdx(0), "the line is empty");
        document.move_cursor(Motion::Down, 10);
        assert_eq!(
            document.cursor().column,
            CharIdx(6),
            "the column survived the short line"
        );
    }

    #[test]
    fn vertical_movement_never_lands_inside_a_cluster() {
        // The wide line puts column 1 in the middle of 日; the cursor must snap.
        let mut document = doc("xx\n日本語");
        document.move_cursor(Motion::Right, 10);
        document.move_cursor(Motion::Down, 10);
        assert_eq!(document.cursor().column, CharIdx(0));
        assert_eq!(document.cursor_visual_col(), VisualCol(0));
    }

    #[test]
    fn page_movement_stops_at_the_ends_of_the_document() {
        let mut document = doc("1\n2\n3\n4\n5");
        document.move_cursor(Motion::PageDown, 3);
        assert_eq!(document.cursor().line, 3);
        document.move_cursor(Motion::PageDown, 3);
        assert_eq!(document.cursor().line, 4);
        document.move_cursor(Motion::PageUp, 100);
        assert_eq!(document.cursor().line, 0);
    }

    #[test]
    fn word_motion_crosses_lines_at_their_ends() {
        let mut document = doc("let x\nlet y");
        document.move_cursor(Motion::End, 10);
        document.move_cursor(Motion::WordRight, 10);
        assert_eq!((document.cursor().line, document.cursor().column.0), (1, 0));
        document.move_cursor(Motion::WordLeft, 10);
        assert_eq!((document.cursor().line, document.cursor().column.0), (0, 5));
    }

    #[test]
    fn document_start_and_end_reach_both_ends() {
        let mut document = doc("one\ntwo\nthree");
        document.move_cursor(Motion::DocumentEnd, 10);
        assert_eq!((document.cursor().line, document.cursor().column.0), (2, 5));
        document.move_cursor(Motion::DocumentStart, 10);
        assert_eq!((document.cursor().line, document.cursor().column.0), (0, 0));
    }

    #[test]
    fn the_mouse_places_the_cursor_by_display_column() {
        let mut document = doc("日本語");
        document.place_cursor(0, VisualCol(3));
        assert_eq!(
            document.cursor().column,
            CharIdx(1),
            "inside 本 lands before it"
        );
        document.place_cursor(99, VisualCol(99));
        assert_eq!(document.cursor().line, 0, "clamped to the last line");
        assert_eq!(document.cursor().column, char_len("日本語"));
    }

    #[test]
    fn goto_line_is_one_based() {
        let mut document = doc("1\n2\n3");
        document.goto_line(3);
        assert_eq!(document.cursor().line, 2);
        document.goto_line(0);
        assert_eq!(document.cursor().line, 0, "a zero argument is line 1");
        document.goto_line(999);
        assert_eq!(document.cursor().line, 2, "past the end is the last line");
    }

    #[test]
    fn every_fixture_line_can_be_walked_and_deleted_backwards() {
        let mut document = Document::from_text(FIXTURE, None);
        document.move_cursor(Motion::DocumentEnd, 10);
        // Every press must make progress and never split a cluster; 64 is well
        // over the fixture's cluster count and well under an infinite loop.
        for _ in 0..64 {
            document.backspace();
        }
        assert_eq!(text(&document), "");
    }

    // --- selection ---------------------------------------------------------

    #[test]
    fn shift_navigation_leaves_an_anchor_behind_and_plain_navigation_drops_it() {
        let mut document = doc("hello world");
        assert!(document.selection().is_none());
        document.extend_cursor(Motion::WordRight, 10);
        let selection = document.selection().expect("Shift+Ctrl+Right selects");
        assert_eq!(selection.start(), Position::new(0, CharIdx(0)));
        assert_eq!(document.selected_text().as_deref(), Some("hello "));
        document.move_cursor(Motion::Left, 10);
        assert!(document.selection().is_none(), "moving dismisses it");
    }

    #[test]
    fn a_selection_can_be_extended_backwards() {
        let mut document = doc("hello");
        document.move_cursor(Motion::End, 10);
        document.extend_cursor(Motion::Left, 10);
        document.extend_cursor(Motion::Left, 10);
        assert_eq!(document.selected_text().as_deref(), Some("lo"));
        assert_eq!(document.cursor().column, CharIdx(3), "the head moved");
    }

    #[test]
    fn extending_across_lines_takes_the_newlines_with_it() {
        let mut document = doc("one\ntwo\nthree");
        document.extend_cursor(Motion::Down, 10);
        document.extend_cursor(Motion::End, 10);
        assert_eq!(document.selected_text().as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn extending_over_a_cluster_never_splits_it() {
        let mut document = doc("é👨‍👩‍👧x");
        document.extend_cursor(Motion::Right, 10);
        assert_eq!(document.selected_text().as_deref(), Some("é"));
        document.extend_cursor(Motion::Right, 10);
        assert_eq!(document.selected_text().as_deref(), Some("é👨‍👩‍👧"));
    }

    #[test]
    fn select_all_takes_the_whole_buffer() {
        let mut document = doc("one\ntwo\nthree\n");
        document.select_all();
        assert_eq!(
            document.selected_text().as_deref(),
            Some("one\ntwo\nthree\n")
        );
        assert_eq!(document.selected_len(), 14);
        assert_eq!(document.cursor().line, 3, "the caret ends at the end");
    }

    #[test]
    fn select_all_on_an_empty_document_selects_nothing() {
        let mut document = doc("");
        document.select_all();
        assert!(document.selection().is_none());
        assert_eq!(document.selected_len(), 0);
    }

    #[test]
    fn double_clicking_selects_the_word_under_the_pointer() {
        let mut document = doc("let value = 1;");
        document.select_word_at(0, VisualCol(6));
        assert_eq!(document.selected_text().as_deref(), Some("value"));
        // A click on the space between words selects that run of spaces, not
        // nothing: an empty selection would read as a click that did nothing.
        document.select_word_at(0, VisualCol(3));
        assert_eq!(document.selected_text().as_deref(), Some(" "));
    }

    #[test]
    fn double_clicking_a_non_ascii_word_selects_all_of_it() {
        let mut document = doc("Привіт світ");
        document.select_word_at(0, VisualCol(8));
        assert_eq!(document.selected_text().as_deref(), Some("світ"));
    }

    #[test]
    fn double_clicking_cjk_selects_one_ideograph() {
        // UAX #29 puts a word boundary between ideographs — Japanese is written
        // without spaces, so there is nothing else to break on. Selecting the
        // character under the pointer is what the standard segmentation gives,
        // and dictionary-based segmentation is not something to invent here.
        let mut document = doc("日本語 text");
        document.select_word_at(0, VisualCol(2));
        assert_eq!(document.selected_text().as_deref(), Some("本"));
    }

    #[test]
    fn dragging_extends_from_where_the_button_went_down() {
        let mut document = doc("one\ntwo\nthree");
        document.place_cursor(0, VisualCol(1));
        document.extend_to(2, VisualCol(3));
        assert_eq!(document.selected_text().as_deref(), Some("ne\ntwo\nthr"));
        document.extend_to(0, VisualCol(0));
        assert_eq!(document.selected_text().as_deref(), Some("o"), "and back");
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut document = doc("hello world");
        document.extend_cursor(Motion::WordRight, 10);
        document.insert_char('!');
        assert_eq!(text(&document), "!world");
        assert!(document.selection().is_none());
        assert_eq!(document.cursor().column, CharIdx(1));
    }

    #[test]
    fn backspace_and_delete_take_the_selection_rather_than_one_character() {
        let mut document = doc("hello world");
        document.extend_cursor(Motion::WordRight, 10);
        document.backspace();
        assert_eq!(text(&document), "world");

        let mut document = doc("hello world");
        document.extend_cursor(Motion::WordRight, 10);
        document.delete();
        assert_eq!(text(&document), "world");
    }

    #[test]
    fn deleting_a_selection_that_spans_lines_joins_what_is_left() {
        let mut document = doc("one\ntwo\nthree");
        document.move_cursor(Motion::Right, 10);
        document.extend_cursor(Motion::Down, 10);
        document.extend_cursor(Motion::Down, 10);
        assert!(document.delete_selection());
        assert_eq!(text(&document), "ohree");
        assert_eq!(document.cursor().line, 0);
        assert_eq!(document.cursor().column, CharIdx(1));
    }

    #[test]
    fn deleting_with_nothing_selected_changes_nothing() {
        let mut document = doc("text");
        assert!(!document.delete_selection());
        assert!(!document.is_dirty());
    }

    #[test]
    fn a_paste_is_one_operation_whatever_it_contains() {
        let mut document = doc("ab");
        document.move_cursor(Motion::Right, 10);
        document.insert_text("one\ntwo\n");
        assert_eq!(text(&document), "aone\ntwo\nb");
        assert_eq!(document.cursor().line, 2);
        assert_eq!(document.cursor().column, CharIdx(0));
    }

    #[test]
    fn a_paste_out_of_a_windows_editor_leaves_no_carriage_returns_behind() {
        let mut document = doc("");
        document.insert_text("one\r\ntwo\rthree");
        assert_eq!(text(&document), "one\ntwo\nthree");
        assert_eq!(document.line_count(), 3);
    }

    #[test]
    fn a_paste_replaces_the_selection() {
        let mut document = doc("hello world");
        document.select_all();
        document.insert_text("Привіт");
        assert_eq!(text(&document), "Привіт");
    }

    #[test]
    fn saving_does_not_disturb_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sel.txt");
        std::fs::write(&path, "one two").unwrap();

        let mut document = Document::open(&path).unwrap();
        document.extend_cursor(Motion::WordRight, 10);
        document.save().unwrap();
        assert_eq!(document.selected_text().as_deref(), Some("one "));
    }

    #[test]
    fn open_reads_a_utf8_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unicode.txt");
        std::fs::write(&path, FIXTURE).unwrap();

        let document = Document::open(&path).unwrap();
        assert_eq!(document.line(1), "Привіт");
        assert_eq!(document.title(), "unicode.txt");
        assert!(!document.is_dirty());
    }

    #[test]
    fn open_reports_a_missing_file_instead_of_panicking() {
        let err = Document::open(Path::new("/no/such/file.txt")).unwrap_err();
        assert!(matches!(err, DocumentError::Io { .. }));
        assert!(err.to_string().contains("/no/such/file.txt"));
    }

    #[test]
    fn a_path_that_does_not_exist_yet_opens_as_an_empty_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");

        let document = Document::open_or_create(&path).unwrap();
        assert_eq!(document.line_count(), 1);
        assert_eq!(document.title(), "new.txt");
        assert!(!document.is_dirty());
        assert!(!path.exists(), "naming a file must not create it");
    }

    #[test]
    fn saving_a_new_buffer_creates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");

        let mut document = Document::open_or_create(&path).unwrap();
        document.insert_char('x');
        document.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x");
    }

    #[test]
    fn open_or_create_still_reports_a_failure_that_is_not_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        // A directory is not a missing file, so it must not open as a buffer
        // that would then overwrite it on save.
        assert!(Document::open_or_create(dir.path()).is_err());
    }

    #[test]
    fn open_refuses_a_file_that_is_not_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin1.txt");
        std::fs::write(&path, [0xff, 0xfe, 0x41]).unwrap();
        assert!(matches!(
            Document::open(&path).unwrap_err(),
            DocumentError::NotUtf8 { .. }
        ));
    }

    #[test]
    fn save_round_trips_an_lf_file_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unicode.txt");
        std::fs::write(&path, FIXTURE).unwrap();

        let mut document = Document::open(&path).unwrap();
        document.insert_char('!');
        assert!(document.is_dirty());
        document.save().unwrap();

        assert!(!document.is_dirty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("!{FIXTURE}")
        );
    }

    #[test]
    fn save_writes_back_the_crlf_it_opened_with() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crlf.txt");
        std::fs::write(&path, "one\r\ntwo\r\n").unwrap();

        let mut document = Document::open(&path).unwrap();
        document.move_cursor(Motion::End, 10);
        document.insert_char('!');
        document.save().unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one!\r\ntwo\r\n");
    }

    #[test]
    fn save_does_not_add_a_trailing_newline_to_a_file_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bare.txt");
        std::fs::write(&path, "no newline").unwrap();

        let mut document = Document::open(&path).unwrap();
        document.insert_char('x');
        document.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "xno newline");
    }

    #[test]
    fn saving_a_document_with_no_path_is_an_error_not_a_panic() {
        let mut document = Document::from_text("text", None);
        assert!(matches!(document.save(), Err(DocumentError::NoPath)));
    }

    // --- undo / redo -------------------------------------------------------

    /// Types a string one character at a time, the way the keymap does.
    fn type_text(document: &mut Document, text: &str) {
        for ch in text.chars() {
            document.insert_char(ch);
        }
    }

    #[test]
    fn a_typed_word_undoes_as_a_word() {
        let mut document = doc("");
        type_text(&mut document, "hello world");
        assert!(document.undo());
        assert_eq!(
            text(&document),
            "hello ",
            "the last word, not the last letter"
        );
        assert!(document.undo());
        assert_eq!(text(&document), "hello");
        assert!(document.undo());
        assert_eq!(text(&document), "");
        assert!(!document.undo(), "nothing left to undo");
    }

    #[test]
    fn redo_replays_the_step_undone_last() {
        let mut document = doc("");
        type_text(&mut document, "hello");
        document.undo();
        assert!(document.redo());
        assert_eq!(text(&document), "hello");
        assert_eq!(
            document.cursor().column,
            CharIdx(5),
            "and where it left the caret"
        );
        assert!(!document.redo());
    }

    #[test]
    fn undo_puts_the_caret_back_where_the_edit_started() {
        let mut document = doc("one\ntwo\n");
        document.move_cursor(Motion::Down, 10);
        document.move_cursor(Motion::End, 10);
        type_text(&mut document, "!");
        document.undo();
        assert_eq!(document.cursor().line, 1);
        assert_eq!(document.cursor().column, CharIdx(3));
    }

    #[test]
    fn moving_the_caret_ends_the_undo_step() {
        let mut document = doc("");
        type_text(&mut document, "ab");
        document.move_cursor(Motion::Left, 10);
        type_text(&mut document, "c");
        document.undo();
        assert_eq!(text(&document), "ab", "only what was typed after the move");
    }

    #[test]
    fn typing_over_a_selection_undoes_in_one_step() {
        let mut document = doc("hello");
        document.select_all();
        document.insert_char('x');
        assert_eq!(text(&document), "x");
        assert!(document.undo());
        assert_eq!(
            text(&document),
            "hello",
            "the removal and the insert are one step"
        );
        assert!(!document.undo());
    }

    #[test]
    fn a_paste_undoes_in_one_step() {
        let mut document = doc("");
        document.insert_text("one\ntwo\nthree");
        assert!(document.undo());
        assert_eq!(text(&document), "");
        document.redo();
        assert_eq!(text(&document), "one\ntwo\nthree");
    }

    #[test]
    fn a_paste_does_not_swallow_the_keystroke_after_it() {
        let mut document = doc("");
        document.insert_text("pasted");
        type_text(&mut document, "!");
        document.undo();
        assert_eq!(text(&document), "pasted");
    }

    #[test]
    fn backspacing_a_word_undoes_as_a_word() {
        let mut document = doc("hello world");
        document.move_cursor(Motion::End, 10);
        for _ in 0..5 {
            document.backspace();
        }
        assert_eq!(text(&document), "hello ");
        assert!(document.undo());
        assert_eq!(text(&document), "hello world");
    }

    #[test]
    fn each_newline_is_its_own_undo_step() {
        let mut document = doc("");
        document.insert_newline();
        document.insert_newline();
        document.undo();
        assert_eq!(text(&document), "\n");
    }

    #[test]
    fn undoing_a_cluster_deletion_puts_the_whole_cluster_back() {
        let mut document = doc("a\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}b");
        document.move_cursor(Motion::End, 10);
        document.backspace();
        document.backspace();
        assert_eq!(
            text(&document),
            "a",
            "b and the whole ZWJ sequence are gone"
        );
        // Two steps, because a letter and an emoji are different runs. The
        // first put back the whole sequence: a cluster, not a `char`.
        assert!(document.undo());
        assert_eq!(
            text(&document),
            "a\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"
        );
        assert!(document.undo());
        assert_eq!(
            text(&document),
            "a\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}b"
        );
    }

    #[test]
    fn a_cut_undoes_in_one_step() {
        let mut document = doc("one\ntwo\nthree\n");
        document.extend_cursor(Motion::Down, 10);
        document.extend_cursor(Motion::Down, 10);
        assert_eq!(document.selected_text().as_deref(), Some("one\ntwo\n"));
        document.delete_selection();
        assert_eq!(text(&document), "three\n");
        assert!(document.undo());
        assert_eq!(text(&document), "one\ntwo\nthree\n");
    }

    #[test]
    fn editing_after_an_undo_drops_the_redo_branch() {
        let mut document = doc("");
        type_text(&mut document, "abc");
        document.undo();
        type_text(&mut document, "xyz");
        assert!(!document.redo(), "the undone branch is gone");
        assert_eq!(text(&document), "xyz");
    }

    #[test]
    fn undoing_back_to_the_last_save_clears_the_dirty_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("undo.txt");
        std::fs::write(&path, "start").unwrap();

        let mut document = Document::open(&path).unwrap();
        assert!(!document.is_dirty());
        type_text(&mut document, "!");
        assert!(document.is_dirty());
        document.undo();
        assert!(!document.is_dirty(), "the buffer matches the file again");
        document.redo();
        assert!(document.is_dirty());

        document.save().unwrap();
        assert!(!document.is_dirty());
        document.undo();
        assert!(
            document.is_dirty(),
            "undone past the save, so it differs again"
        );
    }

    #[test]
    fn undo_survives_a_document_it_can_no_longer_fit_in() {
        // The stored cursor is clamped, not trusted: a bug elsewhere must cost
        // a misplaced caret, never a panic (SPEC §45).
        let mut document = doc("one\ntwo\n");
        document.move_cursor(Motion::DocumentEnd, 10);
        type_text(&mut document, "x");
        document.select_all();
        document.delete_selection();
        assert!(document.undo());
        assert!(document.undo());
        assert_eq!(text(&document), "one\ntwo\n");
    }
    #[test]
    fn find_all_reports_every_hit_in_reading_order() {
        let document = Document::from_text("foo\nbar foo\nFOO\n", None);
        let (matches, truncated) = document.find_all("foo", true, 100);
        assert!(!truncated);
        assert_eq!(
            matches
                .iter()
                .map(|m| (m.line, m.start.0, m.end.0))
                .collect::<Vec<_>>(),
            vec![(0, 0, 3), (1, 4, 7)]
        );
        let (insensitive, _) = document.find_all("foo", false, 100);
        assert_eq!(insensitive.len(), 3, "the third hit is `FOO` on line 3");
    }

    #[test]
    fn find_all_stops_at_the_limit_and_says_so() {
        let document = Document::from_text(&"x\n".repeat(50), None);
        let (matches, truncated) = document.find_all("x", true, 10);
        assert_eq!(matches.len(), 10);
        assert!(truncated);
    }

    #[test]
    fn selecting_a_match_makes_it_the_selection() {
        let mut document = Document::from_text("bar foo baz\n", None);
        let (matches, _) = document.find_all("foo", true, 10);
        document.select_match(matches[0]);
        assert_eq!(document.selected_text().as_deref(), Some("foo"));
        assert_eq!(document.cursor().column, CharIdx(7), "caret at the end");
    }

    #[test]
    fn replace_all_is_one_undo_step() {
        let mut document = Document::from_text("foo foo\nfoo\n", None);
        let (matches, _) = document.find_all("foo", true, 10);
        assert_eq!(document.replace_matches(&matches, "quux"), 3);
        assert_eq!(text(&document), "quux quux\nquux\n");

        assert!(document.undo());
        assert_eq!(text(&document), "foo foo\nfoo\n", "one step, not three");
        assert!(document.redo());
        assert_eq!(text(&document), "quux quux\nquux\n");
    }

    #[test]
    fn replace_all_with_a_shorter_string_keeps_the_later_hits_aligned() {
        // Applied last-to-first precisely so this cannot drift.
        let mut document = Document::from_text("aaa aaa aaa\n", None);
        let (matches, _) = document.find_all("aaa", true, 10);
        document.replace_matches(&matches, "b");
        assert_eq!(text(&document), "b b b\n");
    }

    #[test]
    fn replacing_with_nothing_deletes_the_matches() {
        let mut document = Document::from_text("keep-DROP-keep\n", None);
        let (matches, _) = document.find_all("DROP", true, 10);
        document.replace_matches(&matches, "");
        assert_eq!(text(&document), "keep--keep\n");
    }

    #[test]
    fn a_replace_leaves_the_caret_after_the_first_replacement() {
        let mut document = Document::from_text("one two one\n", None);
        let (matches, _) = document.find_all("one", true, 10);
        document.replace_matches(&matches, "1");
        assert_eq!(document.cursor().line, 0);
        assert_eq!(document.cursor().column, CharIdx(1));
        assert!(document.selection().is_none());
    }

    #[test]
    fn a_replace_all_does_not_merge_into_the_typing_before_it() {
        let mut document = Document::from_text("foo\n", None);
        document.insert_char('x');
        let (matches, _) = document.find_all("foo", true, 10);
        document.replace_matches(&matches, "bar");
        assert_eq!(text(&document), "xbar\n");
        assert!(document.undo());
        assert_eq!(text(&document), "xfoo\n", "the replace undoes on its own");
        assert!(document.undo());
        assert_eq!(text(&document), "foo\n");
    }

    #[test]
    fn the_revision_counts_changes_and_nothing_else() {
        let mut document = Document::from_text("a\n", None);
        let start = document.revision();
        document.move_cursor(Motion::Right, 10);
        assert_eq!(document.revision(), start, "moving is not a change");
        document.insert_char('b');
        assert!(document.revision() > start);
        let typed = document.revision();
        document.undo();
        assert!(document.revision() > typed, "undo changes the buffer too");
    }
}
