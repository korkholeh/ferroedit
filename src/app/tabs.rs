//! Open editor tabs: one document, one viewport, one place in the tab bar.
//!
//! The collection itself is still a plain `Vec<Tab>` on `App` (SPEC §8) — what
//! lives here is the tab, and the two rules that go with removing one: which
//! tab becomes active afterwards, and the fact that a closed tab takes its
//! `History` with it.

use std::path::Path;

use crate::app::diff::DiffState;
use crate::app::log::LogState;
use crate::app::table::TableView;
use crate::app::TextView;
use crate::editor::csv::Dialect;
use crate::editor::document::Document;
use crate::editor::viewport::Viewport;
use crate::editor::wrap::Layout;
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

/// One tab of the strip: a file being edited, a diff being read, or a history
/// being scrolled.
///
/// A diff is a tab rather than a pane drawn over the editor (it was one until
/// ADR-037): it is a thing the user opens, keeps, comes back to and closes, and
/// every one of those verbs already had an answer for tabs. What it is *not*
/// is a document — there is no cursor in it and nothing to save — which is why
/// it is a variant here rather than a flag on `Tab`. Every command that edits
/// goes through `App::active`, and that answers `None` for a diff tab, so a
/// read-only tab is read-only by construction rather than by a guard in each
/// of forty commands.
#[derive(Debug)]
pub enum TabItem {
    /// Every variant is boxed, so an element of `App::tabs` is a pointer
    /// rather than the size of the largest of them — a strip of diffs paying
    /// for documents that are not there.
    Editor(Box<Tab>),
    Diff(Box<DiffState>),
    /// A commit history (ADR-068). A tab for the reason a diff is one — it is
    /// opened, kept, come back to and closed — and read-only for the same
    /// reason: `App::active` answers `None` for it, so no editing command can
    /// reach it.
    Log(Box<LogState>),
}

impl TabItem {
    pub fn editor(&self) -> Option<&Tab> {
        match self {
            Self::Editor(tab) => Some(tab),
            _ => None,
        }
    }

    pub fn editor_mut(&mut self) -> Option<&mut Tab> {
        match self {
            Self::Editor(tab) => Some(tab),
            _ => None,
        }
    }

    /// A tab over an open document.
    pub fn editing(tab: Tab) -> Self {
        Self::Editor(Box::new(tab))
    }

    /// A tab over a diff being read.
    pub fn viewing(diff: DiffState) -> Self {
        Self::Diff(Box::new(diff))
    }

    pub fn diff(&self) -> Option<&DiffState> {
        match self {
            Self::Diff(diff) => Some(diff),
            _ => None,
        }
    }

    pub fn diff_mut(&mut self) -> Option<&mut DiffState> {
        match self {
            Self::Diff(diff) => Some(diff),
            _ => None,
        }
    }

    pub fn log(&self) -> Option<&LogState> {
        match self {
            Self::Log(log) => Some(log),
            _ => None,
        }
    }

    pub fn log_mut(&mut self) -> Option<&mut LogState> {
        match self {
            Self::Log(log) => Some(log),
            _ => None,
        }
    }

    pub fn history(log: LogState) -> Self {
        Self::Log(Box::new(log))
    }

    /// What the tab bar draws. A diff says whose diff it is: two tabs called
    /// `main.rs` — one being edited and one being read — would be a strip the
    /// user has to guess at.
    pub fn title(&self) -> String {
        match self {
            Self::Editor(tab) => tab.document.title().to_string(),
            Self::Diff(diff) => format!("Diff: {}", diff.name()),
            Self::Log(log) => log.name(),
        }
    }

    /// Whether the tab has unsaved changes. A diff never does: there is
    /// nothing in it to change.
    pub fn is_dirty(&self) -> bool {
        self.editor().is_some_and(|tab| tab.document.is_dirty())
    }

    /// What happened to the file underneath, for a tab that has one.
    pub fn stale(&self) -> Option<Stale> {
        self.editor().and_then(|tab| tab.stale)
    }

    /// Whether this is the tab that file is *edited* in. A diff of the same
    /// file is deliberately not a match: opening the file has to reach the
    /// buffer, not the diff beside it.
    pub fn is_at(&self, path: &Path) -> bool {
        self.editor().is_some_and(|tab| tab.is_at(path))
    }
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
    /// The dialect of the last table this tab showed, so that turning the view
    /// off and on again does not re-sniff a file the user has already answered
    /// for.
    last_dialect: Option<Dialect>,
    /// The CSV view, when this tab is showing one (SPEC §65).
    ///
    /// `Some` means the pane draws a table instead of the text, which also
    /// makes the tab read-only for as long as it lasts: the fields on screen
    /// are a parse of the buffer and there is nowhere in them to put a caret.
    /// A `.csv` or `.tsv` file opens with one already on, because that is what
    /// the file is; every other file gets one only when asked.
    pub table: Option<TableView>,
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
        let table = is_delimited(&document).then(|| TableView::new(sniff(&document)));
        Self {
            document,
            viewport: Viewport::default(),
            highlights: HighlightCache::default(),
            table,
            stale: None,
            last_dialect: None,
            asked: false,
        }
    }

    /// Whether the pane is showing a table rather than the text (SPEC §65).
    pub fn shows_table(&self) -> bool {
        self.table.is_some()
    }

    /// Turns the CSV view on or off, keeping the dialect a tab was opened with:
    /// switching to the text to fix a row and back must not throw away a
    /// delimiter the user chose by hand.
    pub fn toggle_table(&mut self) {
        match self.table.take() {
            Some(view) => self.last_dialect = Some(view.dialect),
            None => {
                let dialect = self.last_dialect.unwrap_or_else(|| sniff(&self.document));
                self.table = Some(TableView::new(dialect));
            }
        }
    }

    /// Re-reads the table from the buffer, if the view is on and anything it
    /// was parsed from has changed.
    pub fn sync_table(&mut self) {
        if let Some(view) = self.table.as_mut() {
            view.sync(&self.document);
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

    /// How this tab's document is laid out in the pane: the rows it shows, and
    /// where a line breaks when lines wrap.
    pub fn layout(&self, view: TextView) -> Layout {
        view.layout(self.document.line_count())
    }

    /// Scrolls the pane so the cursor is visible, after a move or an edit.
    pub fn follow_cursor(&mut self, view: TextView) {
        let layout = self.layout(view);
        self.viewport.follow_cursor(&self.document, layout);
    }

    /// The file this tab is showing, if it has one.
    pub fn is_at(&self, path: &Path) -> bool {
        self.document.path() == Some(path)
    }
}

/// Whether a document is one the CSV view opens by itself.
///
/// The extension and nothing else. Content sniffing would open the table over
/// any file with commas in it — a log, a shopping list — and being wrong here
/// costs the user the text of a file they only wanted to read.
fn is_delimited(document: &Document) -> bool {
    document
        .path()
        .and_then(|path| path.extension())
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("csv") || ext.eq_ignore_ascii_case("tsv"))
}

/// The dialect a document is read with until the user says otherwise.
fn sniff(document: &Document) -> Dialect {
    let extension = document
        .path()
        .and_then(|path| path.extension())
        .and_then(|ext| ext.to_str());
    let lines = (0..document.line_count()).map(|index| document.line(index));
    Dialect::sniff(extension, lines)
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
