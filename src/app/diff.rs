//! The diff viewer's state: which file, which side, and where it is scrolled.
//!
//! Read-only by construction (SPEC §36): there is no document, no cursor and
//! no history here, only lines and an offset into them. It is the payload of a
//! tab of its own (`app::tabs::TabItem::Diff`), so a diff is opened, kept,
//! switched away from and closed exactly like a file — and it is re-read
//! whenever the repository changes, so a tab left open does not go on showing
//! a change that has since been staged.

use std::path::{Path, PathBuf};

use crate::git::diff::{Diff, DiffSide};

/// Columns one press of `Left` or `Right` moves the window by.
///
/// A cell at a time would be a key held down for a minified line; a whole pane
/// would lose the reader's place. Eight is a step that stays readable.
pub const HORIZONTAL_STEP: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffState {
    /// Repository-relative, exactly as git printed it in the status.
    pub path: PathBuf,
    pub side: DiffSide,
    pub diff: Diff,
    /// First line drawn.
    pub scroll: usize,
    /// First display column drawn.
    pub h_scroll: usize,
}

impl DiffState {
    pub fn new(path: &Path, side: DiffSide, diff: Diff) -> Self {
        Self {
            path: path.to_path_buf(),
            side,
            diff,
            scroll: 0,
            h_scroll: 0,
        }
    }

    /// The file name alone, for the tab strip. A tab is a dozen cells wide and
    /// `src/commands/execute.rs` is not.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// ` Diff — src/main.rs [worktree] +12 −3 `.
    ///
    /// The side is in the title because the two sides are different answers
    /// and a viewer that did not say which it was showing would be no answer
    /// at all. A conflicted file adds `merge` for the same reason: what is on
    /// screen then is a combined diff, whose `+` and `-` are one column per
    /// parent rather than one change (ADR-045), and the columns do not
    /// announce themselves.
    pub fn title(&self) -> String {
        let truncated = if self.diff.truncated { " (cut)" } else { "" };
        let merge = if self.diff.is_combined() {
            ", merge"
        } else {
            ""
        };
        format!(
            " Diff — {} [{}{merge}] {}{truncated} ",
            self.path.display(),
            self.side.label(),
            self.diff.summary()
        )
    }

    /// `12/340`, the bottom-right readout: which line is at the top of the
    /// window, out of how many there are.
    pub fn position(&self) -> String {
        let first = if self.diff.is_empty() {
            0
        } else {
            self.scroll + 1
        };
        format!(" {first}/{} ", self.diff.len())
    }

    /// Moves the window by `delta` lines, clamped at both ends.
    ///
    /// The last line can be scrolled to the top of the window rather than to
    /// the bottom of the pane: a diff whose end is one line past the fold is
    /// otherwise unreachable.
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let target = self.scroll as isize + delta;
        self.scroll = target.max(0) as usize;
        self.clamp(height);
    }

    pub fn scroll_h(&mut self, delta: isize) {
        let target = self.h_scroll as isize + delta;
        self.h_scroll = (target.max(0) as usize).min(self.diff.width.saturating_sub(1));
    }

    /// The top of the diff, horizontally as well: a reader who has gone back
    /// to the first line wants the first column with it.
    pub fn home(&mut self) {
        self.scroll = 0;
        self.h_scroll = 0;
    }

    pub fn end(&mut self, height: usize) {
        self.scroll = self.diff.len();
        self.clamp(height);
    }

    /// Keeps the window inside the diff. Called after anything that changes
    /// either — the scroll, or the diff a refresh replaced it with.
    pub fn clamp(&mut self, height: usize) {
        let last = self.diff.len().saturating_sub(height.max(1));
        self.scroll = self.scroll.min(last);
        self.h_scroll = self.h_scroll.min(self.diff.width.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(lines: usize) -> Diff {
        let text: String = (0..lines).map(|i| format!(" line {i}\n")).collect();
        Diff::parse(&text)
    }

    fn viewer(lines: usize) -> DiffState {
        DiffState::new(Path::new("src/main.rs"), DiffSide::Worktree, diff(lines))
    }

    #[test]
    fn the_title_names_the_file_the_side_and_the_counts() {
        let mut viewer = viewer(0);
        viewer.diff = Diff::parse("@@ -1 +1 @@\n-a\n+b\n");
        assert_eq!(viewer.title(), " Diff — src/main.rs [worktree] +1 −1 ");
    }

    #[test]
    fn a_cut_diff_says_so_in_its_title() {
        let mut viewer = viewer(1);
        viewer.diff.truncated = true;
        assert!(viewer.title().contains("(cut)"), "{}", viewer.title());
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut viewer = viewer(20);
        viewer.scroll_by(-5, 10);
        assert_eq!(viewer.scroll, 0, "there is nothing above the first line");
        viewer.scroll_by(100, 10);
        assert_eq!(viewer.scroll, 10, "the last line is the last one reachable");
    }

    #[test]
    fn a_diff_shorter_than_the_pane_does_not_scroll_at_all() {
        let mut viewer = viewer(3);
        viewer.scroll_by(5, 10);
        assert_eq!(viewer.scroll, 0);
        viewer.end(10);
        assert_eq!(viewer.scroll, 0);
    }

    #[test]
    fn end_puts_the_last_line_in_the_window_and_home_comes_back() {
        let mut viewer = viewer(100);
        viewer.end(10);
        assert_eq!(viewer.scroll, 90);
        assert_eq!(viewer.position(), " 91/100 ");
        viewer.h_scroll = 4;
        viewer.home();
        assert_eq!((viewer.scroll, viewer.h_scroll), (0, 0));
    }

    #[test]
    fn the_horizontal_window_stops_at_the_widest_line() {
        let mut viewer = viewer(2);
        // ` line 0` is seven cells, so six is the furthest offset that still
        // leaves a cell of text on screen.
        viewer.scroll_h(50);
        assert_eq!(viewer.h_scroll, 6);
        viewer.scroll_h(-50);
        assert_eq!(viewer.h_scroll, 0);
    }

    #[test]
    fn a_refresh_that_shortened_the_diff_pulls_the_window_back() {
        let mut viewer = viewer(100);
        viewer.end(10);
        viewer.diff = diff(12);
        viewer.clamp(10);
        assert_eq!(viewer.scroll, 2);
    }

    #[test]
    fn an_empty_viewer_reports_no_position_and_does_not_panic() {
        let mut viewer = viewer(0);
        assert_eq!(viewer.position(), " 0/0 ");
        viewer.end(0);
        viewer.scroll_by(3, 0);
        assert_eq!(viewer.scroll, 0);
    }
}
