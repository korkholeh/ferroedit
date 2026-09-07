//! A one-line text field: the value, and a caret that moves by cluster.
//!
//! Two things need one — a dialog's name prompt (Phase 6) and the search bar's
//! two fields (Phase 8) — and neither is a document: there is no undo history,
//! no selection and no viewport, so `Document` would be the wrong thing to
//! reach for and a second copy of this would be the wrong thing to write.

use crate::editor::coords::{next_grapheme, prev_grapheme, CharIdx};

/// A one-line text field.
///
/// The caret is a `CharIdx` and moves by grapheme cluster, exactly as it does
/// in the editor: a file name is arbitrary Unicode, and `Backspace` over `é`
/// written as `e` + U+0301 has to remove the whole cluster (SPEC §48).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputField {
    pub value: String,
    pub cursor: CharIdx,
}

impl InputField {
    /// A field holding `value`, with the caret at its end — the position a user
    /// who is about to edit a pre-filled name expects.
    pub fn new(value: &str) -> Self {
        Self {
            value: value.to_string(),
            cursor: CharIdx(value.chars().count()),
        }
    }

    pub fn insert(&mut self, ch: char) {
        let at = self.byte_of(self.cursor);
        self.value.insert(at, ch);
        self.cursor = CharIdx(self.cursor.0 + 1);
    }

    pub fn insert_str(&mut self, text: &str) {
        let at = self.byte_of(self.cursor);
        self.value.insert_str(at, text);
        self.cursor = CharIdx(self.cursor.0 + text.chars().count());
    }

    /// Removes the cluster before the caret.
    pub fn backspace(&mut self) {
        let start = prev_grapheme(&self.value, self.cursor);
        if start == self.cursor {
            return;
        }
        self.remove(start, self.cursor);
        self.cursor = start;
    }

    /// Removes the cluster after the caret.
    pub fn delete(&mut self) {
        let end = next_grapheme(&self.value, self.cursor);
        if end == self.cursor {
            return;
        }
        self.remove(self.cursor, end);
    }

    /// Moves the caret by whole clusters, clamped at both ends.
    pub fn step(&mut self, delta: i16) {
        self.cursor = if delta < 0 {
            prev_grapheme(&self.value, self.cursor)
        } else {
            next_grapheme(&self.value, self.cursor)
        };
    }

    pub fn home(&mut self) {
        self.cursor = CharIdx(0);
    }

    pub fn end(&mut self) {
        self.cursor = CharIdx(self.value.chars().count());
    }

    fn remove(&mut self, from: CharIdx, to: CharIdx) {
        let (from, to) = (self.byte_of(from), self.byte_of(to));
        self.value.replace_range(from..to, "");
    }

    fn byte_of(&self, index: CharIdx) -> usize {
        self.value
            .char_indices()
            .nth(index.0)
            .map_or(self.value.len(), |(byte, _)| byte)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_deleting_move_by_cluster_not_by_char() {
        let mut field = InputField::new("");
        for ch in "note".chars() {
            field.insert(ch);
        }
        assert_eq!(field.value, "note");
        assert_eq!(field.cursor, CharIdx(4));

        // `é` decomposed: one cluster, two chars (SPEC §48).
        field.insert_str("e\u{301}");
        assert_eq!(field.cursor, CharIdx(6));
        field.backspace();
        assert_eq!(field.value, "note", "the mark and its base went together");

        field.home();
        field.delete();
        assert_eq!(field.value, "ote");
        field.step(1);
        field.step(1);
        field.step(1);
        field.step(1);
        assert_eq!(field.cursor, CharIdx(3), "and the caret stops at the end");
        field.step(-1);
        assert_eq!(field.cursor, CharIdx(2));
    }

    #[test]
    fn editing_an_empty_field_does_nothing_rather_than_underflowing() {
        let mut field = InputField::default();
        field.backspace();
        field.delete();
        field.step(-1);
        assert_eq!(field.value, "");
        assert_eq!(field.cursor, CharIdx(0));
    }

    #[test]
    fn the_caret_can_be_moved_into_the_middle_and_typed_at() {
        let mut field = InputField::new("main.rs");
        field.home();
        field.insert_str("old_");
        assert_eq!(field.value, "old_main.rs");
        assert_eq!(field.cursor, CharIdx(4));
        field.end();
        assert_eq!(field.cursor, CharIdx(11));
    }
}
