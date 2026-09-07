//! Open editor tabs: one document, one viewport, one place in the tab bar.
//!
//! The collection itself is still a plain `Vec<Tab>` on `App` (SPEC §8) — what
//! lives here is the tab, and the two rules that go with removing one: which
//! tab becomes active afterwards, and the fact that a closed tab takes its
//! `History` with it.

use std::path::Path;

use crate::app::EditorView;
use crate::editor::document::Document;
use crate::editor::viewport::Viewport;
use crate::syntax::cache::{Disabled, HighlightCache};

/// A file that moved under a buffer the editor could not simply re-read
/// (ADR-043).
///
/// Only a tab with unsaved changes ever carries one: a clean buffer is reloaded
/// where it stands, because there is nothing in it that is not also on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stale {
    /// Somebody else wrote to the file.
    Changed,
    /// It is not there any more.
    Gone,
}

/// One open editor tab: a document and where the pane is scrolled to.
///
/// The scroll offset lives here rather than on `App` so that switching back to
/// a tab shows it where it was left.
#[derive(Debug)]
pub struct Tab {
    pub document: Document,
    pub viewport: Viewport,
    /// The colours of the lines on screen. Per tab, because the parser state it
    /// caches is this document's, and it dies with the tab the way the history
    /// does.
    pub highlights: HighlightCache,
    /// What happened to the file underneath, when it is something the editor
    /// has not resolved. Drawn in the tab bar and answered by a prompt.
    pub stale: Option<Stale>,
    /// Whether the user has already been asked about the current `stale`.
    ///
    /// The watcher reports every burst in the workspace, so without this the
    /// same question would be re-opened on every build, every checkout and
    /// every save of an unrelated file for as long as the tab stayed
    /// unresolved. It resets only when the state itself changes.
    pub asked: bool,
}

impl Tab {
    pub fn new(document: Document) -> Self {
        Self {
            document,
            viewport: Viewport::default(),
            highlights: HighlightCache::default(),
            stale: None,
            asked: false,
        }
    }

    /// Records what happened to the file, and whether it is worth asking about.
    ///
    /// Returns true when this is news — a state the tab was not already in — so
    /// the caller can ask once rather than once per filesystem event.
    pub fn mark_stale(&mut self, stale: Option<Stale>) -> bool {
        if self.stale == stale {
            return false;
        }
        self.stale = stale;
        self.asked = false;
        true
    }

    /// Re-colours the lines the viewport is over, if anything moved.
    ///
    /// Called once per frame from the run loop rather than from the renderer:
    /// `ui/` is read-only over `&App` (ARCHITECTURE §1), and highlighting needs
    /// to write down what it parsed or it is not a cache.
    ///
    /// Returns the reason on the frame that turns highlighting off, so the
    /// caller can say so once.
    pub fn sync_highlight(&mut self, height: usize) -> Option<Disabled> {
        self.highlights
            .sync(&mut self.document, self.viewport.top_line, height)
    }

    /// Scrolls the pane so the cursor is visible, after a move or an edit.
    pub fn follow_cursor(&mut self, view: EditorView) {
        self.viewport.clamp(self.document.line_count());
        self.viewport.follow_cursor(
            self.document.cursor().line,
            self.document.cursor_visual_col(),
            view.height as usize,
            view.text_width(self.document.line_count()),
        );
    }

    /// The file this tab is showing, if it has one.
    pub fn is_at(&self, path: &Path) -> bool {
        self.document.path() == Some(path)
    }
}

/// Which tab is active after the one at `closed` is removed.
///
/// Closing the active tab moves to the one that slid into its place, which is
/// the tab to its right, and to the new last tab when it was already the last.
/// Closing a tab to the left of the active one shifts the active index down so
/// the same document stays in front of the user — the bug this function exists
/// to prevent is closing tab 0 and silently switching the file being edited.
pub fn active_after_close(active: Option<usize>, closed: usize, remaining: usize) -> Option<usize> {
    if remaining == 0 {
        return None;
    }
    match active {
        None => None,
        Some(active) if active > closed => Some(active - 1),
        Some(active) if active == closed => Some(closed.min(remaining - 1)),
        Some(active) => Some(active),
    }
}

/// Directory the fixture documents claim to live in.
///
/// `/dev/null` is a file, so nothing can ever be created underneath it: a test
/// that saves a fixture document gets a filesystem error instead of writing
/// into whatever directory the test runner started in.
#[cfg(test)]
const FIXTURE_DIR: &str = "/dev/null/ferroedit-fixture";

#[cfg(test)]
impl Tab {
    /// A tab over in-memory text that has a name but no file behind it.
    pub fn scratch(name: &str, text: &str) -> Self {
        let path = std::path::PathBuf::from(FIXTURE_DIR).join(name);
        Self::new(Document::from_text(text, Some(path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_active_tab_moves_to_the_one_that_takes_its_place() {
        assert_eq!(active_after_close(Some(1), 1, 3), Some(1));
    }

    #[test]
    fn closing_the_last_tab_falls_back_to_the_new_last_one() {
        assert_eq!(active_after_close(Some(3), 3, 3), Some(2));
    }

    #[test]
    fn closing_a_tab_to_the_left_keeps_the_same_document_in_front() {
        assert_eq!(active_after_close(Some(2), 0, 3), Some(1));
    }

    #[test]
    fn closing_a_tab_to_the_right_leaves_the_active_index_alone() {
        assert_eq!(active_after_close(Some(0), 2, 3), Some(0));
    }

    #[test]
    fn closing_the_only_tab_leaves_nothing_active() {
        assert_eq!(active_after_close(Some(0), 0, 0), None);
    }
}
