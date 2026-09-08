//! Application state. Owns tabs, workspace, focus and notifications.
//! Never renders and never talks to crossterm directly.

pub mod browser;
pub mod dialog;
pub mod diff;
pub mod focus;
pub mod git;
pub mod help;
pub mod input_field;
pub mod notifications;
pub mod search;
pub mod tabs;
pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use crate::config::Settings;
use crate::event::AppEvent;
use crate::filesystem::watcher::Watch;
use dialog::DialogState;
use diff::DiffState;
use focus::FocusTarget;
use git::GitState;
use help::HelpState;
use notifications::Notifications;
use search::SearchState;
pub use tabs::{Tab, TabItem};
use workspace::Workspace;

use crate::editor::clipboard::Clipboard;
use crate::editor::document::{Document, DocumentError};
use crate::editor::viewport::gutter_width;
use crate::editor::wrap::Layout;
use crate::filesystem::tree::{FileTree, TreeRow};

/// The size of the editor pane in cells, mirrored out of the last frame's
/// layout.
///
/// Scrolling has to know how big the window is, and the layout is only known
/// while a frame is being drawn — so the main loop copies it here, the same way
/// it keeps the frame's rects for mouse hit-testing. It is frame geometry, not
/// user state: nothing but the loop writes it, and no command reads it except
/// to decide how far to scroll.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EditorView {
    pub width: u16,
    pub height: u16,
}

impl EditorView {
    /// Columns left for text once the line-number gutter has taken its share.
    pub fn text_width(self, line_count: usize) -> usize {
        (self.width as usize).saturating_sub(gutter_width(line_count))
    }
}

/// The editor pane as the *text* sees it: the geometry of the last drawn frame
/// plus whether lines wrap in it (SPEC §58).
///
/// The two travel together because everything that scrolls or moves a cursor
/// needs both, and reading them separately at a dozen call sites is how the
/// wrapped and unwrapped answers drift apart. `layout` is the pair resolved
/// against a document, which is what settles the width of the gutter.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TextView {
    pub view: EditorView,
    pub wrap: bool,
}

