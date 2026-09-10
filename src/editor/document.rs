//! Rope-backed document, line-ending detection, load/save.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use ropey::{Rope, RopeSlice};
use thiserror::Error;

use crate::editor::charset::Charset;
use crate::editor::compression::{self, Compression};
use crate::editor::coords::{self, ByteIdx, CharIdx, GraphemeIdx, VisualCol, DEFAULT_TAB_WIDTH};
use crate::editor::cursor::{Cursor, Motion};
use crate::editor::history::{EditOperation, History};
use crate::editor::search::{self, Match};
use crate::editor::selection::{Position, Selection};
use crate::editor::wrap::{self, Layout, Row};

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

    /// Where each one is the convention, for a picker row: `LF` and `CRLF` are
    /// the names of the bytes, and the answer to "which do I want?" is the
    /// platform the file is going to.
    pub fn platforms(self) -> &'static str {
        match self {
            Self::Lf => "Unix, macOS",
            Self::Crlf => "Windows",
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
    /// A file with a NUL byte in it, which is the one thing no text file has
    /// (ADR-059). It is refused rather than opened as mojibake: every legacy
    /// charset decodes every byte, so without this guard a JPEG would open as
    /// six hundred kilobytes of line noise.
    #[error("{path}: not a text file")]
    Binary { path: String },
    /// A character the file's charset cannot hold, found before anything was
    /// written. The buffer and the file are both untouched.
    #[error("{charset} cannot hold {character:?} — save as UTF-8, or convert")]
    Unmappable {
        charset: &'static str,
        character: char,
    },
    /// A save aimed at a file the editor only ever unpacked (ADR-074). Raised
    /// before anything is written, so the file on disk is untouched.
    #[error("{path} is read-only ({compression}) — Save As writes the text out")]
    ReadOnly {
        path: String,
        compression: &'static str,
    },
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

/// What the file looked like the last time the buffer and the disk agreed
/// (ADR-043).
///
/// Modification time *and* length, because neither is enough on its own: a
/// filesystem with one-second timestamps hides a rewrite inside the same
/// second, and a rewrite that keeps the length is exactly what a one-character
/// change by another editor is. Together they miss only a same-second edit that
/// also preserves the length, which is a narrower gap than reading the file
/// back on every event to compare it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskStamp {
    /// `None` on a filesystem that does not report one — the length is then
    /// the whole of the comparison rather than a reason to give up on it.
    modified: Option<SystemTime>,
    len: u64,
}

impl DiskStamp {
    /// Reads the stamp of a path, or `None` when it cannot be stat'd.
    ///
    /// A failure here is not an error to report: it means the next comparison
    /// has nothing to compare against, and a buffer with no stamp is simply one
    /// the editor makes no claim about.
    fn of(path: &Path) -> Option<Self> {
        std::fs::metadata(path)
            .ok()
            .map(|metadata| Self::from_metadata(&metadata))
    }

    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        }
    }
}

