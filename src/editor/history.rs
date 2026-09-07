//! Undo/redo transactions with coalescing.
//!
//! Memory is O(edited bytes) and never O(document): a transaction holds the
//! text an operation put in or took out, never a snapshot of the buffer
//! (SPEC §16, ARCHITECTURE §5). Typing a word is one transaction whose single
//! `Insert` grows by a character at a time, so a hundred thousand keystrokes
//! cost the hundred thousand bytes that were typed plus one small header per
//! word — not a hundred thousand copies of the file.
//!
//! This module knows nothing about a rope. It records what happened and hands
//! transactions back; `Document` is what applies and inverts them.

use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;

use crate::editor::coords::CharIdx;
use crate::editor::cursor::Cursor;

/// How long an open transaction keeps absorbing keystrokes (ARCHITECTURE §5).
///
/// Long enough that ordinary typing is one step per word, short enough that
/// coming back to the keyboard after a pause starts a new one.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(500);

/// One rope mutation, kept together with the text that inverts it.
///
/// `at` is a document-wide `char` offset — the same unit `ropey` is indexed in,
/// so applying an operation is O(log n) and needs no line lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOperation {
    Insert {
        at: CharIdx,
        text: String,
    },
    /// `text` is the removed text: without it a delete could not be undone.
    Delete {
        at: CharIdx,
        text: String,
    },
}

impl EditOperation {
    pub fn text(&self) -> &str {
        match self {
            Self::Insert { text, .. } | Self::Delete { text, .. } => text,
        }
    }

    /// How many `char`s the operation covers — the length of the range it
    /// occupies in the rope once applied.
    pub fn len_chars(&self) -> usize {
        self.text().chars().count()
    }

    #[cfg(test)]
    fn heap_bytes(&self) -> usize {
        match self {
            Self::Insert { text, .. } | Self::Delete { text, .. } => text.capacity(),
        }
    }
}

/// One undo step: everything a single user action did, and where the caret was
/// on either side of it.
///
/// The ops are stored in the order they were applied, so undoing walks them
/// backwards and redoing walks them forwards. More than one op appears when a
/// single action is genuinely two mutations — typing over a selection is a
/// remove followed by an insert, and must undo as one.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub ops: Vec<EditOperation>,
    pub cursor_before: Cursor,
    pub cursor_after: Cursor,
    /// When the last operation joined it. Coalescing measures from here, not
    /// from when the transaction opened, so a steady typist is never split.
    pub at: Instant,
}

impl Transaction {
    #[cfg(test)]
    fn heap_bytes(&self) -> usize {
        self.ops.capacity() * std::mem::size_of::<EditOperation>()
            + self
                .ops
                .iter()
                .map(EditOperation::heap_bytes)
                .sum::<usize>()
    }
}