impl TextView {
    pub fn layout(self, line_count: usize) -> Layout {
        let width = self.view.text_width(line_count);
        let height = self.view.height as usize;
        if self.wrap {
            Layout::wrapping(height, width)
        } else {
            Layout::plain(height, width)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarMode {
    Explorer,
    Git,
}

#[derive(Debug)]
pub struct SidebarState {
    /// Which pane the sidebar shows. Both are always rendered in the MVP
    /// layout; the mode decides which one owns the sidebar's focus ring.
    pub mode: SidebarMode,
    /// The real project tree (SPEC §18). Its rows are what the explorer draws
    /// and what `selected` indexes into.
    pub tree: FileTree,
    pub selected: usize,
    pub scroll: usize,
}

impl SidebarState {
    pub fn rows(&self) -> &[TreeRow] {
        self.tree.rows()
    }

    /// The entry the explorer's selection is on, if the tree has any rows.
    pub fn selected_row(&self) -> Option<&TreeRow> {
        self.tree.row(self.selected)
    }

    /// Keeps the selection on a row and the row on screen.
    ///
    /// Called after anything that changes the tree or the selection, so
    /// "the selected row exists and is visible" is one rule in one place rather
    /// than a correction repeated in every command that touches the explorer.
    pub fn follow_selection(&mut self, height: usize) {
        let len = self.tree.len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(len - 1);
        if height == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        // A tree that shrank can leave the view scrolled past its end.
        self.scroll = self.scroll.min(len.saturating_sub(height));
    }
}

#[derive(Debug)]
pub struct MenuState {
    /// Index into `commands::MENUS`, or `None` when the menu bar is idle.
    pub open: Option<usize>,
    pub item: usize,
    /// Where focus goes when the menu closes.
    pub return_focus: FocusTarget,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            open: None,
            item: 0,
            return_focus: FocusTarget::Editor,
        }
    }
}

/// The last place the editor was clicked, for double-click detection.
///
/// Crossterm reports presses, not double clicks, so the second click of a pair
/// is recognised by where and when the first one landed. It is UI state rather
/// than document state, which is why it sits on `App` and not on `Tab`.
#[derive(Debug, Clone, Copy)]
pub struct LastClick {
    pub at: std::time::Instant,
    pub line: usize,
    pub col: usize,
}

pub struct App {
    pub workspace: Workspace,
    /// What the editor remembers between runs (SPEC §43). Loaded by `main`
    /// rather than by `new`, so nothing that builds an `App` in a test reads
    /// or writes the user's real config file.
    pub settings: Settings,
    /// Whether a settings change is written back to disk. Only the binary
    /// turns it on: a test that switches themes must not rewrite the config of
    /// whoever is running it.
    pub persist_settings: bool,
    /// Everything the tab strip holds, in strip order: files being edited and
    /// diffs being read (SPEC §11, §36).
    pub tabs: Vec<TabItem>,
    pub active_tab: Option<usize>,
    /// The tab the bar is scrolled to: the first one it tries to draw.
    ///
    /// A request rather than a fact — the layout still pulls the strip along
    /// far enough to keep the active tab whole, and back when the tabs after
    /// this one no longer fill the bar. Scrolling the wheel over the strip is
    /// what moves it, so a bar with twenty tabs on it can be read without
    /// switching to each of them in turn (SPEC §11).
    pub tab_scroll: usize,
    /// Width of the tab strip in the last drawn frame. Frame geometry, like
    /// `editor_view`: what the strip can show is what decides how far it has
    /// to be scrolled for the active tab to be whole.
    pub tab_bar_width: u16,
    pub sidebar: SidebarState,
    pub menu: MenuState,
    pub focus: FocusTarget,
    pub git: GitState,
    pub notifications: Notifications,
    pub clipboard: Clipboard,
    /// The find/replace bar (SPEC §22, §23). Closed until `Ctrl+F`.
    pub search: SearchState,
    /// The one modal window, when there is one. Nothing else receives input
    /// while it is open (SPEC §8, §40).
    pub dialog: Option<DialogState>,
    /// The help screen, when one is open (SPEC §6). Drawn over the whole body
    /// — the tab strip and the sidebar included — so it cannot outlive its own
    /// focus (ADR-038).
    pub help: Option<HelpState>,
    pub last_click: Option<LastClick>,
    /// Size of the editor pane in the last drawn frame.
    pub editor_view: EditorView,
    /// Rows the explorer panel could show in the last drawn frame. Frame
    /// geometry, like `editor_view`, and read for the same reason: scrolling
    /// has to know how big the window is (ADR-010).
    pub explorer_rows: u16,
    /// Rows the git panel could show in the last drawn frame, for the same
    /// reason as `explorer_rows`: the changed-file list can be longer than the
    /// four to ten rows the layout gives it.
    pub git_rows: u16,
    /// Rows the diff viewer could show in the last drawn frame, for the same
    /// reason as `git_rows`: a diff is longer than its pane and paging through
    /// it needs the pane's height.
    pub diff_rows: u16,
    /// Rows and columns the help screen could show in the last drawn frame.
    /// The width matters as well as the height here, because a note wraps: the
    /// lines are laid out against it, and so is the scroll.
    pub help_rows: u16,
    pub help_cols: u16,
    /// Rows the open dialog's list could show in the last drawn frame, for the
    /// same reason as `explorer_rows`: the browser's box is clamped to the
    /// terminal, and scrolling has to know how big the window really is.
    pub dialog_rows: u16,
    /// The run loop's event channel, kept so the one producer `App` restarts
    /// itself can be given one: the filesystem watcher, when the workspace
    /// root changes (ADR-051). `None` in every headless test, which is also
    /// what makes those tests spawn no threads.
    pub events: Option<Sender<AppEvent>>,
    /// The watch on the workspace root, while there is one. Held rather than
    /// forgotten because dropping it is how the previous root stops being
    /// watched.
    pub watch: Option<Watch>,
    pub should_quit: bool,
}

impl App {
    pub fn new(workspace: Workspace) -> Self {
        let tree = FileTree::new(workspace.root());
        Self {
            settings: Settings::default(),
            persist_settings: false,
            tabs: Vec::new(),
            active_tab: None,
            tab_scroll: 0,
            tab_bar_width: 0,
            sidebar: SidebarState {
                mode: SidebarMode::Explorer,
                tree,
                selected: 0,
                scroll: 0,
            },
            menu: MenuState::default(),
            focus: FocusTarget::Editor,
            git: GitState::default(),
            notifications: Notifications::default(),
            clipboard: Clipboard::default(),
            search: SearchState::default(),
            dialog: None,
            help: None,
            last_click: None,
            editor_view: EditorView::default(),
            explorer_rows: 0,
            git_rows: 0,
            diff_rows: 0,
            help_rows: 0,
            help_cols: 0,
            dialog_rows: 0,
            events: None,
            watch: None,
            should_quit: false,
            workspace,
        }
    }

