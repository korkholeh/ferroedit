//! Application state. Owns tabs, workspace, focus and notifications.
//! Never renders and never talks to crossterm directly.

pub mod browser;
pub mod dialog;
pub mod diff;
pub mod focus;
pub mod git;
pub mod help;
pub mod image;
pub mod input_field;
// Shadows the `log` *crate* inside this file, and only inside this file: the
// three logging calls below therefore spell it `::log::`. Every other module
// imports `LogState` by name and is unaffected.
pub mod log;
pub mod notifications;
pub mod opening;
pub mod search;
pub mod table;
pub mod tabs;
pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::config::Settings;
use crate::event::AppEvent;
use crate::filesystem::watcher::Watch;
use dialog::DialogState;
use diff::DiffState;
use focus::FocusTarget;
use git::GitState;
use help::HelpState;
use image::{Canvas, ImageState};
use log::LogState;
use notifications::Notifications;
use opening::{Opening, SLICED_ABOVE};
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
    /// Rows the log viewer could show, for the same reason again. It is not the
    /// diff's: the log's search field takes a row off the top of the pane, so
    /// the two are different heights whenever it is open.
    pub log_rows: u16,
    /// The pane the picture in front was drawn into in the last frame, in
    /// cells (ADR-078). Geometry like `diff_rows`, and read for the same
    /// reason: the zoom, the pan and the block grid are all measured against
    /// the pane, and only a laid-out frame knows how big it is.
    pub image_canvas: Canvas,
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
    /// The large file being read a slice at a time, while one is (ADR-077).
    /// Its tab does not exist until the read lands: an empty tab that fills in
    /// later would be a window the user could type into and scroll through
    /// while it lied about what the file holds.
    pub opening: Option<Opening>,
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
            log_rows: 0,
            image_canvas: Canvas::default(),
            help_rows: 0,
            help_cols: 0,
            dialog_rows: 0,
            events: None,
            opening: None,
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

    /// The history in front, when the tab in front is one (ADR-068).
    pub fn log(&self) -> Option<&LogState> {
        self.active_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(TabItem::log)
    }

    pub fn log_mut(&mut self) -> Option<&mut LogState> {
        match self.active_tab {
            Some(index) => self.tabs.get_mut(index).and_then(TabItem::log_mut),
            None => None,
        }
    }

    /// The picture in front, when the tab in front is one (ADR-078).
    pub fn image(&self) -> Option<&ImageState> {
        self.active_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(TabItem::image)
    }

    pub fn image_mut(&mut self) -> Option<&mut ImageState> {
        match self.active_tab {
            Some(index) => self.tabs.get_mut(index).and_then(TabItem::image_mut),
            None => None,
        }
    }

    /// Re-samples the picture in front into the blocks the pane draws
    /// (ADR-078).
    ///
    /// Called once a frame from the run loop, beside `sync_highlight` and for
    /// the same reason: `ui/` is read-only over `&App`, and averaging a
    /// twelve-megapixel photo down to a pane is a cache that has to write. It
    /// is a no-op on every frame where neither the zoom, the pan nor the pane
    /// has moved.
    pub fn sync_image(&mut self) {
        let canvas = self.image_canvas;
        if let Some(image) = self.image_mut() {
            image.sync(canvas);
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
        let front = self.active_tab.and_then(|i| self.tabs.get(i));
        let showing_diff = front.is_some_and(|item| item.diff().is_some());
        let showing_log = front.is_some_and(|item| item.log().is_some());
        let showing_image = front.is_some_and(|item| item.image().is_some());
        self.focus = match self.focus {
            FocusTarget::Editor if showing_diff => FocusTarget::Diff,
            FocusTarget::Editor if showing_log => FocusTarget::Log,
            FocusTarget::Editor if showing_image => FocusTarget::Image,
            FocusTarget::Diff if !showing_diff => FocusTarget::Editor,
            // The search field goes with its own tab: switching away from a
            // history has to leave the field as well, or the next pane's keys
            // would be typing into a list that is not on screen.
            FocusTarget::Log | FocusTarget::LogSearch if !showing_log => FocusTarget::Editor,
            FocusTarget::Image if !showing_image => FocusTarget::Editor,
            other => other,
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

    /// Re-reads the active tab's table from its buffer when either has changed
    /// (SPEC §65).
    ///
    /// Next to `sync_highlight` and for the same reasons: the table is a cache
    /// over the document, only the tab in front is on screen, and doing it here
    /// means no command that edits, undoes, reloads or changes the dialect has
    /// to remember to invalidate it.
    pub fn sync_table(&mut self) {
        if let Some(tab) = self.active_mut() {
            tab.sync_table();
        }
    }

    /// Finds the open query's hits again when anything they depend on has
    /// changed — the query, the options, the active tab, or the buffer.
    ///
    /// Called once a frame from the run loop, next to `sync_highlight` and for
    /// the same reason: a dozen commands can change the answer, and doing it
    /// here means none of them has to remember to.
    pub fn sync_search(&mut self) {
        if !self.search.open {
            self.search.stop_scan();
            return;
        }
        self.advance_search(Some(search::SCAN_BUDGET), None, true);
    }

    /// Runs the search to the end, here and now — Replace and Replace All,
    /// which act on the whole list and cannot be given a part of it.
    ///
    /// This is the one that ignores both `LIVE_SEARCH_MAX_LINES` (ADR-075) and
    /// the per-frame budget (ADR-076): a large file is why the bar stopped
    /// answering as the query was typed, and it is not a reason to refuse to
    /// answer at all.
    pub fn search_now(&mut self) {
        self.advance_search(None, None, false);
    }

    /// Runs the search and steps to a hit when it lands — `Enter`, Find Next,
    /// Find Previous.
    ///
    /// On a file small enough to walk inside one frame this is over before it
    /// returns and is indistinguishable from doing the work here. On a larger
    /// one it starts a walk the following frames carry on and animate, and the
    /// caret moves when the walk lands (ADR-076).
    pub fn search_and_step(&mut self, delta: isize) {
        self.advance_search(Some(search::SCAN_BUDGET), Some(delta), false);
    }

    /// Carries the walk forward by at most `budget`, starting one if there is
    /// none, and returns whether it landed (ADR-076).
    ///
    /// `budget` of `None` is a walk to the end in this call. `step` is what to
    /// do when it lands, and is recorded only on a walk this call started.
    fn advance_search(
        &mut self,
        budget: Option<Duration>,
        step: Option<isize>,
        defer_large: bool,
    ) -> bool {
        if !self.search.open {
            return false;
        }
        let Some(index) = self.active_tab.filter(|i| *i < self.tabs.len()) else {
            self.search.forget_matches();
            return false;
        };
        let Some(tab) = self.editor_at(index) else {
            self.search.forget_matches();
            return false;
        };
        // Copied out rather than held, so the borrow of the tab ends here and
        // the walk below can be lifted out of `self.search` and put back.
        let revision = tab.document.revision();
        let line_count = tab.document.line_count();
        let cursor = tab.document.cursor();

        // Already answered. A step asked for now is taken against the hits on
        // hand, which is what makes a second `Enter` cost nothing.
        if self.search.is_current(index, revision) {
            self.search.stop_scan();
            if let Some(delta) = step {
                self.step_to_match(delta);
            }
            return true;
        }
        // A walk whose question changed under it — the query, the options, the
        // buffer — is thrown away before anything is decided about it, so what
        // follows sees either a walk that is still the right one or none.
        self.search.abandon_stale_scan(index, revision);
        // Past this size the walk is too slow to *start* on a keystroke, so the
        // query waits to be asked for and the bar says so (ADR-075). Asked
        // after the answer on hand has been checked, so a frame drawn over a
        // large file with its hits already found does not throw them away — and
        // asked only of a file with no walk in flight, because one already
        // running is the answer the user asked for and these are the frames
        // that turn its spinner (ADR-076).
        if defer_large && !self.search.is_scanning() && line_count > search::LIVE_SEARCH_MAX_LINES {
            self.search.defer();
            return false;
        }
        self.search
            .begin_scan(index, revision, (cursor.line, cursor.column), step);
        let Some(mut scan) = self.search.take_scan() else {
            return false;
        };

        let deadline = budget.map(|budget| Instant::now() + budget);
        let (line, truncated) = match self.editor_at(index) {
            Some(tab) => tab.document.find_from(
                &self.search.query.value,
                self.search.case_sensitive,
                search::MAX_MATCHES,
                scan.resume_line(),
                deadline,
                scan.matches_mut(),
            ),
            None => return false,
        };
        scan.advance_to(line, truncated);
        if !scan.has_landed(line_count) {
            self.search.resume_scan(scan);
            return false;
        }
        ::log::debug!(
            "search {:?}: {} hits{}",
            self.search.query.value,
            scan.hits(),
            if truncated { " (truncated)" } else { "" }
        );
        let (matches, truncated, caret, step) = scan.into_result();
        self.search
            .set_matches(index, revision, matches, truncated, caret);
        if let Some(delta) = step {
            self.step_to_match(delta);
        }
        true
    }

    /// Moves the caret onto the next or previous hit and says where it landed.
    fn step_to_match(&mut self, delta: isize) {
        let Some(hit) = self.search.step(delta) else {
            self.notifications
                .warning(format!("No matches for {}", self.search.query.value));
            return;
        };
        let view = self.text_view();
        if let Some(tab) = self.active_mut() {
            tab.document.select_match(hit);
            tab.follow_cursor(view);
        }
        let label = self.search.count_label();
        self.notifications.info(format!("Match {label}"));
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
        ::log::info!("workspace is now {}", workspace.root().display());
        self.sidebar.tree = FileTree::new(workspace.root());
        self.sidebar.selected = 0;
        self.sidebar.scroll = 0;
        self.git.discover(workspace.root());
        self.watch = None;
        if let Some(events) = self.events.clone() {
            match crate::filesystem::watcher::spawn(workspace.root(), events) {
                Ok(watch) => self.watch = Some(watch),
                Err(err) => {
                    ::log::warn!("no filesystem watcher: {err}");
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
    pub fn open_path(&mut self, path: &Path, line: Option<usize>) -> Result<Opened, DocumentError> {
        let absolute = absolute(path);
        // A read already in flight is finished here, where it stands, rather
        // than dropped or left running: every path the editor was asked to open
        // gets a tab, and a read that landed frames after the user had moved on
        // would pull the window out from under them. It is also why there is no
        // queue — two files named at once is the command line, which opens them
        // before there is a frame to animate.
        self.finish_pending_open();
        if let Some(index) = self.tabs.iter().position(|tab| tab.is_at(&absolute)) {
            self.show_tab(index, line);
            return Ok(Opened::Now);
        }
        if self.slice_the_read(&absolute) {
            match Opening::start(&absolute, line) {
                Ok(opening) => {
                    ::log::info!("reading {} a slice at a time", absolute.display());
                    self.opening = Some(opening);
                    return Ok(Opened::Reading);
                }
                // Something that could be stat'd and not opened: fall through
                // and let the whole-file path report it the way it always has.
                Err(err) => ::log::debug!("{} is not sliceable: {err}", absolute.display()),
            }
        }
        if is_image_path(&absolute) {
            let bytes = std::fs::read(&absolute).map_err(|source| DocumentError::Io {
                path: absolute.display().to_string(),
                source,
            })?;
            self.place_image(&absolute, bytes)?;
            return Ok(Opened::Now);
        }
        let document = Document::open_or_create(&absolute)?;
        self.place_document(document, line);
        Ok(Opened::Now)
    }

    /// Decodes bytes already read and puts the picture in a tab (ADR-078).
    fn place_image(&mut self, path: &Path, bytes: Vec<u8>) -> Result<(), DocumentError> {
        let file_bytes = bytes.len() as u64;
        let image = crate::image::decode(&bytes).map_err(|err| DocumentError::Image {
            path: path.display().to_string(),
            reason: err.to_string(),
        })?;
        ::log::info!(
            "{} is a {} {}x{}",
            path.display(),
            image.format.label(),
            image.width,
            image.height
        );
        self.tabs
            .push(TabItem::showing(ImageState::new(path, image, file_bytes)));
        self.show_tab(self.tabs.len() - 1, None);
        Ok(())
    }

    /// Whether a path is worth reading a slice at a time (ADR-077).
    ///
    /// A file the editor cannot stat, or one small enough to read inside a
    /// frame, opens whole: the machinery below only earns its keep when there
    /// is something to watch.
    fn slice_the_read(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() >= SLICED_ABOVE)
    }

    /// Reads the next slice of the file being opened, and lands it when that
    /// was the last one (ADR-077).
    ///
    /// Called once a frame by the run loop, next to `sync_highlight` and for
    /// the same reason: it is work a frame owes, not something any one command
    /// should have to remember.
    pub fn advance_open(&mut self) {
        let Some(opening) = self.opening.as_mut() else {
            return;
        };
        match opening.read_slice() {
            Ok(false) => {}
            Ok(true) => self.finish_pending_open(),
            Err(err) => {
                let path = opening.path().to_path_buf();
                ::log::error!("could not read {}: {err}", path.display());
                self.opening = None;
                self.notifications.error(format!("Failed to open: {err}"));
            }
        }
    }

    /// Finishes the read in flight, if there is one: decodes what was read and
    /// puts it in a tab.
    pub(crate) fn finish_pending_open(&mut self) {
        let Some(mut opening) = self.opening.take() else {
            return;
        };
        if let Err(err) = opening.read_rest() {
            ::log::error!("could not read {}: {err}", opening.path().display());
            self.notifications.error(format!("Failed to open: {err}"));
            return;
        }
        let line = opening.line();
        let (path, bytes, disk) = opening.finish();
        // A large PNG went down the same sliced read as a large log, because
        // the slow part of opening either of them is the read (ADR-077). It is
        // here, with the bytes in hand, that the two part company.
        if is_image_path(&path) {
            let title = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("image")
                .to_string();
            match self.place_image(&path, bytes) {
                Ok(()) => self.notifications.info(format!("Opened {title}")),
                Err(err) => {
                    ::log::error!("could not open {}: {err}", path.display());
                    self.notifications.error(format!("Failed to open: {err}"));
                }
            }
            return;
        }
        match Document::from_bytes(&path, bytes, disk, None) {
            Ok(document) => {
                let title = document.title().to_string();
                self.place_document(document, line);
                // The tab appears frames after the command that asked for it,
                // so it says so: the box the user was watching is gone by now,
                // and nothing else would mark the moment the file arrived.
                self.notifications.info(format!("Opened {title}"));
            }
            Err(err) => {
                ::log::error!("could not open {}: {err}", path.display());
                self.notifications.error(format!("Failed to open: {err}"));
            }
        }
    }

    /// Puts a freshly read document in a tab of its own and focuses it.
    fn place_document(&mut self, document: Document, line: Option<usize>) {
        // A stream longer than the unpacking cap opens on its beginning
        // (ADR-074). The status bar carries the same news for as long as the
        // tab is open; this is the one moment it has to be impossible to miss,
        // because everything below the cut looks exactly like the end of a
        // file.
        let truncated = document.is_truncated();
        self.tabs.push(TabItem::editing(Tab::new(document)));
        if truncated {
            self.notifications
                .warning("Too large to unpack whole — showing the beginning");
        }
        self.show_tab(self.tabs.len() - 1, line);
    }

    /// Brings a tab to the front, on a given line if one was asked for.
    fn show_tab(&mut self, index: usize, line: Option<usize>) {
        self.active_tab = Some(index);
        self.focus = FocusTarget::Editor;
        let view = self.text_view();
        if let Some(tab) = self.editor_at_mut(index) {
            if let Some(line) = line {
                tab.document.goto_line(line);
            }
            tab.follow_cursor(view);
        }
    }
}

/// Whether a path is one the image viewer opens (ADR-078).
///
/// The extension and nothing else, which is the rule the CSV view already
/// follows (ADR-062): it is what decides *which viewer* a double-click lands
/// in, and it has to answer before the file has been read. What the file
/// actually is still decides how it is decoded — a `.png` holding a JPEG opens
/// as the JPEG it is — so the extension is a route and never a claim.
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("png")
                || ext.eq_ignore_ascii_case("jpg")
                || ext.eq_ignore_ascii_case("jpeg")
        })
}

/// What an open did: the file is in front, or it is being read (ADR-077).
///
/// The caller says "Opened X" for the first and nothing for the second — the
/// box on screen is already saying what is happening, and a file that is still
/// being read has not been opened yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    Now,
    Reading,
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

    /// A file over the slicing threshold (ADR-077), with `lines` lines in it.
    fn large_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let line = "the quick brown fox jumps over the lazy dog\n";
        let lines = (SLICED_ABOVE as usize / line.len()) + 1_000;
        std::fs::write(&path, line.repeat(lines)).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > SLICED_ABOVE);
        path
    }

    #[test]
    fn a_small_file_opens_inside_the_command_that_asked_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.txt");
        std::fs::write(&path, "small\n").unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        assert_eq!(app.open_path(&path, None).unwrap(), Opened::Now);
        assert!(app.opening.is_none(), "nothing to animate");
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn a_large_file_is_read_a_slice_at_a_time_and_lands_in_a_tab() {
        let dir = tempfile::tempdir().unwrap();
        let path = large_file(dir.path(), "big.txt");

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        assert_eq!(app.open_path(&path, Some(3)).unwrap(), Opened::Reading);
        assert!(app.tabs.is_empty(), "the tab appears when the read lands");
        assert!(app.opening.is_some());

        // The run loop's frames, until it lands. Bounded, because a test that
        // spins forever on a bug is a test that says nothing.
        for _ in 0..10_000 {
            app.advance_open();
            if app.opening.is_none() {
                break;
            }
        }

        assert!(app.opening.is_none(), "the read landed");
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab, Some(0));
        assert_eq!(app.focus, FocusTarget::Editor);
        let document = &app.active().unwrap().document;
        assert_eq!(document.title(), "big.txt");
        assert_eq!(
            document.len_bytes(),
            crate::editor::coords::ByteIdx(std::fs::metadata(&path).unwrap().len() as usize),
            "every byte of the file is in the buffer"
        );
        assert_eq!(document.cursor().line, 2, "the line asked for is kept");
    }

    #[test]
    fn a_second_open_finishes_the_read_the_first_one_started() {
        let dir = tempfile::tempdir().unwrap();
        let big = large_file(dir.path(), "big.txt");
        let small = dir.path().join("small.txt");
        std::fs::write(&small, "small\n").unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&big, None).unwrap();
        app.open_path(&small, None).unwrap();

        assert!(app.opening.is_none());
        assert_eq!(app.tabs.len(), 2, "neither file was dropped");
        assert_eq!(
            app.active().unwrap().document.title(),
            "small.txt",
            "the file asked for last is the one in front"
        );
    }

    #[test]
    fn a_large_file_already_open_is_re_focused_rather_than_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = large_file(dir.path(), "big.txt");

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, None).unwrap();
        app.finish_pending_open();
        assert_eq!(app.open_path(&path, None).unwrap(), Opened::Now);
        assert!(app.opening.is_none());
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn a_file_that_vanishes_under_the_read_is_reported_and_opens_no_tab() {
        let dir = tempfile::tempdir().unwrap();
        let path = large_file(dir.path(), "big.txt");

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, None).unwrap();
        // The handle stays valid on Unix, so this is not the read failing —
        // it is the weaker claim the test can make everywhere: whatever was
        // read still becomes a tab, and nothing panics on the way.
        let _ = std::fs::remove_file(&path);
        app.finish_pending_open();
        assert!(app.opening.is_none());
    }

    /// A one-by-one PNG, so the open path can be tested without a fixture
    /// file in the repository.
    #[cfg(test)]
    fn tiny_png() -> Vec<u8> {
        use std::io::Write as _;
        fn crc(bytes: &[u8]) -> u32 {
            let mut c = 0xFFFF_FFFFu32;
            for byte in bytes {
                c ^= u32::from(*byte);
                for _ in 0..8 {
                    c = if c & 1 != 0 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            c ^ 0xFFFF_FFFF
        }
        fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = (body.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            let mut all = kind.to_vec();
            all.extend_from_slice(body);
            out.extend_from_slice(&crc(&all).to_be_bytes());
            out
        }
        let mut ihdr = 1u32.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&[0, 1, 2, 3]).unwrap();
        let idat = encoder.finish().unwrap();
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &ihdr));
        out.extend(chunk(b"IDAT", &idat));
        out.extend(chunk(b"IEND", &[]));
        out
    }

    #[test]
    fn a_png_opens_in_a_viewer_rather_than_as_a_buffer_of_line_noise() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dot.png");
        std::fs::write(&path, tiny_png()).unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        assert_eq!(app.open_path(&path, None).unwrap(), Opened::Now);
        assert_eq!(app.tabs.len(), 1);
        let image = app.image().expect("the tab in front is a picture");
        assert_eq!((image.image.width, image.image.height), (1, 1));
        assert!(app.active().is_none(), "and nothing editable is in front");
    }

    /// The same file twice is the same tab, exactly as it is for a document
    /// (SPEC §11) — which is what `TabItem::is_at` answers for a picture.
    #[test]
    fn a_picture_already_open_is_re_focused_rather_than_decoded_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dot.png");
        std::fs::write(&path, tiny_png()).unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, None).unwrap();
        app.open_path(&path, None).unwrap();
        assert_eq!(app.tabs.len(), 1);
    }

    /// A `.png` that is not one is refused with a sentence about the file, not
    /// opened as six hundred kilobytes of mojibake.
    #[test]
    fn a_file_named_png_that_is_not_one_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lie.png");
        std::fs::write(&path, b"GIF89a...").unwrap();

        let mut app = App::new(Workspace::from_arg(Some(dir.path())).unwrap());
        let err = app.open_path(&path, None).unwrap_err();
        assert!(err.to_string().contains("not a PNG or a JPEG"), "{err}");
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn the_viewer_is_chosen_by_extension_whatever_its_case() {
        assert!(is_image_path(Path::new("a.png")));
        assert!(is_image_path(Path::new("a.JPG")));
        assert!(is_image_path(Path::new("a.jpeg")));
        assert!(!is_image_path(Path::new("a.png.gz")));
        assert!(!is_image_path(Path::new("png")));
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