/// How the buffer stands against the file it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskState {
    /// No path, or no stamp to compare against: a new file that has never been
    /// saved, and a buffer built from text in memory.
    Untracked,
    /// The file is what the editor last read or wrote.
    Same,
    /// Somebody else has written to it.
    Changed,
    /// It is no longer there.
    Gone,
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
    /// How the file's bytes become this text and back (ADR-059). Sniffed from
    /// the file's own byte order mark when it has one, chosen by the user
    /// otherwise, and written back unchanged by every save.
    charset: Charset,
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
    /// What the file was when the buffer and the disk last agreed, which is
    /// the moment it was opened, reloaded or saved. `None` for a buffer with no
    /// file behind it yet (ADR-043).
    disk: Option<DiskStamp>,
    /// What the file's bytes were packed in, when they were (ADR-074). Anything
    /// but `None` makes the document read-only: the editor unpacks and never
    /// packs, so there is no honest thing for a save to write back.
    compression: Compression,
    /// Whether the unpacked stream was longer than the cap and the buffer holds
    /// only its beginning (ADR-074).
    truncated: bool,
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
            charset: Charset::default(),
            cursor: Cursor::default(),
            anchor: None,
            dirty: false,
            tab_width: DEFAULT_TAB_WIDTH,
            history: History::new(),
            revision: 0,
            disk: None,
            compression: Compression::None,
            truncated: false,
            // A document nothing has highlighted yet is stale from its first
            // line, which is also what makes a freshly opened file get parsed.
            dirty_from: 0,
        }
    }

    /// Reads a file, decoding it with the charset its own bytes announce
    /// (SPEC §17, ADR-059).
    ///
    /// Reading the bytes and converting explicitly, rather than
    /// `fs::read_to_string`, is what lets a binary file be reported as such and
    /// a Latin-1 one be opened at all.
    pub fn open(path: &Path) -> Result<Self, DocumentError> {
        Self::open_as(path, None)
    }

    /// The same read with the charset named rather than sniffed — Reopen with
    /// Encoding, for the file whose bytes carry no mark and are not UTF-8.
    pub fn open_as(path: &Path, charset: Option<Charset>) -> Result<Self, DocumentError> {
        // The stamp is taken *before* the read, not after: a writer that
        // finishes between the two would otherwise be recorded as the state the
        // buffer holds, and the change would never be noticed. Taken first, the
        // worst case is a change reported that has already been read, which
        // costs a reload of text that is already right.
        let disk = DiskStamp::of(path);
        let bytes = std::fs::read(path).map_err(|err| DocumentError::io(path, err))?;
        // Unpacking comes first: what a container holds decides nothing about
        // the charset inside it, and the NUL guard below has to look at the
        // text rather than at a compressed stream, which is noise by design
        // (ADR-074).
        let unpacked = compression::unpack(bytes).map_err(|err| DocumentError::io(path, err))?;
        let (text, charset) = decode(path, &unpacked.bytes, charset)?;
        let mut document = Self::from_text(&text, Some(path.to_path_buf()));
        document.charset = charset;
        document.disk = disk;
        document.compression = unpacked.compression;
        document.truncated = unpacked.truncated;
        log::info!(
            "opened {} ({} bytes, {} lines, {}, {}{}{})",
            path.display(),
            document.len_bytes(),
            document.line_count(),
            document.line_ending.label(),
            charset.label,
            unpacked
                .compression
                .label()
                .map_or(String::new(), |label| format!(", {label}")),
            if unpacked.truncated { ", cut" } else { "" }
        );
        Ok(document)
    }

    /// Opens `path`, or starts an empty buffer for it when nothing is there yet.
    ///
    /// Naming a file that does not exist is how a file gets created: the buffer
    /// is empty and clean, and nothing is written until the user saves, so a
    /// mistyped name costs nothing. Every other failure — a directory, a
    /// permission error, a binary file — is still reported rather than
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

    /// Writes the buffer back with the line ending and the charset it was
    /// opened with.
    ///
    /// A legacy charset is encoded in *full* before the file is opened
    /// (ADR-059): it holds a few hundred characters and the buffer may hold
    /// anything, and a character it cannot take must leave the file on disk
    /// untouched rather than truncated at the byte the encoder gave up on. The
    /// Unicode charsets hold everything, so they keep the streaming path SPEC
    /// §44 asks for — which is every file the editor opens by default.
    pub fn save(&mut self) -> Result<(), DocumentError> {
        let path = self.path.clone().ok_or(DocumentError::NoPath)?;
        if let Some(compression) = self.compression.label() {
            return Err(DocumentError::ReadOnly {
                path: self.title().to_string(),
                compression,
            });
        }
        let prepared = self.encode_all()?;
        let file = File::create(&path).map_err(|err| DocumentError::io(&path, err))?;
        let mut writer = BufWriter::new(file);
        match prepared {
            Some(bytes) => writer.write_all(&bytes),
            None => self.write_all(&mut writer).and_then(|()| writer.flush()),
        }
        .map_err(|err| DocumentError::io(&path, err))?;
        self.dirty = false;
        self.history.mark_saved();
        // What is on disk is now what is in the buffer, so the watcher's report
        // of this very write is a change the editor already knows about.
        self.disk = DiskStamp::of(&path);
        log::info!("saved {} ({} bytes)", path.display(), self.len_bytes());
        Ok(())
    }

    /// How the buffer stands against the file it came from (ADR-043).
    ///
    /// One `stat`, and nothing is read: this is asked about every open tab on
    /// every burst of filesystem events, and reading each file back to compare
    /// it would make a `cargo build` in the next terminal cost the size of the
    /// working set.
    pub fn disk_state(&self) -> DiskState {
        let (Some(path), Some(stamp)) = (self.path.as_deref(), self.disk) else {
            return DiskState::Untracked;
        };
        match std::fs::metadata(path) {
            Ok(metadata) => {
                if DiskStamp::from_metadata(&metadata) == stamp {
                    DiskState::Same
                } else {
                    DiskState::Changed
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => DiskState::Gone,
            // Something is there and cannot be stat'd — a directory whose
            // permissions changed under the editor. There is nothing to say
            // about it that a reload prompt would help with.
            Err(err) => {
                log::debug!("cannot stat {}: {err}", path.display());
                DiskState::Untracked
            }
        }
    }

    /// Re-reads the file, replacing the buffer with what is on disk.
    ///
    /// It is one undo step rather than a new document (ADR-043): a reload the
    /// user did not mean is otherwise the one action in the editor that cannot
    /// be taken back, and the byte budget on the stack is what keeps the cost
    /// of holding both versions bounded (ADR-042).
    ///
    /// The caret keeps its line and column, clamped to whatever the file now
    /// has — the same rule an undo restores a cursor by, and for the same
    /// reason: a position that no longer exists must not be a panic.
    pub fn reload(&mut self) -> Result<(), DocumentError> {
        self.reread(None)
    }

    /// A reload that decodes with a charset the user named — Reopen with
    /// Encoding (ADR-059).
    ///
    /// The same undoable step a reload is, and for a stronger reason: this is
    /// the command whose whole purpose is to be tried again when the answer
    /// looks wrong, so getting back must never mean re-opening the file.
    pub fn reopen_as(&mut self, charset: Charset) -> Result<(), DocumentError> {
        self.reread(Some(charset))
    }

    fn reread(&mut self, charset: Option<Charset>) -> Result<(), DocumentError> {
        let path = self.path.clone().ok_or(DocumentError::NoPath)?;
        let disk = DiskStamp::of(&path);
        let bytes = std::fs::read(&path).map_err(|err| DocumentError::io(&path, err))?;
        let unpacked = compression::unpack(bytes).map_err(|err| DocumentError::io(&path, err))?;
        // A plain reload keeps the charset the buffer already has: it may have
        // been chosen by hand, and re-sniffing would quietly undo that choice
        // on every `F5`. The container is re-sniffed rather than kept, because
        // unlike the charset it is a fact about the bytes and the bytes are
        // what changed (ADR-074).
        let (text, charset) = decode(&path, &unpacked.bytes, charset.or(Some(self.charset)))?;
        let line_ending = LineEnding::detect(&text);
        let normalised: Cow<'_, str> = match line_ending {
            LineEnding::Lf => Cow::Borrowed(text.as_str()),
            LineEnding::Crlf => Cow::Owned(text.replace("\r\n", "\n")),
        };

        let cursor = self.cursor;
        // Nothing may merge into the step before this one, and nothing may
        // merge into it afterwards: a reload is one action, and it is not
        // typing.
        self.history.seal();
        self.history.begin_edit(cursor);
        self.remove(0..self.rope.len_chars());
        if !normalised.is_empty() {
            self.touch(0);
            self.rope.insert(0, &normalised);
            self.history.record(EditOperation::Insert {
                at: CharIdx(0),
                text: normalised.into_owned(),
            });
        }
        self.history.end_edit(cursor);
        self.history.seal();

        self.line_ending = line_ending;
        self.charset = charset;
        self.disk = disk;
        self.compression = unpacked.compression;
        self.truncated = unpacked.truncated;
        self.restore_cursor(cursor);
        // The buffer is what is on disk again, so this is the save point —
        // which is also what makes an undo of the reload report as dirty.
        self.dirty = false;
        self.history.mark_saved();
        log::info!(
            "reloaded {} ({} bytes, {} lines, {}, {}{})",
            path.display(),
            self.len_bytes(),
            self.line_count(),
            line_ending.label(),
            charset.label,
            self.compression
                .label()
                .map_or(String::new(), |label| format!(", {label}"))
        );
        Ok(())
    }

    /// The whole file as bytes, for a charset that can refuse a character —
    /// and `None` for the Unicode ones, which cannot, and are streamed instead.
    fn encode_all(&self) -> Result<Option<Vec<u8>>, DocumentError> {
        if self.charset.is_unicode() {
            return Ok(None);
        }
        let mut bytes = self.charset.bom_bytes().to_vec();
        let mut unmappable = false;
        self.for_each_piece(|text| unmappable |= self.charset.encode(text, &mut bytes));
        if unmappable {
            // Only now, once the fast answer has already said there is one.
            let mut found = None;
            self.for_each_piece(|text| {
                found = found.or_else(|| self.charset.first_unmappable(text));
            });
            return Err(DocumentError::Unmappable {
                charset: self.charset.label,
                character: found.unwrap_or('\u{fffd}'),
            });
        }
        Ok(Some(bytes))
    }

    /// Streams the rope out piece by piece — never `to_string()`, which would
    /// be an allocation the size of the file on every save (SPEC §44).
    fn write_all(&self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(self.charset.bom_bytes())?;
        let mut scratch = Vec::new();
        let mut outcome = Ok(());
        self.for_each_piece(|text| {
            if outcome.is_err() {
                return;
            }
            scratch.clear();
            self.charset.encode(text, &mut scratch);
            outcome = writer.write_all(&scratch);
        });
        outcome
    }

    /// Hands the text of the file to `piece` a chunk at a time, with the line
    /// terminators the file is written with rather than the `\n` the rope
    /// holds.
    ///
    /// One walk for both the streaming and the buffered write, so an LF file
    /// and a CRLF one cannot come out of the two paths differently.
    fn for_each_piece(&self, mut piece: impl FnMut(&str)) {
        if self.line_ending == LineEnding::Lf {
            for chunk in self.rope.chunks() {
                piece(chunk);
            }
            return;
        }
        for line in self.rope.lines() {
            let (text, terminated) = split_terminator(line);
            for chunk in text.chunks() {
                piece(chunk);
            }
            if terminated {
                piece(self.line_ending.as_str());
            }
        }
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
    /// What the file's bytes were packed in, for the status bar (ADR-074).
    pub fn compression(&self) -> Compression {
        self.compression
    }

    /// Whether the buffer may be changed and saved back.
    ///
    /// Only unpacking makes a document read-only today: the text is a *reading*
    /// of the file rather than the file, and writing one back would mean
    /// inventing a compression level and a header for it.
    pub fn is_read_only(&self) -> bool {
        self.compression != Compression::None
    }

    /// Whether the buffer is only the beginning of what the file holds
    /// (ADR-074) — a stream longer than the unpacking cap.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// The path the grammar is chosen by, which for a `dump.sql.gz` is
    /// `dump.sql`.
    ///
    /// The container is not the language, and highlighting SQL as plain text
    /// because it arrived zipped would make the unpacked view worse than the
    /// unpacked file for no reason (SPEC §21).
    pub fn syntax_path(&self) -> Option<Cow<'_, Path>> {
        let path = self.path.as_deref()?;
        if !self.is_read_only() {
            return Some(Cow::Borrowed(path));
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some("gz") => Some(Cow::Owned(path.with_extension(""))),
            _ => Some(Cow::Borrowed(path)),
        }
    }

    /// Points the document at a different file *and* forgets that its bytes
    /// arrived packed — what Save As does to a `.gz` (ADR-074).
    ///
    /// A rename in the explorer goes through `set_path` instead and keeps the
    /// container, because the file it renamed is still compressed. Save As is
    /// the other case: the text is about to be written out as itself, and from
    /// then on the buffer *is* the file at the new path.
    pub fn save_to(&mut self, path: PathBuf) {
        self.set_path(path);
        self.compression = Compression::None;
    }

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

    /// Changes what the file's lines will be separated by on disk (ADR-058).
    ///
    /// Nothing in the rope moves: it holds `\n` alone whatever the file used, so
    /// the ending is a property of the *write* and not of the text. The buffer
    /// is marked dirty because what would now be written differs from what is
    /// on disk — which is exactly what "unsaved changes" means, even though no
    /// character of the document changed.
    ///
    /// How the file's bytes are read and written (ADR-059).
    pub fn charset(&self) -> Charset {
        self.charset
    }

    /// Changes what the file will be written as.
    ///
    /// Like `set_line_ending`, and for the same reasons: nothing in the rope
    /// moves — the charset is a property of the *bytes* — and the buffer is
    /// marked dirty because what would be written now differs from what is on
    /// disk. It is not an undo step; the way back is the other row of the same
    /// picker. Unlike the line ending it can make a save *fail*, on a character
    /// the new charset cannot hold, which `save` finds before it opens the file.
    pub fn set_charset(&mut self, charset: Charset) {
        if self.charset == charset {
            return;
        }
        log::info!("{} will be written as {}", self.title(), charset.label);
        self.charset = charset;
        self.dirty = true;
    }

    /// It is deliberately not an undo step. History records edits to the rope,
    /// and there is none here; the way back is to choose the other ending,
    /// which is one gesture away in the same dialog.
    pub fn set_line_ending(&mut self, ending: LineEnding) {
        if self.line_ending == ending {
            return;
        }
        log::info!(
            "{} will be written with {} endings",
            self.title(),
            ending.label()
        );
        self.line_ending = ending;
        self.dirty = true;
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
    pub fn extend_cursor(&mut self, motion: Motion, layout: Layout) {
        self.anchor_here();
        self.step(motion, layout);
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

    /// Applies a motion. `layout` is the shape of the editor pane — how many
    /// rows it shows, and where it breaks a line when it wraps — which is why
    /// it is passed in rather than known here: `editor/` has no idea a terminal
    /// exists.
    ///
    /// The pane is what `Up`, `Down`, `Home` and `End` are *about* once lines
    /// wrap (ADR-057): they move by drawn row, so a paragraph reads a row at a
    /// time instead of jumping over the whole of it.
    pub fn move_cursor(&mut self, motion: Motion, layout: Layout) {
        // Moving without Shift is how a selection is dismissed, which is why
        // every plain motion goes through here and every extending one does not.
        self.clear_selection();
        self.step(motion, layout);
    }

    /// The motion itself, with no opinion about the anchor.
    fn step(&mut self, motion: Motion, layout: Layout) {
        // Moving the caret ends the open undo step (ARCHITECTURE §5): coming
        // back and typing somewhere else is a new edit, not a longer one.
        self.history.seal();
        let last_line = self.line_count().saturating_sub(1);
        match motion {
            Motion::Left => self.step_left(),
            Motion::Right => self.step_right(),
            Motion::Up => self.step_vertical(-1, 1, layout),
            Motion::Down => self.step_vertical(1, 1, layout),
            Motion::PageUp => self.step_vertical(-1, layout.height.max(1), layout),
            Motion::PageDown => self.step_vertical(1, layout.height.max(1), layout),
            // The ends of the *row*, which are the ends of the line whenever
            // the line is one row — so there is one rule here and not two.
            Motion::Home => self.cursor.column = self.cursor_row_ends(layout).0,
            Motion::End => self.cursor.column = self.cursor_row_ends(layout).1,
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

    /// The row the cursor is on, under `layout`. One row per line while the
    /// pane does not wrap, so every caller of it works either way.
    fn cursor_row(&self, layout: Layout) -> Row {
        layout
            .row_at_col(
                &self.cursor_line(),
                self.tab_width,
                self.cursor_visual_col(),
            )
            .1
    }

    /// The ends of the row the cursor is on.
    ///
    /// The ends of the *line* while the pane does not wrap, taken without
    /// laying the line out: `Home` on a four-megabyte line is one assignment,
    /// and it stays one.
    fn cursor_row_ends(&self, layout: Layout) -> (CharIdx, CharIdx) {
        if !layout.wraps() {
            return (CharIdx(0), coords::char_len(&self.cursor_line()));
        }
        let row = self.cursor_row(layout);
        (row.start, row.end)
    }

    /// Vertical movement lands on the preferred column, then snaps onto a
    /// cluster boundary so the caret can never sit inside a character.
    fn step_vertical(&mut self, direction: isize, distance: usize, layout: Layout) {
        if layout.wraps() {
            return self.step_rows(direction, distance, layout);
        }
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

    /// The same movement counted in drawn rows, for a pane that wraps.
    ///
    /// The preferred column is still a column of a *line*, so it is read
    /// through the row it falls on: on the third row of a paragraph it means
    /// "this far into a row", which is the offset that is then carried onto
    /// the row moved to. That is what makes a column of `Down` presses through
    /// a wrapped paragraph come back out where it started, exactly as it does
    /// through a ragged block of short lines.
    fn step_rows(&mut self, direction: isize, distance: usize, layout: Layout) {
        let tab_width = self.tab_width;
        let text = self.cursor_line().into_owned();
        let (row, _) = layout.row_at_col(&text, tab_width, self.cursor_visual_col());
        let preferred = layout
            .row_at_col(&text, tab_width, self.cursor.preferred_col)
            .1;
        let offset = self
            .cursor
            .preferred_col
            .0
            .saturating_sub(preferred.start_col.0);

        let (line, row) = wrap::step_rows(
            (self.cursor.line, row),
            direction * distance as isize,
            self.line_count(),
            |line| layout.row_count(&self.line(line), tab_width),
        );

        let text = self.line(line).into_owned();
        let target = layout.row_at(&text, tab_width, row).1;
        // Clamped to where the row ends: a column past it belongs to the row
        // below, and landing there would be a `Down` that moved by two.
        let col = VisualCol((target.start_col.0 + offset).min(target.end_col.0));
        self.cursor.line = line;
        self.cursor.column = coords::snap(&text, coords::char_at_visual_col(&text, col, tab_width));
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

    /// Removes the lines the caret is on, or every line a selection touches.
    ///
    /// Whole lines, terminator included, so what is below moves up instead of
    /// leaving a blank row behind. The last line of a buffer has no terminator
    /// of its own, so the one *above* it goes instead — otherwise deleting it
    /// would leave the empty line its missing newline implies.
    ///
    /// One undo step however many lines went (SPEC §15, §16), and the caret
    /// lands on the line that moved up into the gap with its column clamped to
    /// it — the same place every editor with this key puts it.
    pub fn delete_line(&mut self) {
        self.history.begin_edit(self.cursor);

        let last_line = self.line_count().saturating_sub(1);
        let (first, last) = match self.selection() {
            Some(selection) => (selection.start().line, selection.end().line.min(last_line)),
            None => (
                self.cursor.line.min(last_line),
                self.cursor.line.min(last_line),
            ),
        };

        let ends_the_buffer = last >= last_line;
        let start = self.rope.line_to_char(first);
        // Take the newline above when there is none below: a range that stopped
        // at the end of the buffer would leave it behind as an empty last line.
        let start = if ends_the_buffer && first > 0 {
            start - 1
        } else {
            start
        };
        let end = if ends_the_buffer {
            self.rope.len_chars()
        } else {
            self.rope.line_to_char(last + 1)
        };

        self.clear_selection();
        self.remove(start..end);

        self.cursor.line = first.min(self.line_count().saturating_sub(1));
        let len = coords::char_len(&self.cursor_line());
        self.cursor.column = self.cursor.column.min(len);
        self.remember_column();

        self.history.end_edit(self.cursor);
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

    /// Every occurrence of `query` in the buffer, up to `limit`, in one go.
    ///
    /// The bool is whether the limit cut the list short: `e` in a five-megabyte
    /// file is half a million hits, and a vector of them is both a memory
    /// spike and a count nobody reads. The bar says `500+` and search still
    /// works — the matches past the limit are simply not offered as
    /// destinations.
    ///
    /// Test-only since ADR-076: the bar walks the buffer a frame's worth at a
    /// time through `find_from`, and nothing in the editor wants the whole
    /// answer at the cost of the whole frame. It stays because the tests below
    /// are about *what* is found, and threading a deadline through each of them
    /// would be noise around the thing they assert.
    #[cfg(test)]
    pub fn find_all(&self, query: &str, case_sensitive: bool, limit: usize) -> (Vec<Match>, bool) {
        let mut matches = Vec::new();
        let (_, truncated) = self.find_from(query, case_sensitive, limit, 0, None, &mut matches);
        (matches, truncated)
    }

    /// The same walk, started at `from` and allowed to stop early (ADR-076).
    ///
    /// `deadline` is what makes the scan of a very large file something a frame
    /// can do a slice of: hits are appended to `into`, and the line the walk
    /// stopped at comes back so the next frame can carry on from it. `None` is
    /// a walk to the end, which is what `find_all` is.
    ///
    /// The clock is read every `CLOCK_EVERY` lines rather than on each one. At
    /// a few hundred nanoseconds a read, one per line would be a measurable
    /// share of the scan it exists to bound.
    pub fn find_from(
        &self,
        query: &str,
        case_sensitive: bool,
        limit: usize,
        from: usize,
        deadline: Option<Instant>,
        into: &mut Vec<Match>,
    ) -> (usize, bool) {
        const CLOCK_EVERY: usize = 512;
        let end = self.line_count();
        if query.is_empty() {
            return (end, false);
        }
        for line in from..end {
            for (start, end) in search::find_in_line(&self.line(line), query, case_sensitive) {
                if into.len() == limit {
                    return (line, true);
                }
                into.push(Match { line, start, end });
            }
            if (line - from) % CLOCK_EVERY == CLOCK_EVERY - 1
                && deadline.is_some_and(|deadline| Instant::now() >= deadline)
            {
                return (line + 1, false);
            }
        }
        (end, false)
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

/// Fixtures for the tests in other modules that need a document in a state the
/// filesystem would take a very large file to produce.
#[cfg(test)]
impl Document {
    /// Says the buffer holds only the beginning of its file, without unpacking
    /// the sixty-four megabytes it would take to mean it (ADR-074).
    pub fn pretend_truncated(&mut self) {
        self.compression = Compression::Gzip;
        self.truncated = true;
    }
}

/// How many bytes of a file are looked at to decide whether it is text.
///
/// A NUL in the first eight kilobytes is what every tool that has to make this
/// decision uses, and reading further buys almost nothing: a file whose first
/// 8 KB are text and whose middle is not is a file the user meant to open.
const BINARY_SNIFF: usize = 8 * 1024;

/// Turns a file's bytes into text, and says what charset it took to do it
/// (ADR-059).
///
/// `wanted` is the charset the user named, and `None` means "work it out": the
/// file's own byte order mark when it has one, UTF-8 when the bytes are valid
/// UTF-8, and `Charset::FALLBACK` otherwise. That last guess is safe where
/// guessing between two Cyrillic charsets would not be — every byte of it
/// round-trips — and the caller says so on the status bar rather than leaving
/// the user to notice.
fn decode(
    path: &Path,
    bytes: &[u8],
    wanted: Option<Charset>,
) -> Result<(String, Charset), DocumentError> {
    let sniffed = Charset::sniff(bytes);
    let charset = match (wanted, sniffed) {
        (Some(charset), _) => charset,
        (None, Some(charset)) => charset,
        (None, None) if std::str::from_utf8(bytes).is_ok() => Charset::UTF8,
        (None, None) => Charset::FALLBACK,
    };
    // The guard runs against *decoded* text and not against the file, because
    // UTF-16 text is half NUL bytes and is still text. Only the head is decoded
    // for it, so deciding that a gigabyte is binary costs eight kilobytes.
    let head = &bytes[..bytes.len().min(BINARY_SNIFF)];
    if charset.decode(head).contains('\0') {
        return Err(DocumentError::Binary {
            path: path.display().to_string(),
        });
    }
    Ok((charset.decode(bytes), charset))
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
    use std::time::Duration;

    use super::*;
    use crate::editor::coords::char_len;

    const FIXTURE: &str = include_str!("../../tests/fixtures/unicode.txt");

    /// A pane `height` rows tall that does not wrap — what almost every test
    /// here is about, because a motion over unwrapped text is a motion over
    /// lines. The wrapped cases build their own narrow layout.
    fn pane(height: usize) -> Layout {
        Layout::plain(height, 80)
    }

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
        document.move_cursor(Motion::Right, pane(10));
        assert!(!document.is_dirty(), "moving is not editing");
        document.insert_char('y');
        assert!(document.is_dirty());
    }

    #[test]
    fn newline_splits_the_line_at_the_cursor() {
        let mut document = doc("abcd");
        document.move_cursor(Motion::Right, pane(10));
        document.move_cursor(Motion::Right, pane(10));
        document.insert_newline();
        assert_eq!(text(&document), "ab\ncd");
        assert_eq!(document.cursor().line, 1);
        assert_eq!(document.cursor().column, CharIdx(0));
    }

    #[test]
    fn backspace_removes_a_whole_grapheme_cluster() {
        // e + combining acute: one press must take both chars.
        let mut document = doc("e\u{301}");
        document.move_cursor(Motion::End, pane(10));
        document.backspace();
        assert_eq!(text(&document), "");
    }

    #[test]
    fn backspace_at_the_start_of_a_line_joins_it_to_the_previous_one() {
        let mut document = doc("one\ntwo");
        document.move_cursor(Motion::Down, pane(10));
        document.move_cursor(Motion::Home, pane(10));
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
        document.move_cursor(Motion::End, pane(10));
        document.delete();
        assert_eq!(text(&document), "onetwo");
    }

    #[test]
    fn delete_at_the_end_of_the_document_does_nothing() {
        let mut document = doc("abc");
        document.move_cursor(Motion::DocumentEnd, pane(10));
        document.delete();
        assert_eq!(text(&document), "abc");
        assert!(!document.is_dirty());
    }

    #[test]
    fn delete_line_takes_the_whole_line_and_its_terminator() {
        let mut document = doc("one\ntwo\nthree");
        document.move_cursor(Motion::Down, pane(10));
        document.delete_line();
        assert_eq!(text(&document), "one\nthree");
        assert_eq!(document.cursor().line, 1, "the line below moved up into it");
    }

    #[test]
    fn delete_line_on_the_last_line_takes_the_newline_above_it() {
        let mut document = doc("one\ntwo");
        document.move_cursor(Motion::DocumentEnd, pane(10));
        document.delete_line();
        assert_eq!(
            text(&document),
            "one",
            "no empty last line is left where the terminator was"
        );
        assert_eq!(document.line_count(), 1);
        assert_eq!(document.cursor().line, 0);
    }

    #[test]
    fn delete_line_on_the_only_line_empties_it() {
        let mut document = doc("alone");
        document.delete_line();
        assert_eq!(text(&document), "");
        assert_eq!(document.line_count(), 1);
        assert_eq!(document.cursor().column.0, 0);
    }

    #[test]
    fn delete_line_on_an_empty_document_changes_nothing() {
        let mut document = doc("");
        document.delete_line();
        assert_eq!(text(&document), "");
        assert!(!document.is_dirty());
    }

    #[test]
    fn delete_line_takes_every_line_a_selection_touches() {
        let mut document = doc("one\ntwo\nthree\nfour");
        document.move_cursor(Motion::Down, pane(10));
        // Anchored mid-line and ending mid-line: partial lines go whole.
        document.move_cursor(Motion::Right, pane(10));
        document.extend_cursor(Motion::Down, pane(10));
        document.delete_line();
        assert_eq!(text(&document), "one\nfour");
        assert_eq!(document.cursor().line, 1);
        assert!(document.selection().is_none());
    }

    #[test]
    fn delete_line_clamps_the_column_to_the_line_that_moves_up() {
        let mut document = doc("one\nlonger line\nab");
        document.move_cursor(Motion::Down, pane(10));
        document.move_cursor(Motion::End, pane(10));
        document.delete_line();
        assert_eq!(text(&document), "one\nab");
        assert_eq!((document.cursor().line, document.cursor().column.0), (1, 2));
    }

    #[test]
    fn delete_line_is_one_undo_step() {
        let mut document = doc("one\ntwo\nthree");
        document.move_cursor(Motion::Down, pane(10));
        document.delete_line();
        assert!(document.undo());
        assert_eq!(text(&document), "one\ntwo\nthree");
    }

    #[test]
    fn left_and_right_cross_line_boundaries() {
        let mut document = doc("ab\ncd");
        document.move_cursor(Motion::End, pane(10));
        document.move_cursor(Motion::Right, pane(10));
        assert_eq!((document.cursor().line, document.cursor().column.0), (1, 0));
        document.move_cursor(Motion::Left, pane(10));
        assert_eq!((document.cursor().line, document.cursor().column.0), (0, 2));
    }

    #[test]
    fn vertical_movement_keeps_the_preferred_column_over_a_short_line() {
        let mut document = doc("abcdef\n\nabcdef");
        document.move_cursor(Motion::End, pane(10));
        assert_eq!(document.cursor_visual_col(), VisualCol(6));
        document.move_cursor(Motion::Down, pane(10));
        assert_eq!(document.cursor().column, CharIdx(0), "the line is empty");
        document.move_cursor(Motion::Down, pane(10));
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
        document.move_cursor(Motion::Right, pane(10));
        document.move_cursor(Motion::Down, pane(10));
        assert_eq!(document.cursor().column, CharIdx(0));
        assert_eq!(document.cursor_visual_col(), VisualCol(0));
    }

    /// A pane ten columns wide, which is what the wrapped cases are about.
    fn narrow(height: usize) -> Layout {
        Layout::wrapping(height, 10)
    }

    #[test]
    fn down_moves_by_one_drawn_row_inside_a_wrapped_line() {
        // Three rows: "aaaa bbbb ", "cccc dddd ", "eeee".
        let mut document = doc("aaaa bbbb cccc dddd eeee
next
");
        document.move_cursor(Motion::Down, narrow(10));
        assert_eq!(document.cursor().line, 0, "still the same line");
        assert_eq!(document.cursor().column, CharIdx(10), "the second row");
        document.move_cursor(Motion::Down, narrow(10));
        assert_eq!(document.cursor().column, CharIdx(20));
        document.move_cursor(Motion::Down, narrow(10));
        assert_eq!(document.cursor().line, 1, "off the end of the wrapped line");
    }

    #[test]
    fn up_walks_back_into_the_rows_of_the_line_above() {
        let mut document = doc("aaaa bbbb cccc dddd eeee
next
");
        document.place_cursor(1, VisualCol(2));
        document.move_cursor(Motion::Up, narrow(10));
        assert_eq!(document.cursor().line, 0);
        assert_eq!(
            document.cursor().column,
            CharIdx(22),
            "the last row of the line above, two columns in"
        );
    }

    #[test]
    fn the_preferred_column_is_kept_across_a_short_row() {
        // Row one is full, row two is short, row three is full again: a column
        // of Down presses has to come back out where it started.
        let mut document = doc("aaaaaaaaa
bb
cccccccccc
");
        document.place_cursor(0, VisualCol(7));
        document.move_cursor(Motion::Down, narrow(10));
        assert_eq!(document.cursor_visual_col(), VisualCol(2), "the short line");
        document.move_cursor(Motion::Down, narrow(10));
        assert_eq!(document.cursor_visual_col(), VisualCol(7));
    }

    #[test]
    fn home_and_end_are_the_ends_of_the_drawn_row_when_lines_wrap() {
        let mut document = doc("aaaa bbbb cccc dddd
");
        document.place_cursor(0, VisualCol(12));
        document.move_cursor(Motion::Home, narrow(10));
        assert_eq!(document.cursor().column, CharIdx(10), "the row's start");
        document.move_cursor(Motion::End, narrow(10));
        assert_eq!(document.cursor().column, CharIdx(19), "and its end");
    }

    #[test]
    fn home_and_end_are_still_the_line_when_nothing_wraps() {
        let mut document = doc("aaaa bbbb cccc dddd
");
        document.place_cursor(0, VisualCol(12));
        document.move_cursor(Motion::Home, pane(10));
        assert_eq!(document.cursor().column, CharIdx(0));
        document.move_cursor(Motion::End, pane(10));
        assert_eq!(document.cursor().column, CharIdx(19));
    }

    #[test]
    fn a_page_is_counted_in_drawn_rows_when_lines_wrap() {
        let mut document = doc("aaaa bbbb cccc dddd eeee ffff
next
");
        // Four rows of the paragraph; a page of two lands on the third.
        document.move_cursor(Motion::PageDown, Layout::wrapping(2, 10));
        assert_eq!(document.cursor().line, 0);
        assert_eq!(document.cursor().column, CharIdx(20));
    }

    #[test]
    fn page_movement_stops_at_the_ends_of_the_document() {
        let mut document = doc("1\n2\n3\n4\n5");
        document.move_cursor(Motion::PageDown, pane(3));
        assert_eq!(document.cursor().line, 3);
        document.move_cursor(Motion::PageDown, pane(3));
        assert_eq!(document.cursor().line, 4);
        document.move_cursor(Motion::PageUp, pane(100));
        assert_eq!(document.cursor().line, 0);
    }

    #[test]
    fn word_motion_crosses_lines_at_their_ends() {
        let mut document = doc("let x\nlet y");
        document.move_cursor(Motion::End, pane(10));
        document.move_cursor(Motion::WordRight, pane(10));
        assert_eq!((document.cursor().line, document.cursor().column.0), (1, 0));
        document.move_cursor(Motion::WordLeft, pane(10));
        assert_eq!((document.cursor().line, document.cursor().column.0), (0, 5));
    }

    #[test]
    fn document_start_and_end_reach_both_ends() {
        let mut document = doc("one\ntwo\nthree");
        document.move_cursor(Motion::DocumentEnd, pane(10));
        assert_eq!((document.cursor().line, document.cursor().column.0), (2, 5));
        document.move_cursor(Motion::DocumentStart, pane(10));
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
        document.move_cursor(Motion::DocumentEnd, pane(10));
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
        document.extend_cursor(Motion::WordRight, pane(10));
        let selection = document.selection().expect("Shift+Ctrl+Right selects");
        assert_eq!(selection.start(), Position::new(0, CharIdx(0)));
        assert_eq!(document.selected_text().as_deref(), Some("hello "));
        document.move_cursor(Motion::Left, pane(10));
        assert!(document.selection().is_none(), "moving dismisses it");
    }

    #[test]
    fn a_selection_can_be_extended_backwards() {
        let mut document = doc("hello");
        document.move_cursor(Motion::End, pane(10));
        document.extend_cursor(Motion::Left, pane(10));
        document.extend_cursor(Motion::Left, pane(10));
        assert_eq!(document.selected_text().as_deref(), Some("lo"));
        assert_eq!(document.cursor().column, CharIdx(3), "the head moved");
    }

    #[test]
    fn extending_across_lines_takes_the_newlines_with_it() {
        let mut document = doc("one\ntwo\nthree");
        document.extend_cursor(Motion::Down, pane(10));
        document.extend_cursor(Motion::End, pane(10));
        assert_eq!(document.selected_text().as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn extending_over_a_cluster_never_splits_it() {
        let mut document = doc("é👨‍👩‍👧x");
        document.extend_cursor(Motion::Right, pane(10));
        assert_eq!(document.selected_text().as_deref(), Some("é"));
        document.extend_cursor(Motion::Right, pane(10));
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
        document.extend_cursor(Motion::WordRight, pane(10));
        document.insert_char('!');
        assert_eq!(text(&document), "!world");
        assert!(document.selection().is_none());
        assert_eq!(document.cursor().column, CharIdx(1));
    }

    #[test]
    fn backspace_and_delete_take_the_selection_rather_than_one_character() {
        let mut document = doc("hello world");
        document.extend_cursor(Motion::WordRight, pane(10));
        document.backspace();
        assert_eq!(text(&document), "world");

        let mut document = doc("hello world");
        document.extend_cursor(Motion::WordRight, pane(10));
        document.delete();
        assert_eq!(text(&document), "world");
    }

    #[test]
    fn deleting_a_selection_that_spans_lines_joins_what_is_left() {
        let mut document = doc("one\ntwo\nthree");
        document.move_cursor(Motion::Right, pane(10));
        document.extend_cursor(Motion::Down, pane(10));
        document.extend_cursor(Motion::Down, pane(10));
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
        document.move_cursor(Motion::Right, pane(10));
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
        document.extend_cursor(Motion::WordRight, pane(10));
        document.save().unwrap();
        assert_eq!(document.selected_text().as_deref(), Some("one "));
    }

    /// The bytes of `text` as a `.gz`, for the tests below.
    fn gzipped(text: &str) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn a_gzipped_file_opens_on_its_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql.gz");
        std::fs::write(&path, gzipped("select 1;\nselect 2;\n")).unwrap();

        let document = Document::open(&path).unwrap();
        assert_eq!(document.line(0), "select 1;");
        assert_eq!(document.line(1), "select 2;");
        assert_eq!(document.compression(), Compression::Gzip);
        assert!(document.is_read_only());
        assert!(!document.is_truncated());
    }

    /// The container says nothing about the encoding inside it, so the charset
    /// is sniffed from the unpacked bytes exactly as a plain file's are
    /// (ADR-059, ADR-074).
    #[test]
    fn a_gzipped_file_is_decoded_by_its_own_charset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom.txt.gz");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("Привіт".as_bytes());
        let text = String::from_utf8_lossy(&bytes).into_owned();
        std::fs::write(&path, gzipped(&text)).unwrap();

        let document = Document::open(&path).unwrap();
        assert_eq!(document.line(0), "Привіт");
        assert_eq!(document.charset().label, "UTF-8 with BOM");
    }

    /// A `.tar.gz` unpacks perfectly well and is still not a text file: the NUL
    /// guard runs on what came out, so it is refused like any other binary.
    #[test]
    fn a_gzipped_binary_is_still_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.tar.gz");
        std::fs::write(&path, gzipped("head\0\0tail")).unwrap();

        assert!(matches!(
            Document::open(&path).unwrap_err(),
            DocumentError::Binary { .. }
        ));
    }

    #[test]
    fn a_gzipped_file_refuses_to_be_saved_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt.gz");
        let packed = gzipped("one\n");
        std::fs::write(&path, &packed).unwrap();

        let mut document = Document::open(&path).unwrap();
        assert!(matches!(
            document.save().unwrap_err(),
            DocumentError::ReadOnly { .. }
        ));
        // Refused before anything was written, so the file is byte for byte
        // what it was.
        assert_eq!(std::fs::read(&path).unwrap(), packed);
    }

    /// Save As is the way out: the text is written as itself, and the buffer
    /// stops being a reading of a packed file (ADR-074).
    #[test]
    fn save_as_writes_the_text_out_and_the_buffer_is_a_plain_file_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let packed = dir.path().join("dump.sql.gz");
        std::fs::write(&packed, gzipped("select 1;\n")).unwrap();
        let plain = dir.path().join("dump.sql");

        let mut document = Document::open(&packed).unwrap();
        document.save_to(plain.clone());
        document.save().unwrap();

        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "select 1;\n");
        assert!(!document.is_read_only());
        assert_eq!(document.compression(), Compression::None);
    }

    /// A rename is not a Save As: the file it renamed is still compressed, so
    /// the buffer over it is still a reading of one.
    #[test]
    fn a_rename_keeps_the_buffer_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql.gz");
        std::fs::write(&path, gzipped("select 1;\n")).unwrap();

        let mut document = Document::open(&path).unwrap();
        document.set_path(dir.path().join("backup.sql.gz"));
        assert!(document.is_read_only());
    }

    /// `F5` over a `.gz` unpacks again rather than reading the stream as text.
    #[test]
    fn reloading_a_gzipped_file_unpacks_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt.gz");
        std::fs::write(&path, gzipped("before\n")).unwrap();

        let mut document = Document::open(&path).unwrap();
        std::fs::write(&path, gzipped("after\n")).unwrap();
        document.reload().unwrap();

        assert_eq!(document.line(0), "after");
        assert!(document.is_read_only());
    }

    /// A file that stops being compressed between two reads stops being
    /// read-only with it: the container is a fact about the bytes, and the
    /// bytes are what a reload re-reads.
    #[test]
    fn a_reload_that_finds_plain_text_drops_the_container() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt.gz");
        std::fs::write(&path, gzipped("packed\n")).unwrap();

        let mut document = Document::open(&path).unwrap();
        std::fs::write(&path, "plain\n").unwrap();
        document.reload().unwrap();

        assert_eq!(document.line(0), "plain");
        assert!(!document.is_read_only());
    }

    /// The grammar comes from the name inside the container, so SQL is
    /// highlighted as SQL (SPEC §21, ADR-074).
    #[test]
    fn the_grammar_is_chosen_by_the_name_inside_the_container() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql.gz");
        std::fs::write(&path, gzipped("select 1;\n")).unwrap();

        let document = Document::open(&path).unwrap();
        assert_eq!(
            document.syntax_path().unwrap().file_name().unwrap(),
            "dump.sql"
        );
        // A plain file's is the path itself, `.gz` or not.
        let plain = Document::from_text("", Some(dir.path().join("notes.gz")));
        assert_eq!(
            plain.syntax_path().unwrap().file_name().unwrap(),
            "notes.gz"
        );
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

    /// A file with no mark and bytes that are not UTF-8 opens rather than being
    /// refused (ADR-059): every byte of the fallback round-trips, so the user
    /// can look at it and name the charset it really is.
    #[test]
    fn a_file_that_is_not_utf8_opens_in_the_fallback_charset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin1.txt");
        // `Café` in Windows-1252, which is not valid UTF-8.
        std::fs::write(&path, [b'C', b'a', b'f', 0xE9]).unwrap();

        let document = Document::open(&path).unwrap();
        assert_eq!(document.charset(), Charset::FALLBACK);
        assert_eq!(document.line(0), "Café");
    }

    /// The one thing no text file has. Without the guard a JPEG would open as
    /// a megabyte of line noise, since every legacy charset decodes every byte.
    #[test]
    fn open_refuses_a_binary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("thing.bin");
        std::fs::write(&path, [0x89, b'P', b'N', b'G', 0x00, 0x1a]).unwrap();
        assert!(matches!(
            Document::open(&path).unwrap_err(),
            DocumentError::Binary { .. }
        ));
    }

    /// A mark is what a file is recognised by, and it is not part of the text —
    /// but it is written back, so the file is still the file it was.
    #[test]
    fn a_marked_file_round_trips_its_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("win.txt");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("hi\n".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&path, &bytes).unwrap();

        let mut document = Document::open(&path).unwrap();
        assert_eq!(document.charset().label, "UTF-16 LE");
        assert_eq!(document.line(0), "hi");
        document.save().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    /// A save that would write `&#1071;` where the user typed `Я` is corruption,
    /// so it is refused — and refused before the file is opened, so what is on
    /// disk is still what was there.
    #[test]
    fn saving_a_character_the_charset_cannot_hold_is_refused_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latin.txt");
        std::fs::write(&path, "hello\n").unwrap();

        let mut document = Document::open(&path).unwrap();
        document.set_charset(Charset::by_label("Windows-1252").unwrap());
        document.insert_text("Я");
        let err = document.save().unwrap_err();
        assert!(
            matches!(
                err,
                DocumentError::Unmappable {
                    character: 'Я', ..
                }
            ),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    }

    /// Reopening is how a file the fallback guessed wrong about is put right,
    /// and it is undoable — it is the command whose point is to be tried again.
    #[test]
    fn reopening_with_a_charset_decodes_the_same_bytes_differently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koi8.txt");
        let charset = Charset::by_label("KOI8-U").unwrap();
        let mut bytes = Vec::new();
        charset.encode("Привіт\n", &mut bytes);
        std::fs::write(&path, &bytes).unwrap();

        let mut document = Document::open(&path).unwrap();
        assert_eq!(document.charset(), Charset::FALLBACK, "guessed, and wrong");
        assert_ne!(document.line(0), "Привіт");

        document.reopen_as(charset).unwrap();
        assert_eq!(document.line(0), "Привіт");
        assert!(!document.is_dirty());

        document.undo();
        assert_ne!(document.line(0), "Привіт", "the reopen is undoable");
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
        document.move_cursor(Motion::End, pane(10));
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
        document.move_cursor(Motion::Down, pane(10));
        document.move_cursor(Motion::End, pane(10));
        type_text(&mut document, "!");
        document.undo();
        assert_eq!(document.cursor().line, 1);
        assert_eq!(document.cursor().column, CharIdx(3));
    }

    #[test]
    fn moving_the_caret_ends_the_undo_step() {
        let mut document = doc("");
        type_text(&mut document, "ab");
        document.move_cursor(Motion::Left, pane(10));
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
        document.move_cursor(Motion::End, pane(10));
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
        document.move_cursor(Motion::End, pane(10));
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
        document.extend_cursor(Motion::Down, pane(10));
        document.extend_cursor(Motion::Down, pane(10));
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
        document.move_cursor(Motion::DocumentEnd, pane(10));
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

    /// The walk stops when the clock runs out and says where to pick up, and
    /// picking up from there finds exactly what one pass would have (ADR-076).
    #[test]
    fn a_walk_cut_short_resumes_where_it_stopped() {
        let text = "needle\n".repeat(4_000);
        let document = Document::from_text(&text, None);

        let mut cut = Vec::new();
        // A deadline already in the past, so the walk stops at the first check.
        let (resume, truncated) = document.find_from(
            "needle",
            true,
            1_000_000,
            0,
            Some(Instant::now() - Duration::from_millis(1)),
            &mut cut,
        );
        assert!(!truncated, "it ran out of time, not out of room");
        assert!(resume > 0 && resume < document.line_count(), "at {resume}");

        let (end, _) = document.find_from("needle", true, 1_000_000, resume, None, &mut cut);
        assert_eq!(end, document.line_count());

        let (whole, _) = document.find_all("needle", true, 1_000_000);
        assert_eq!(cut, whole, "two passes find what one pass finds");
    }

    /// A walk started past the end of the buffer lands rather than looping.
    #[test]
    fn a_walk_starting_past_the_end_is_over() {
        let document = Document::from_text("one\ntwo\n", None);
        let mut into = Vec::new();
        let (end, truncated) = document.find_from(
            "o",
            true,
            10,
            document.line_count(),
            Some(Instant::now()),
            &mut into,
        );
        assert_eq!(end, document.line_count());
        assert!(!truncated);
        assert!(into.is_empty());
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
        document.move_cursor(Motion::Right, pane(10));
        assert_eq!(document.revision(), start, "moving is not a change");
        document.insert_char('b');
        assert!(document.revision() > start);
        let typed = document.revision();
        document.undo();
        assert!(document.revision() > typed, "undo changes the buffer too");
    }

    // --- what is on disk (ADR-043) -----------------------------------------

    /// A stamp is mtime *and* length, and a test that rewrites a file inside
    /// one timestamp tick would be comparing lengths alone. Every fixture here
    /// changes the length too, which is what a real edit does.
    fn on_disk(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn a_buffer_with_no_file_behind_it_makes_no_claim_about_disk() {
        assert_eq!(doc("hello").disk_state(), DiskState::Untracked);
    }

    #[test]
    fn a_file_nobody_has_touched_reads_as_the_one_that_was_opened() {
        let (_dir, path) = on_disk("one\ntwo\n");
        let document = Document::open(&path).unwrap();
        assert_eq!(document.disk_state(), DiskState::Same);
    }

    #[test]
    fn a_file_written_by_somebody_else_reads_as_changed() {
        let (_dir, path) = on_disk("one\ntwo\n");
        let document = Document::open(&path).unwrap();
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(document.disk_state(), DiskState::Changed);
    }

    #[test]
    fn a_file_that_was_removed_reads_as_gone() {
        let (_dir, path) = on_disk("one\n");
        let document = Document::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(document.disk_state(), DiskState::Gone);
    }

    #[test]
    fn saving_makes_the_buffer_what_the_file_is_again() {
        let (_dir, path) = on_disk("one\n");
        let mut document = Document::open(&path).unwrap();
        document.insert_text("longer text\n");
        assert_eq!(document.disk_state(), DiskState::Same, "nobody else wrote");
        document.save().unwrap();
        assert_eq!(
            document.disk_state(),
            DiskState::Same,
            "the editor's own write is not somebody else's"
        );
    }

    #[test]
    fn a_reload_takes_the_file_and_settles_the_comparison() {
        let (_dir, path) = on_disk("one\ntwo\n");
        let mut document = Document::open(&path).unwrap();
        std::fs::write(&path, "rewritten by another editor\n").unwrap();
        assert_eq!(document.disk_state(), DiskState::Changed);

        document.reload().unwrap();
        assert_eq!(text(&document), "rewritten by another editor\n");
        assert_eq!(document.disk_state(), DiskState::Same);
        assert!(!document.is_dirty(), "the buffer is the file");
    }

    #[test]
    fn a_reload_is_one_undo_step_that_gives_the_edits_back() {
        let (_dir, path) = on_disk("one\n");
        let mut document = Document::open(&path).unwrap();
        document.insert_text("mine and only mine\n");
        let mine = text(&document);
        std::fs::write(&path, "theirs\n").unwrap();

        document.reload().unwrap();
        assert_eq!(text(&document), "theirs\n");

        assert!(document.undo(), "a reload is undoable");
        assert_eq!(text(&document), mine, "the unsaved work comes back");
        assert!(document.is_dirty(), "and it is unsaved again");
        assert!(document.redo());
        assert_eq!(text(&document), "theirs\n");
    }

    #[test]
    fn a_reload_of_a_shorter_file_keeps_the_caret_inside_it() {
        let (_dir, path) = on_disk("one\ntwo\nthree\nfour\n");
        let mut document = Document::open(&path).unwrap();
        document.goto_line(4);
        document.move_cursor(Motion::End, pane(10));
        std::fs::write(&path, "1\n").unwrap();

        document.reload().unwrap();
        let cursor = document.cursor_position();
        assert!(cursor.line < document.line_count(), "{cursor:?}");
        assert!(cursor.column <= char_len(&document.line(cursor.line)));
    }

    #[test]
    fn a_reload_follows_the_line_ending_the_file_now_has() {
        let (_dir, path) = on_disk("one\ntwo\n");
        let mut document = Document::open(&path).unwrap();
        assert_eq!(document.line_ending(), LineEnding::Lf);

        std::fs::write(&path, "one\r\ntwo\r\nthree\r\n").unwrap();
        document.reload().unwrap();
        assert_eq!(document.line_ending(), LineEnding::Crlf);
        assert!(
            !text(&document).contains('\r'),
            "and no carriage return reaches the buffer"
        );
    }

    #[test]
    fn a_reload_of_a_file_that_is_gone_leaves_the_buffer_alone() {
        let (_dir, path) = on_disk("one\n");
        let mut document = Document::open(&path).unwrap();
        document.insert_text("mine\n");
        let mine = text(&document);
        std::fs::remove_file(&path).unwrap();

        assert!(document.reload().is_err());
        assert_eq!(text(&document), mine, "nothing was spent on the failure");
        assert!(document.is_dirty());
    }

    #[test]
    fn a_reload_of_a_buffer_with_no_file_is_an_error_and_not_a_panic() {
        assert!(matches!(
            Document::from_text("x", None).reload(),
            Err(DocumentError::NoPath)
        ));
    }
}
