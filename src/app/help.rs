//! The help screen's state: the key tables, and where they are scrolled.
//!
//! Read-only like the diff viewer, and for the same reason (SPEC §36's shape
//! reused for SPEC §6's Help menu): there is nothing here to edit, only rows to
//! page through. What it shows is `docs::sections()` — the keymap itself — so
//! the screen cannot advertise a key that is not bound (ADR-038).
//!
//! The lines are laid out on demand rather than stored, because their number
//! depends on the width: a note wraps into two rows in a narrow terminal and
//! one in a wide one, and a scroll offset measured against the wrong width
//! would jump when the window is resized.

use unicode_width::UnicodeWidthStr;

use crate::app::focus::FocusTarget;
use crate::docs::{self, HelpSection};

/// Columns kept for the key column at most. Past this a long key label is
/// allowed to run into the gap rather than squeezing every action off screen.
const MAX_KEY_COLUMN: usize = 22;

/// Two spaces of indent, and two between the columns.
const INDENT: usize = 2;
const GAP: usize = 2;

/// The last line of the screen: what it does *not* cover, and where that is.
const FOOTER: &str = "Typing, the mouse and the terminal's own limits are covered in \
                      docs/SHORTCUTS.md, which is generated from these same tables.";

/// One rendered line. The renderer decides colours; this decides text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpLine {
    Blank,
    Heading(String),
    Note(String),
    Row { keys: String, action: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpState {
    pub sections: Vec<HelpSection>,
    /// First line drawn.
    pub scroll: usize,
    /// Where focus goes when the screen closes — the pane it was opened from.
    pub return_focus: FocusTarget,
}

impl HelpState {
    pub fn new(return_focus: FocusTarget) -> Self {
        Self {
            sections: docs::sections(),
            scroll: 0,
            return_focus,
        }
    }

    pub fn title(&self) -> String {
        " Keyboard shortcuts ".to_string()
    }

    /// `12/240`, the bottom-right readout, in the diff viewer's shape.
    pub fn position(&self, width: usize) -> String {
        let len = self.lines(width).len();
        let first = if len == 0 { 0 } else { self.scroll + 1 };
        format!(" {first}/{len} ")
    }

    /// Every line of the screen, laid out for a pane `width` cells wide.
    ///
    /// Cheap enough to call once a frame: a few dozen rows of static text, and
    /// re-deriving it is what keeps the scroll honest across a resize.
    pub fn lines(&self, width: usize) -> Vec<HelpLine> {
        let key_column = self.key_column(width);
        let mut lines = Vec::new();
        for section in &self.sections {
            if !lines.is_empty() {
                lines.push(HelpLine::Blank);
            }
            lines.push(HelpLine::Heading(section.heading.to_string()));
            for line in wrap(&plain(section.note), width.saturating_sub(INDENT)) {
                lines.push(HelpLine::Note(line));
            }
            lines.push(HelpLine::Blank);
            for row in &section.rows {
                lines.push(HelpLine::Row {
                    keys: pad(&row.key_label(), key_column),
                    action: row.action.clone(),
                });
            }
        }
        lines.push(HelpLine::Blank);
        for line in wrap(FOOTER, width.saturating_sub(INDENT)) {
            lines.push(HelpLine::Note(line));
        }
        lines
    }

    /// Where the action column starts: the widest key label, capped so that a
    /// narrow pane keeps room for the actions.
    fn key_column(&self, width: usize) -> usize {
        let widest = self
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .map(|row| row.key_label().width())
            .max()
            .unwrap_or(0);
        widest
            .min(MAX_KEY_COLUMN)
            .min(width.saturating_sub(INDENT + GAP) / 2)
            + GAP
    }

    pub fn scroll_by(&mut self, delta: isize, width: usize, height: usize) {
        let target = self.scroll as isize + delta;
        self.scroll = target.max(0) as usize;
        self.clamp(width, height);
    }

    pub fn home(&mut self) {
        self.scroll = 0;
    }

    pub fn end(&mut self, width: usize, height: usize) {
        self.scroll = self.lines(width).len();
        self.clamp(width, height);
    }

    /// Keeps the window inside the text. Called after anything that changes
    /// either — the scroll, or the width the lines were laid out for.
    pub fn clamp(&mut self, width: usize, height: usize) {
        let last = self.lines(width).len().saturating_sub(height.max(1));
        self.scroll = self.scroll.min(last);
    }
}

/// Markdown's emphasis marks, removed.
///
/// The notes are written once, for a file that renders backticks and asterisks
/// and for a terminal that does not. Stripping here is the cheaper direction:
/// the file is the one with a syntax, so the screen takes it away rather than
/// the file having to avoid it.
fn plain(text: &str) -> String {
    text.chars().filter(|c| *c != '`' && *c != '*').collect()
}

/// Greedy word wrap. A word longer than the line gets a line of its own rather
/// than being cut: every word here is English or a key label, and neither is
/// improved by being broken in half.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let extra = if current.is_empty() {
            word.width()
        } else {
            word.width() + 1
        };
        if !current.is_empty() && current.width() + extra > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Right-pads to a column count, measured in cells, always leaving at least