    /// The editor pane, with the wrap setting that decides how a line is laid
    /// out in it. Everything that scrolls the pane or moves the cursor takes
    /// this rather than `editor_view` alone.
    pub fn text_view(&self) -> TextView {
        TextView {
            view: self.editor_view,
            wrap: self.settings.word_wrap,
        }
    }

    /// Whether the open dialog is one that types into a text field.
    ///
    /// The keyboard asks before resolving a key: in an input dialog `Left` and
    /// `Right` move a caret, and in a confirmation they move the button
    /// selection.
    pub fn dialog_wants_text(&self) -> bool {
        self.dialog.as_ref().is_some_and(|d| d.field().is_some())
    }

    /// The file being edited, when the tab in front is one. A diff tab answers
    /// `None`, which is what makes every editing command a no-op over it.
    pub fn active(&self) -> Option<&Tab> {
        self.active_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(TabItem::editor)
    }

    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        match self.active_tab {
            Some(index) => self.tabs.get_mut(index).and_then(TabItem::editor_mut),
            None => None,
        }
    }

    /// The editor tab at `index`, when that tab is one.
    pub fn editor_at(&self, index: usize) -> Option<&Tab> {
        self.tabs.get(index).and_then(TabItem::editor)
    }

    pub fn editor_at_mut(&mut self, index: usize) -> Option<&mut Tab> {
        self.tabs.get_mut(index).and_then(TabItem::editor_mut)
    }

    /// Every file being edited, with the tab index each one sits at.
    pub fn editors(&self) -> impl Iterator<Item = (usize, &Tab)> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(i, item)| item.editor().map(|tab| (i, tab)))
    }

    pub fn editors_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut().filter_map(TabItem::editor_mut)
    }

    /// The diff in front of the user, when the tab in front is one (SPEC §36).
    pub fn diff(&self) -> Option<&DiffState> {
        self.active_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(TabItem::diff)
    }

    pub fn diff_mut(&mut self) -> Option<&mut DiffState> {
        match self.active_tab {
            Some(index) => self.tabs.get_mut(index).and_then(TabItem::diff_mut),
            None => None,
        }
    }

    /// Keeps focus and the tab in front in step (SPEC §26).
    ///
    /// The two read-only panes are one focus target each, and which of them the
    /// tab strip is showing is not something every command that switches tabs
    /// should have to remember. So it is settled in one place, after each
    /// command: a diff tab in front means `Diff` has focus wherever `Editor`
    /// would have, and the other way round.
    pub fn normalize_focus(&mut self) {
        let showing_diff = self
            .active_tab
            .and_then(|i| self.tabs.get(i))
            .is_some_and(|item| item.diff().is_some());
        self.focus = match (self.focus, showing_diff) {
            (FocusTarget::Editor, true) => FocusTarget::Diff,
            (FocusTarget::Diff, false) => FocusTarget::Editor,
            (other, _) => other,
        };
    }

    /// Re-colours the active tab's viewport, and says so once if the document
    /// turns out to be one that cannot be highlighted.
    ///
    /// The run loop calls this before every draw. Only the active tab is
    /// synced: a background tab is not on screen, and its cache is still valid
    /// when it comes back because an edit it did not receive cannot have
    /// invalidated it.
    pub fn sync_highlight(&mut self) {
        let height = self.editor_view.height as usize;
        let Some(tab) = self.active_mut() else { return };
        let Some(why) = tab.sync_highlight(height) else {
            return;
        };
        let message = format!(
            "Syntax highlighting off for {}: {}",
            tab.document.title(),
            why.reason()
        );
        self.notifications.warning(message);
    }

    /// Finds the open query's hits again when anything they depend on has
    /// changed — the query, the options, the active tab, or the buffer.
    ///
    /// Called once a frame from the run loop, next to `sync_highlight` and for
    /// the same reason: a dozen commands can change the answer, and doing it
    /// here means none of them has to remember to.
    pub fn sync_search(&mut self) {
        if !self.search.open {
            return;
        }
        let Some(index) = self.active_tab.filter(|i| *i < self.tabs.len()) else {
            self.search.matches.clear();
            self.search.current = None;
            return;
        };
        let Some(tab) = self.editor_at(index) else {
            self.search.matches.clear();
            self.search.current = None;
            return;
        };
        let revision = tab.document.revision();
        if self.search.is_current(index, revision) {
            return;
        }
        let cursor = tab.document.cursor();
        let caret = (cursor.line, cursor.column);
        let (matches, truncated) = tab.document.find_all(
            &self.search.query.value,
            self.search.case_sensitive,
            search::MAX_MATCHES,
        );
        log::debug!(
            "search {:?}: {} hits{}",
            self.search.query.value,
            matches.len(),
            if truncated { " (truncated)" } else { "" }
        );
        self.search
            .set_matches(index, revision, matches, truncated, caret);
    }

    /// Makes `root` the workspace: the explorer, the git panel and the
    /// filesystem watcher all move to it (ADR-051).
    ///
    /// Open tabs are deliberately untouched. A tab is a buffer and a path, not
    /// a member of a directory, and closing files because the sidebar moved
    /// would be the editor throwing away work the user never asked it to.
    ///
    /// The watch is replaced rather than added to: the old one is dropped
    /// first, so a session that walks through five directories still has one
    /// watcher thread at the end of it.
    pub fn open_workspace(&mut self, root: &Path) {
        let workspace = Workspace::at(root);
        log::info!("workspace is now {}", workspace.root().display());
        self.sidebar.tree = FileTree::new(workspace.root());
        self.sidebar.selected = 0;
        self.sidebar.scroll = 0;
        self.git.discover(workspace.root());
        self.watch = None;
        if let Some(events) = self.events.clone() {
            match crate::filesystem::watcher::spawn(workspace.root(), events) {
                Ok(watch) => self.watch = Some(watch),
                Err(err) => {
                    log::warn!("no filesystem watcher: {err}");
                    self.notifications.warning(format!(
                        "Not watching for outside changes: {err} — F5 refreshes"
                    ));
                }
            }
        }
        self.workspace = workspace;
    }

    /// Opens a file in a tab and focuses it, optionally on a given line.
    ///
    /// A path with nothing behind it opens as an empty buffer that will create
    /// the file when it is saved, which is what the CLI relies on; the
    /// explorer's New File writes the file first and then opens it.
    ///
    /// A file that is already open is re-focused rather than opened twice
    /// (SPEC §11), which also means the CLI, the explorer and the menu can all
    /// call this without checking first.
    pub fn open_path(&mut self, path: &Path, line: Option<usize>) -> Result<(), DocumentError> {
        let absolute = absolute(path);
        let existing = self.tabs.iter().position(|tab| tab.is_at(&absolute));

        let index = match existing {
            Some(index) => index,
            None => {
                self.tabs
                    .push(TabItem::editing(Tab::new(Document::open_or_create(
                        &absolute,
                    )?)));
                self.tabs.len() - 1
            }
        };
        self.active_tab = Some(index);
        self.focus = FocusTarget::Editor;
        let view = self.text_view();
        if let Some(tab) = self.editor_at_mut(index) {
            if let Some(line) = line {
                tab.document.goto_line(line);
            }
            tab.follow_cursor(view);
        }
        Ok(())
    }
}

