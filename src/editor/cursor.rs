//! Cursor position plus the preferred visual column.

use crate::editor::coords::{CharIdx, VisualCol};

/// Where the caret is, in the canonical storage of `(line, char_in_line)`.
///
/// `preferred_col` is what makes a column of Down presses through a ragged
/// block of text come back out at the column it started in: vertical movement
/// reads it and never writes it, and every horizontal movement resets it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub line: usize,
    pub column: CharIdx,
    pub preferred_col: VisualCol,
}

/// A cursor movement request. The document resolves it, because only the
/// document knows the text the motion has to travel over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
    PageUp,
    PageDown,
    DocumentStart,
    DocumentEnd,
}

impl Motion {
    /// Whether the motion sets a new preferred column.
    ///
    /// Vertical motions are exactly the ones that must preserve it; everything
    /// else is the user choosing a column deliberately.
    pub fn is_horizontal(self) -> bool {
        !matches!(self, Self::Up | Self::Down | Self::PageUp | Self::PageDown)
    }
}
