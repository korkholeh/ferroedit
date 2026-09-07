//! Undo/redo transactions with coalescing.
//!
//! Memory is O(edited bytes) and never O(document): a transaction holds the
//! text an operation put in or took out, never a snapshot of the buffer
//! (SPEC §16, ARCHITECTURE §5). Typing a word is one transaction whose single
//! `Insert` grows by a character at a time, so a hundred thousand keystrokes
//! cost the hundred thousand bytes that were typed plus one small header per
//! word — not a hundred thousand copies of the file.
//!
//! O(edited bytes) is still unbounded over a long enough session, so the undo
//! stack has a byte budget and drops its oldest steps to stay inside it
//! (ADR-042). The newest step is never dropped: an editor that cannot undo the
//! thing that just happened is worse than one that uses the memory.
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

/// How much the undo stack may hold before its oldest steps are dropped
/// (ADR-042).
///
/// Sixteen megabytes is sixteen million characters of *edited* text, which no
/// typing session reaches — the point is the session that pastes and rewrites
/// for hours, where O(edited bytes) stops being a promise and becomes a leak.
/// The redo stack is fed from this one, so the pair is bounded by twice it.
pub const MAX_UNDO_BYTES: usize = 16 * 1024 * 1024;

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
    fn heap_bytes(&self) -> usize {
        self.ops.capacity() * std::mem::size_of::<EditOperation>()
            + self
                .ops
                .iter()
                .map(EditOperation::heap_bytes)
                .sum::<usize>()
    }

    /// What keeping this step costs, its own place on the stack included.
    ///
    /// A million one-character steps are a million headers, and a budget that
    /// counted only the text they hold would not see them.
    fn weight(&self) -> usize {
        std::mem::size_of::<Self>() + self.heap_bytes()
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
    /// What the undo stack currently weighs, kept as steps are pushed, merged
    /// and popped rather than recomputed: the budget is checked after every
    /// keystroke and walking the whole stack to do it would be O(steps) per
    /// character.
    undo_bytes: usize,
    /// The budget itself. A field rather than the constant read directly, so
    /// the tests for the trimming can use a small one instead of allocating
    /// tens of megabytes to reach the real one.
    budget: usize,
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
            undo_bytes: 0,
            budget: MAX_UNDO_BYTES,
        }
    }

    /// The same, with a smaller budget. Test-only: nothing in the editor sets
    /// one, and a configurable undo budget is not a setting anybody has asked
    /// for.
    #[cfg(test)]
    fn with_budget(budget: usize) -> Self {
        Self {
            budget,
            ..Self::new()
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
            let transaction = Transaction {
                ops: vec![op],
                cursor_before: self.pending_before,
                cursor_after: self.pending_before,
                at: now,
            };
            self.undo_bytes += transaction.weight();
            self.undo.push(transaction);
        }
        self.open = single;
        self.grouped = self.depth > 0;
        self.trim();
    }

    /// Drops the oldest steps until the stack is inside its budget (ADR-042).
    ///
    /// The newest step always survives, however large it is: undoing the paste
    /// that just happened is the one thing undo must always be able to do.
    fn trim(&mut self) {
        if self.undo_bytes <= self.budget {
            return;
        }
        let mut freed = 0;
        let mut dropped = 0;
        for transaction in &self.undo[..self.undo.len().saturating_sub(1)] {
            if self.undo_bytes - freed <= self.budget {
                break;
            }
            freed += transaction.weight();
            dropped += 1;
        }
        if dropped == 0 {
            return;
        }
        log::debug!("undo history: dropping {dropped} step(s), freeing {freed} bytes");
        self.undo.drain(..dropped);
        self.undo_bytes -= freed;
        // `saved` is a stack *height*, so every one of them moves down by what
        // was dropped — and a save point below the new floor is gone, because
        // the state it named can no longer be undone back to.
        self.saved = match self.saved {
            Some(at) if at >= dropped => Some(at - dropped),
            _ => None,
        };
    }

    /// Adds an operation to the open transaction — the forced case, inside one
    /// logical edit.
    fn append(&mut self, op: EditOperation, now: Instant) {
        let Some(last) = self.undo.last_mut() else {
            return;
        };
        let before = last.weight();
        last.at = now;
        match last.ops.last_mut() {
            Some(prev) if mergeable(prev, &op) => merge_into(prev, op),
            _ => last.ops.push(op),
        }
        self.undo_bytes = self.undo_bytes + last.weight() - before;
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
        let before = last.weight();
        last.at = now;
        if let Some(prev) = last.ops.last_mut() {
            merge_into(prev, op);
        }
        self.undo_bytes = self.undo_bytes + last.weight() - before;
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
        let taken = self.undo.pop();
        if let Some(transaction) = taken.as_ref() {
            self.undo_bytes = self.undo_bytes.saturating_sub(transaction.weight());
        }
        taken
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
        self.undo_bytes += transaction.weight();
        self.undo.push(transaction);
        self.open = false;
        self.trim();
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
            transactions.iter().map(Transaction::weight).sum::<usize>()
        };
        stack(&self.undo) + stack(&self.redo)
    }

    /// What the undo stack weighs, as the budget counts it.
    #[cfg(test)]
    pub fn undo_bytes(&self) -> usize {
        self.undo_bytes
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

    /// One insert of a whole string — a paste, which is never coalesced.
    fn paste(at: usize, text: String) -> EditOperation {
        EditOperation::Insert {
            at: CharIdx(at),
            text,
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
        // Nowhere near the budget, which is the point of where it is set: a
        // real typing session never meets it (ADR-042).
        assert!(history.undo_bytes() < MAX_UNDO_BYTES / 4);
    }

    /// The accounting the budget is enforced against has to track what the
    /// stacks actually hold, or the budget is a number about nothing.
    #[test]
    fn the_running_weight_matches_a_full_recount() {
        let mut history = History::new();
        let recount =
            |history: &History| history.undo.iter().map(Transaction::weight).sum::<usize>();
        for i in 0..200 {
            type_text(&mut history, i * 4, "word");
            assert_eq!(history.undo_bytes(), recount(&history), "after {i} words");
        }
        // Coalescing into an open transaction, a paste, and popping.
        history.record(paste(800, "a large paste ".repeat(64)));
        assert_eq!(history.undo_bytes(), recount(&history));
        history.take_undo();
        assert_eq!(history.undo_bytes(), recount(&history));
        history.push_undo(Transaction {
            ops: vec![EditOperation::Insert {
                at: CharIdx(0),
                text: "back".to_string(),
            }],
            cursor_before: Cursor::default(),
            cursor_after: Cursor::default(),
            at: Instant::now(),
        });
        assert_eq!(history.undo_bytes(), recount(&history));
    }

    /// The pathological session the budget exists for: pastes that never stop.
    #[test]
    fn the_oldest_steps_are_dropped_once_the_budget_is_reached() {
        const BUDGET: usize = 16 * 1024;
        let mut history = History::with_budget(BUDGET);
        let chunk = "x".repeat(1024);
        for i in 0..64 {
            history.record(paste(i * chunk.len(), chunk.clone()));
        }
        assert!(
            history.undo_bytes() <= BUDGET,
            "held {} bytes",
            history.undo_bytes()
        );
        assert!(history.undo_depth() >= 8, "{}", history.undo_depth());
        // What is left is the *newest* steps: the last paste is still there to
        // be undone.
        let last = history.take_undo().expect("a step to undo");
        assert_eq!(last.ops[0].len_chars(), chunk.len());
    }

    /// A single step larger than the whole budget is still undoable: dropping
    /// it would mean a paste that cannot be taken back.
    #[test]
    fn the_newest_step_survives_however_large_it_is() {
        const BUDGET: usize = 1024;
        let mut history = History::with_budget(BUDGET);
        history.record(paste(0, "y".repeat(BUDGET * 4)));
        assert_eq!(history.undo_depth(), 1);
        assert!(history.undo_bytes() > BUDGET);
        assert!(history.take_undo().is_some());
    }

    /// The save point is a stack height, so dropping the bottom of the stack
    /// moves it — and a save point below the new floor is gone, because the
    /// state it named can no longer be undone back to.
    #[test]
    fn dropping_steps_moves_the_save_point_or_loses_it() {
        const BUDGET: usize = 16 * 1024;
        let chunk = "z".repeat(1024);
        let fill = |history: &mut History, from: usize, to: usize| {
            for i in from..to {
                history.record(paste(i * chunk.len(), chunk.clone()));
            }
        };

        let mut history = History::with_budget(BUDGET);
        fill(&mut history, 0, 4);
        history.mark_saved();
        assert!(history.at_saved_point());
        fill(&mut history, 4, 64);
        assert!(
            !history.at_saved_point(),
            "the saved state was dropped, so nothing may look clean"
        );

        // A save point above the floor survives, moved down by what went.
        let mut history = History::with_budget(BUDGET);
        fill(&mut history, 0, 64);
        history.mark_saved();
        let depth = history.undo_depth();
        assert_eq!(history.saved, Some(depth));
        history.record(paste(0, chunk.clone()));
        assert!(
            history.saved.is_some_and(|at| at < depth),
            "{:?}",
            history.saved
        );
    }
}