/// Test fixtures. Three tabs of in-memory text, shared by the `ui`, `event` and
/// `commands` test modules so they all exercise the same shapes: a clean file,
/// a file with unsaved changes, and a second language.
#[cfg(test)]
impl App {
    /// The editor tab at `index`, for the tests that know there is one there.
    ///
    /// Production code goes through `editor_at`, which answers `None` for a
    /// diff tab; a test that indexed a tab it did not open has a bug, and the
    /// panic here is the report.
    pub fn tab_mut(&mut self, index: usize) -> &mut Tab {
        self.editor_at_mut(index)
            .unwrap_or_else(|| panic!("tab {index} is not a file being edited"))
    }

    pub fn fixture() -> Self {
        let workspace = Workspace::from_arg(Some(Path::new("/tmp/ferroedit-test")))
            .expect("the fixture workspace path is valid");
        let mut app = Self::new(workspace);
        app.tabs = vec![
            TabItem::editing(Tab::scratch(
                "main.rs",
                "fn main() {\n    println!(\"hello\");\n}",
            )),
            TabItem::editing(Tab::scratch("editor.rs", "// editor.rs")),
            TabItem::editing(Tab::scratch("README.md", "# FerroEdit")),
        ];
        // An edit that cancels itself out still leaves the tab dirty, which is
        // what the tab bar's unsaved marker is drawn from.
        app.editor_at_mut(1).unwrap().document.insert_char(' ');
        app.editor_at_mut(1).unwrap().document.backspace();
        // Nothing in a test may write an escape sequence to the runner's stdout.
        app.clipboard = crate::editor::clipboard::Clipboard::detached();
        // A status that was never read, in a fixture that never runs git: the
        // panel's own tests want something to draw (see `GitState::fixture`).
        app.git = GitState::fixture();
        app.git_rows = 4;
        app.active_tab = Some(0);
        app.editor_view = EditorView {
            width: 80,
            height: 24,
        };
        app
    }