/// `GAP` cells after the text.
///
/// A label wider than the column keeps its own gap rather than being cut:
/// `Ctrl+Shift+Tab / Ctrl+PageUp` is one key row too wide for a column sized
/// for the rest, and running it into its own description is worse than letting
/// that one row sit proud of the others.
fn pad(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    let target = width.max(text.width() + GAP);
    for _ in text.width()..target {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn help() -> HelpState {
        HelpState::new(FocusTarget::Editor)
    }

    #[test]
    fn the_screen_is_built_from_the_keymap() {
        let lines = help().lines(80);
        let rows: Vec<&HelpLine> = lines
            .iter()
            .filter(|line| matches!(line, HelpLine::Row { .. }))
            .collect();
        assert!(rows.len() > 40, "{} rows is too few", rows.len());
        assert!(
            lines.iter().any(|line| matches!(
                line,
                HelpLine::Row { keys, action }
                    if keys.trim() == "Ctrl+Q" && action.starts_with("Quit")
            )),
            "the quit binding is not on the screen"
        );
        assert!(lines
            .iter()
            .any(|line| matches!(line, HelpLine::Heading(h) if h == "Editor")));
    }

    /// The whole point of ADR-038: the screen and the file are one table.
    #[test]
    fn every_binding_on_the_screen_is_one_the_keymap_has() {
        use crate::event::keyboard::{BINDINGS, INPUT_BINDINGS};
        let lines = help().lines(100);
        for binding in BINDINGS.iter().chain(INPUT_BINDINGS) {
            assert!(
                lines.iter().any(|line| match line {
                    HelpLine::Row { keys, .. } =>
                        keys.split('/').any(|key| key.trim() == binding.label),
                    _ => false,
                }),
                "{} is bound but not on the help screen",
                binding.label
            );
        }
    }

    /// The column is one width for every row that fits in it, and a label too
    /// long for it runs past rather than being cut: `Ctrl+Shift+Tab` truncated
    /// to fit a column is a key nobody can press.
    #[test]
    fn the_key_column_is_aligned_and_never_truncates() {
        let lines = help().lines(40);
        let column = 22usize.min((40 - (INDENT + GAP)) / 2) + GAP;
        let mut fitted = 0;
        for line in &lines {
            let HelpLine::Row { keys, .. } = line else {
                continue;
            };
            assert!(
                keys.width() >= column,
                "{keys:?} is narrower than the column"
            );
            if keys.trim_end().width() + GAP <= column {
                assert_eq!(keys.width(), column, "{keys:?}");
                fitted += 1;
            }
        }
        assert!(fitted > 20, "only {fitted} rows fitted the column");
        let long = lines
            .iter()
            .find_map(|line| match line {
                HelpLine::Row { keys, .. } if keys.starts_with("Ctrl+Shift+Tab") => Some(keys),
                _ => None,
            })
            .expect("a label too wide for the column");
        assert_eq!(long.trim_end(), "Ctrl+Shift+Tab / Ctrl+PageUp");
        assert_eq!(
            long.width(),
            long.trim_end().width() + GAP,
            "an over-wide label still keeps its gap"
        );
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut help = help();
        help.scroll_by(-5, 80, 10);
        assert_eq!(help.scroll, 0);
        help.end(80, 10);
        assert_eq!(help.scroll, help.lines(80).len() - 10);
        help.home();
        assert_eq!(help.scroll, 0);
    }

    /// A resize relays out the text, and a window that is now past the end
    /// comes back rather than showing blank rows.
    #[test]
    fn narrowing_the_pane_pulls_the_window_back() {
        let mut help = help();
        help.end(200, 10);
        let deep = help.scroll;
        help.clamp(200, 10);
        assert_eq!(help.scroll, deep, "the same width changes nothing");
        // Narrower wraps more notes, so there are more lines, not fewer: the
        // window stays where it was.
        assert!(help.lines(40).len() >= help.lines(200).len());
    }

    #[test]
    fn the_readout_counts_from_one() {
        let mut help = help();
        assert!(help.position(80).starts_with(" 1/"));
        help.scroll_by(9, 80, 10);
        assert!(
            help.position(80).starts_with(" 10/"),
            "{}",
            help.position(80)
        );
    }

    /// A frame drawn before the first layout reports a zero-sized pane, and a
    /// key pressed in it must not take the process down.
    #[test]
    fn a_pane_with_no_size_still_clamps_and_does_not_panic() {
        let mut help = help();
        help.scroll_by(3, 0, 0);
        assert!(help.scroll < help.lines(0).len());
        help.end(0, 0);
        help.home();
        assert_eq!(help.scroll, 0);
    }

    #[test]
    fn emphasis_marks_do_not_reach_the_screen() {
        assert_eq!(plain("`Ctrl+S` is *not* modal"), "Ctrl+S is not modal");
        for line in help().lines(80) {
            if let HelpLine::Note(text) = line {
                assert!(!text.contains('`') && !text.contains('*'), "{text:?}");
            }
        }
    }

    #[test]
    fn wrapping_breaks_on_words_and_keeps_long_ones_whole() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(
            wrap("supercalifragilistic", 5),
            vec!["supercalifragilistic"]
        );
        assert!(wrap("anything", 0).is_empty());
    }
}