/// The undo and redo stacks, plus the bookkeeping that decides where one undo
/// step ends and the next begins.
#[derive(Debug)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// Whether the transaction on top of the undo stack may still absorb an
    /// operation. A cursor move, a save, or an operation that cannot coalesce
    /// puts this back to `false`.
    open: bool,
    /// Nesting depth of the logical edit being recorded. `Document` opens one
    /// per public editing method, and those methods call each other — an insert
    /// removes the selection first — so this counts rather than flags.
    depth: usize,
    /// Whether an operation has already been recorded inside the current
    /// logical edit. Everything after the first one is forced into the same
    /// transaction: that is what makes replacing a selection one undo step.
    grouped: bool,
    /// Where the caret was when the current logical edit started.
    pending_before: Cursor,
    /// `undo.len()` as of the last save, when that state is still reachable.
    /// `None` means the file on disk matches no state on the stack.
    saved: Option<usize>,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// A history for a document that matches what is on disk — the state a
    /// freshly opened file is in, and the one undoing back to clears the dirty
    /// marker.
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            open: false,
            depth: 0,
            grouped: false,
            pending_before: Cursor::default(),
            saved: Some(0),
        }
    }

    // --- recording ---------------------------------------------------------

    /// Opens a logical edit. `cursor` is where the caret was before it, which
    /// is where undoing the resulting transaction will put it back.
    pub fn begin_edit(&mut self, cursor: Cursor) {
        if self.depth == 0 {
            self.pending_before = cursor;
            self.grouped = false;
        }
        self.depth += 1;
    }

    /// Closes a logical edit and records where it left the caret.
    pub fn end_edit(&mut self, cursor: Cursor) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth > 0 || !self.grouped {
            return;
        }
        if let Some(last) = self.undo.last_mut() {
            last.cursor_after = cursor;
        }
    }

    /// Records an operation that has just been applied to the buffer.
    pub fn record(&mut self, op: EditOperation) {
        self.record_at(op, Instant::now());
    }

    /// `record` with the clock passed in, so coalescing can be tested without
    /// sleeping.
    pub fn record_at(&mut self, op: EditOperation, now: Instant) {
        // Editing after an undo abandons the redo branch. If the save point
        // lived on that branch it is no longer reachable, and keeping the index
        // would make an unrelated future state look clean.
        self.redo.clear();
        if self.saved.is_some_and(|at| at > self.undo.len()) {
            self.saved = None;
        }

        // A single grapheme cluster is what a keystroke produces; anything
        // larger is a paste or a selection, which is one step of its own.
        let single = op.text().graphemes(true).count() == 1;
        if self.grouped && !self.undo.is_empty() {
            self.append(op, now);
        } else if self.open && single && self.can_merge(&op, now) {
            self.merge(op, now);
        } else {
            self.undo.push(Transaction {
                ops: vec![op],
                cursor_before: self.pending_before,
                cursor_after: self.pending_before,
                at: now,
            });
        }
        self.open = single;
        self.grouped = self.depth > 0;
    }

    /// Adds an operation to the open transaction — the forced case, inside one
    /// logical edit.
    fn append(&mut self, op: EditOperation, now: Instant) {
        let Some(last) = self.undo.last_mut() else {
            return;
        };
        last.at = now;
        match last.ops.last_mut() {
            Some(prev) if mergeable(prev, &op) => merge_into(prev, op),
            _ => last.ops.push(op),
        }
    }

    fn can_merge(&self, op: &EditOperation, now: Instant) -> bool {
        let Some(last) = self.undo.last() else {
            return false;
        };
        if now.duration_since(last.at) > COALESCE_WINDOW {
            return false;
        }
        last.ops.last().is_some_and(|prev| mergeable(prev, op))
    }

    fn merge(&mut self, op: EditOperation, now: Instant) {
        let Some(last) = self.undo.last_mut() else {
            return;
        };
        last.at = now;
        if let Some(prev) = last.ops.last_mut() {
            merge_into(prev, op);
        }
    }

    /// Ends the open transaction, so the next operation starts a new undo step.
    /// A cursor jump, a selection change and a save all do this.
    pub fn seal(&mut self) {
        self.open = false;
    }

    // --- undo / redo -------------------------------------------------------

    /// Takes the newest transaction off the undo stack. The caller inverts it
    /// and hands it back with `push_redo` — moved rather than cloned, because a
    /// megabyte paste must not be copied to undo it.
    pub fn take_undo(&mut self) -> Option<Transaction> {
        self.seal();
        self.undo.pop()
    }

    pub fn push_redo(&mut self, transaction: Transaction) {
        self.redo.push(transaction);
    }

    pub fn take_redo(&mut self) -> Option<Transaction> {
        self.seal();
        self.redo.pop()
    }

    /// Puts a redone transaction back on the undo stack, sealed: a keystroke
    /// after a redo is a new step, not an extension of the redone one.
    pub fn push_undo(&mut self, transaction: Transaction) {
        self.undo.push(transaction);
        self.open = false;
    }

    // --- save point --------------------------------------------------------

    /// Remembers that the buffer as it stands now is what is on disk.
    pub fn mark_saved(&mut self) {
        self.seal();
        self.saved = Some(self.undo.len());
    }

    /// Whether the buffer matches the last saved state — undoing back to it
    /// clears the dirty marker rather than leaving a file that differs from
    /// disk by nothing still flagged as modified.
    pub fn at_saved_point(&self) -> bool {
        self.saved == Some(self.undo.len())
    }

    // --- diagnostics -------------------------------------------------------
    //
    // Test-only for now. Phase 9 greys out the Edit menu's entries with
    // `can_redo` and its counterpart, which is when they leave `cfg(test)`.

    #[cfg(test)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[cfg(test)]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Bytes held by both stacks. Exists for the memory test that guards the
    /// O(edited bytes) promise; nothing in the UI reads it.
    #[cfg(test)]
    pub fn memory_bytes(&self) -> usize {
        let stack = |transactions: &[Transaction]| {
            std::mem::size_of_val(transactions)
                + transactions
                    .iter()
                    .map(Transaction::heap_bytes)
                    .sum::<usize>()
        };
        stack(&self.undo) + stack(&self.redo)
    }
}