    /// A fixture over a real directory, for everything that reads the tree.
    ///
    /// The three scratch tabs are gone: an explorer test wants the files it
    /// created on disk and nothing else, and `open_path` is what puts them in
    /// tabs.
    pub fn fixture_in(root: &Path) -> Self {
        let workspace = Workspace::from_arg(Some(root)).expect("a real directory");
        let mut app = Self::new(workspace);
        app.clipboard = crate::editor::clipboard::Clipboard::detached();
        app.focus = FocusTarget::Explorer;
        app.sidebar.mode = SidebarMode::Explorer;
        app.editor_view = EditorView {
            width: 80,
            height: 24,
        };
        app.explorer_rows = 20;
        app
    }

    /// `count` clean scratch tabs, for the cases that are only interesting once
    /// the tab bar has more tabs than it can show (SPEC §11, Phase 5
    /// acceptance).
    pub fn fixture_with_tabs(count: usize) -> Self {
        let mut app = Self::fixture();
        app.tabs = (0..count)
            .map(|i| TabItem::editing(Tab::scratch(&format!("file{i:02}.rs"), "// x")))
            .collect();
        app.active_tab = (count > 0).then_some(0);
        app
    }
}

/// Resolves a path to an absolute one, existing or not.
///
/// Tabs are matched by path, so `./main.rs` and `/home/me/main.rs` have to end
/// up as the same string or the same file opens twice. `canonicalize` refuses a
/// path that is not there yet, which is exactly the case a new file is in, so
/// the fallback joins the working directory by hand.
pub(crate) fn absolute(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    match (path.is_absolute(), std::env::current_dir()) {
        (false, Ok(cwd)) => cwd.join(path),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_app_has_no_tabs_until_a_file_is_opened() {
        let app = App::new(Workspace::from_arg(Some(Path::new("/tmp"))).unwrap());
        assert!(app.tabs.is_empty());
        assert_eq!(app.active_tab, None);
        assert!(app.active().is_none());
    }

    #[test]
    fn opening_a_file_makes_it_the_active_tab() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, None).unwrap();

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab, Some(0));
        assert_eq!(app.active().unwrap().document.title(), "hello.rs");
        assert_eq!(app.focus, FocusTarget::Editor);
    }

    #[test]
    fn opening_an_already_open_file_reuses_its_tab() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.txt");
        let two = dir.path().join("two.txt");
        std::fs::write(&one, "one").unwrap();
        std::fs::write(&two, "two").unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&one, None).unwrap();
        app.open_path(&two, None).unwrap();
        app.open_path(&one, None).unwrap();

        assert_eq!(app.tabs.len(), 2, "no duplicate tab (SPEC §11)");
        assert_eq!(app.active_tab, Some(0));
    }

    #[test]
    fn the_line_argument_places_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lines.txt");
        std::fs::write(&path, "1\n2\n3\n4\n5\n").unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, Some(4)).unwrap();
        assert_eq!(app.active().unwrap().document.cursor().line, 3);
    }

    #[test]
    fn opening_a_path_that_does_not_exist_starts_an_empty_buffer_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, None).unwrap();

        assert_eq!(app.active().unwrap().document.title(), "new.txt");
        assert!(
            !path.exists(),
            "the file appears when it is saved, not before"
        );
    }

    #[test]
    fn opening_a_file_that_cannot_be_read_opens_no_tab() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        // A directory is unreadable as a document, and unlike a missing file it
        // is not something to start a buffer over.
        assert!(app.open_path(dir.path(), None).is_err());
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn a_relative_and_an_absolute_path_are_the_same_tab() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one.txt");
        std::fs::write(&path, "one").unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, None).unwrap();
        app.open_path(&std::fs::canonicalize(&path).unwrap(), None)
            .unwrap();
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn the_gutter_is_taken_out_of_the_text_width() {
        let view = EditorView {
            width: 80,
            height: 24,
        };
        assert_eq!(view.text_width(10), 76, "two digits plus two of padding");
        assert_eq!(view.text_width(1000), 74);
        // A pane narrower than its own gutter must not underflow.
        assert_eq!(
            EditorView {
                width: 2,
                height: 1
            }
            .text_width(1),
            0
        );
    }
}