/// What a character counts as when deciding whether typing it continues the
/// word already being typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Space,
    Punct,
    /// Its own class, and never equal to itself: a newline always ends a step,
    /// so Enter is one undo and the line typed after it is another.
    Newline,
}

fn class_of(ch: char) -> CharClass {
    if ch == '\n' {
        CharClass::Newline
    } else if ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else if ch.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Punct
    }
}

/// Whether two characters belong to the same run — the "does not cross a word
/// or newline boundary" half of the coalescing rule.
fn same_run(edge: Option<char>, text: &str) -> bool {
    let (Some(edge), Some(next)) = (edge, text.chars().next()) else {
        return false;
    };
    let class = class_of(edge);
    class != CharClass::Newline && class == class_of(next)
}

/// Whether `next` continues `prev` rather than starting a new undo step:
/// same kind, adjacent in the buffer, and inside the same run of characters.
fn mergeable(prev: &EditOperation, next: &EditOperation) -> bool {
    match (prev, next) {
        (EditOperation::Insert { at, text }, EditOperation::Insert { at: next_at, .. }) => {
            at.0 + text.chars().count() == next_at.0
                && same_run(text.chars().next_back(), next.text())
        }
        (EditOperation::Delete { at, text }, EditOperation::Delete { at: next_at, .. }) => {
            if next_at.0 + next.len_chars() == at.0 {
                // Backspace: the new deletion ends where the previous began.
                same_run(text.chars().next(), next.text())
            } else if next_at.0 == at.0 {
                // Delete: the text after the caret keeps coming to it.
                same_run(text.chars().next_back(), next.text())
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Folds `next` into `prev`. Only ever called after `mergeable` said yes.
fn merge_into(prev: &mut EditOperation, next: EditOperation) {
    match (prev, next) {
        (EditOperation::Insert { text, .. }, EditOperation::Insert { text: added, .. }) => {
            text.push_str(&added)
        }
        (
            EditOperation::Delete { at, text },
            EditOperation::Delete {
                at: next_at,
                text: removed,
            },
        ) => {
            if next_at.0 < at.0 {
                *at = next_at;
                text.insert_str(0, &removed);
            } else {
                text.push_str(&removed);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(at: usize, text: &str) -> EditOperation {
        EditOperation::Insert {
            at: CharIdx(at),
            text: text.to_string(),
        }
    }

    fn delete(at: usize, text: &str) -> EditOperation {
        EditOperation::Delete {
            at: CharIdx(at),
            text: text.to_string(),
        }
    }

    /// Records a run of single-character inserts as if typed at one position
    /// after another, all within the coalescing window.
    fn type_text(history: &mut History, start: usize, text: &str) {
        let now = Instant::now();
        for (offset, ch) in text.chars().enumerate() {
            history.begin_edit(Cursor::default());
            history.record_at(insert(start + offset, &ch.to_string()), now);
            history.end_edit(Cursor::default());
        }
    }

    #[test]
    fn a_typed_word_is_one_transaction() {
        let mut history = History::new();
        type_text(&mut history, 0, "hello");
        assert_eq!(history.undo_depth(), 1);
        assert_eq!(history.undo[0].ops.len(), 1);
        assert_eq!(history.undo[0].ops[0].text(), "hello");
    }

    #[test]
    fn a_space_starts_a_new_transaction() {
        let mut history = History::new();
        type_text(&mut history, 0, "hello world");
        // "hello", " ", "world" — the space is its own run.
        assert_eq!(history.undo_depth(), 3);
        assert_eq!(history.undo[0].ops[0].text(), "hello");
        assert_eq!(history.undo[1].ops[0].text(), " ");
        assert_eq!(history.undo[2].ops[0].text(), "world");
    }

    #[test]
    fn punctuation_is_its_own_run() {
        let mut history = History::new();
        type_text(&mut history, 0, "a.b");
        assert_eq!(history.undo_depth(), 3);
    }

    #[test]
    fn a_newline_never_coalesces() {
        let mut history = History::new();
        type_text(&mut history, 0, "\n\n");
        assert_eq!(history.undo_depth(), 2, "each Enter is its own undo step");
    }

    #[test]
    fn a_pause_longer_than_the_window_starts_a_new_transaction() {
        let mut history = History::new();
        let start = Instant::now();
        history.begin_edit(Cursor::default());
        history.record_at(insert(0, "a"), start);
        history.end_edit(Cursor::default());
        history.begin_edit(Cursor::default());
        history.record_at(
            insert(1, "b"),
            start + COALESCE_WINDOW + Duration::from_millis(1),
        );
        history.end_edit(Cursor::default());
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn a_gap_in_the_buffer_starts_a_new_transaction() {
        let mut history = History::new();
        let now = Instant::now();
        history.record_at(insert(0, "a"), now);
        history.record_at(insert(40, "b"), now);
        assert_eq!(
            history.undo_depth(),
            2,
            "typing elsewhere is a separate step"
        );
    }

    #[test]
    fn backspaces_coalesce_backwards() {
        let mut history = History::new();
        let now = Instant::now();
        history.record_at(delete(4, "o"), now);
        history.record_at(delete(3, "l"), now);
        history.record_at(delete(2, "l"), now);
        assert_eq!(history.undo_depth(), 1);
        assert_eq!(history.undo[0].ops[0], delete(2, "llo"));
    }

    #[test]
    fn forward_deletes_coalesce_at_one_offset() {
        let mut history = History::new();
        let now = Instant::now();
        history.record_at(delete(2, "l"), now);
        history.record_at(delete(2, "l"), now);
        history.record_at(delete(2, "o"), now);
        assert_eq!(history.undo_depth(), 1);
        assert_eq!(history.undo[0].ops[0], delete(2, "llo"));
    }

    #[test]
    fn an_insert_never_merges_into_a_delete() {
        let mut history = History::new();
        let now = Instant::now();
        history.record_at(delete(0, "a"), now);
        history.record_at(insert(0, "b"), now);
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn a_paste_is_one_step_and_does_not_absorb_the_next_keystroke() {
        let mut history = History::new();
        let now = Instant::now();
        history.record_at(insert(0, "pasted text"), now);
        history.record_at(insert(11, "!"), now);
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn one_logical_edit_is_one_transaction_however_many_operations_it_takes() {
        let mut history = History::new();
        let now = Instant::now();
        history.begin_edit(Cursor::default());
        history.record_at(delete(0, "selected"), now);
        history.record_at(insert(0, "x"), now);
        history.end_edit(Cursor::default());
        assert_eq!(
            history.undo_depth(),
            1,
            "replacing a selection undoes as one"
        );
        assert_eq!(history.undo[0].ops.len(), 2);
    }

    #[test]
    fn sealing_ends_the_open_transaction() {
        let mut history = History::new();
        let now = Instant::now();
        history.record_at(insert(0, "a"), now);
        history.seal();
        history.record_at(insert(1, "b"), now);
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn recording_after_an_undo_drops_the_redo_branch() {
        let mut history = History::new();
        type_text(&mut history, 0, "ab");
        let transaction = history.take_undo().expect("one transaction");
        history.push_redo(transaction);
        assert!(history.can_redo());
        history.record(insert(0, "z"));
        assert!(!history.can_redo(), "a new edit abandons the redo branch");
    }

    #[test]
    fn the_save_point_survives_undo_and_redo() {
        let mut history = History::new();
        assert!(
            history.at_saved_point(),
            "a fresh document matches its file"
        );
        type_text(&mut history, 0, "hello");
        assert!(!history.at_saved_point());
        history.mark_saved();
        assert!(history.at_saved_point());

        let transaction = history.take_undo().expect("one transaction");
        history.push_redo(transaction);
        assert!(!history.at_saved_point(), "undone past the save");

        let transaction = history.take_redo().expect("one transaction");
        history.push_undo(transaction);
        assert!(history.at_saved_point(), "redone back onto it");
    }

    #[test]
    fn a_save_point_on_an_abandoned_branch_is_forgotten() {
        let mut history = History::new();
        type_text(&mut history, 0, "hello");
        history.mark_saved();
        let transaction = history.take_undo().expect("one transaction");
        history.push_redo(transaction);
        history.record(insert(0, "z"));
        assert!(
            !history.at_saved_point(),
            "the saved state is no longer reachable, so nothing may look clean"
        );
    }

    #[test]
    fn memory_is_proportional_to_what_was_typed_not_to_the_document() {
        let mut history = History::new();
        // 100k keystrokes in words of ten, which is the shape real typing has.
        let mut at = 0;
        for _ in 0..10_000 {
            type_text(&mut history, at, "keystroke");
            at += 9;
            type_text(&mut history, at, " ");
            at += 1;
        }
        let typed = at;
        assert_eq!(typed, 100_000);
        // The promise is a constant cost per keystroke, not a cost that grows
        // with the document: no snapshots, and one small header per word.
        assert!(
            history.memory_bytes() < typed * 64,
            "history held {} bytes for {typed} typed characters",
            history.memory_bytes()
        );
    }
}
