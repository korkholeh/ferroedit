//! The single `Command` enum produced by keyboard, menu, mouse and dialogs.
//!
//! Nothing else may mutate `App` (see `docs/ARCHITECTURE.md` invariant 3). The
//! menu table lives here rather than in `ui/` because it *is* command data: the
//! renderer reads it, and so does `execute_command`.

pub mod execute;

use std::path::PathBuf;

use crate::app::focus::FocusTarget;
use crate::app::search::SearchField;
use crate::app::App;
use crate::config::ThemeKind;
use crate::editor::cursor::Motion;
use crate::editor::document::LineEnding;
use crate::filesystem::watcher::FsChange;
use crate::git::JobOutcome;
use crate::update::UpdateCheck;

/// An operation that needs a name or a path typed before it can run
/// (SPEC §20, §40).
///
/// The text is not part of it: the operation is decided when the dialog opens
/// and the name only exists once the user has typed one, so `SubmitInput`
/// carries the operation and `ApplyFileOp` carries both.
///
/// Phase 9 added the one that takes a whole path rather than a name — Save As.
/// It is here rather than in a second enum because the dialog that asks for it
/// is the same dialog: a prompt, a field, and a button that submits whatever is
/// in it. (Open was one of these too, until it became a browser — ADR-051.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    CreateFile {
        parent: PathBuf,
    },
    CreateDirectory {
        parent: PathBuf,
    },
    Rename {
        path: PathBuf,
    },
    /// Writes the tab at `index` to the typed path, resolved against `base`,
    /// and keeps editing it under the new name.
    SaveAs {
        index: usize,
        base: PathBuf,
    },
}

impl FileOp {
    /// What the status bar says once it has happened.
    ///
    /// Save As never reaches it — it reports through the path that already
    /// says "Saved …" — but the match is exhaustive so that a new operation
    /// cannot be added without an answer here.
    pub fn past_tense(&self) -> &'static str {
        match self {
            Self::CreateFile { .. } => "Created",
            Self::CreateDirectory { .. } => "Created directory",
            Self::Rename { .. } => "Renamed to",
            Self::SaveAs { .. } => "Saved",
        }
    }
}

/// Deliberately `Clone` and not `Copy`: `InsertText` carries the pasted text,
/// and a paste of a megabyte must be moved rather than silently copied on every
/// match arm it passes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Asks about each unsaved tab in turn; the last answer is what ends the
    /// run (ADR-047).
    Quit,
    /// One answer of that walk: save this tab, close it, and ask about the
    /// next. A save that fails stops the quit rather than losing the file.
    SaveAndQuit(usize),
    /// The other: close this tab without saving, and ask about the next.
    DiscardAndQuit(usize),

    FocusPane(FocusTarget),
    CycleFocus,

    ToggleSidebarMode,
    MoveSidebarSelection(i16),
    ScrollSidebar(i16),

    /// Opens the selected file, or expands and collapses the selected
    /// directory — Enter, and a click on a row.
    ExplorerActivate,
    /// The same for a given row: the mouse selects and acts in one click.
    ExplorerActivateRow(usize),
    /// Expands the selected directory, or steps into an already open one.
    ExplorerExpand,
    /// Collapses the selected directory, or steps out to its parent.
    ExplorerCollapse,
    /// Re-reads every open directory from disk.
    ExplorerRefresh,
    /// Shows or hides the files `ignore` filters out. They are shown by
    /// default, so this hides them first (SPEC §19).
    ToggleHiddenFiles,

    /// Looks for the repository again and re-reads `git status` (SPEC §30).
    /// Everything that changes a file refreshes the status on its own; this is
    /// for a change made outside the editor.
    GitRefresh,
    /// Opens the file the git panel's selection is on.
    GitOpenSelected,
    /// Selects a row of the git panel and opens its diff — what a click on a
    /// changed file does (SPEC §31, §36).
    GitDiffRow(usize),
    /// Stages or unstages the selected file (SPEC §31). Both run on the worker,
    /// like every other command that writes to the repository (ADR-033).
    GitStage,
    GitUnstage,
    /// Stages the selected file when anything about it is unstaged, and
    /// unstages it when it is not — the one key that does the obvious thing.
    GitToggleStage,
    GitStageAll,
    GitUnstageAll,
    /// Asks for a commit message (SPEC §32). `GitCommit` is what commits.
    GitCommitPrompt,
    /// Runs the commit with whatever the open dialog holds. Only the dialog's
    /// button carries this; activating it turns it into `GitCommit`.
    SubmitCommit,
    GitCommit(String),
    GitPull,
    GitPush,
    /// Stops everything the worker has outstanding (ADR-044): the job in git's
    /// hands is killed, and the ones queued behind it are answered without
    /// being run.
    GitCancel,
    /// Opens the branch picker (SPEC §33). `GitSwitchBranch` is what moves
    /// `HEAD`.
    GitBranchPrompt,
    GitSwitchBranch(String),
    /// Asks for the name of a branch to create at `HEAD`. Reached from the Git
    /// menu and from the picker's own New… button.
    GitNewBranchPrompt,
    /// Runs the branch creation with whatever the open dialog holds; only the
    /// dialog's button carries it.
    SubmitBranch,
    GitCreateBranch(String),
    /// Opens the merge picker (SPEC §35). `GitMerge` is what merges.
    GitMergePrompt,
    GitMerge(String),
    /// Stages the selected file even though it is conflicted — the confirm
    /// dialog's second button, and the gesture that tells git a conflict is
    /// resolved (ADR-036).
    GitStageResolved,
    /// Opens the diff viewer on the file the git panel's selection is on, or
    /// on the file being edited when the editor has focus (SPEC §36).
    GitDiff,
    /// Shows the other side of the same file — staged instead of unstaged, or
    /// the other way round.
    GitDiffToggleSide,
    /// Re-runs the diff the viewer is showing.
    DiffRefresh,
    /// Closes the viewer and gives the pane behind it its focus back.
    DiffClose,
    /// Scrolls the viewer by lines, and by whole panes.
    DiffScroll(i16),
    DiffScrollPage(i16),
    /// Moves the viewer's window sideways, for the lines that are wider than
    /// the pane.
    DiffScrollHorizontal(i16),
    DiffHome,
    DiffEnd,

    /// Opens the repository's own history in a tab of its own (ADR-068).
    GitLog,
    /// The history of one file: the one in front of the editor, or the row the
    /// git panel's selection is on when the panel has focus.
    GitFileHistory,
    /// The history of the lines the editor's selection covers — the caret's own
    /// line when there is no selection (ADR-068).
    GitLineHistory,
    /// Re-reads the history the viewer is showing, search and all.
    LogRefresh,
    /// Closes the viewer and gives the pane behind it its focus back.
    LogClose,
    /// Moves the selection by rows, and by whole panes.
    LogMove(i16),
    LogMovePage(i16),
    LogHome,
    LogEnd,
    /// Selects the row `n` cells down the pane and opens its commit — what a
    /// click on a history does, the way a click on a changed file in the git
    /// panel opens that file's diff (ADR-068).
    LogShowRow(usize),
    /// Scrolls the list by moving the selection — the wheel.
    LogScroll(i16),
    /// Opens the diff of the commit the selection is on (ADR-069).
    LogShowCommit,
    /// Opens the log's search field, and its editing keys. Typing narrows what
    /// was read; `LogSearchSubmit` is what asks git for the whole history.
    LogSearchOpen,
    LogSearchClose,
    LogSearchChar(char),
    LogSearchText(String),
    LogSearchBackspace,
    LogSearchDelete,
    LogSearchMove(i16),
    LogSearchHome,
    LogSearchEnd,
    LogSearchSubmit,

    // --- image viewer (ADR-078) ------------------------------------------
    /// Closes the picture and gives the pane behind it its focus back.
    ImageClose,
    /// Re-reads the file from disk, keeping the zoom and the pan.
    ImageReload,
    /// Steps the zoom, holding the middle of the pane still.
    ImageZoom(i16),
    /// Goes to one image pixel per half-cell.
    ImageZoomActual,
    /// Goes back to the whole picture in the pane, and stays fitted across a
    /// resize.
    ImageZoomFit,
    /// Moves the window over the picture, in eighths of what it can see.
    ImagePan(i16, i16),
    /// A whole pane down or up — `PageDown` and `PageUp`.
    ImagePanPage(i16),
    /// Puts the middle of the picture in the middle of the pane.
    ImageCentre,
    /// Shows or hides the metadata column.
    ImageToggleMeta,
    /// A button pressed over the picture: where the drag that may follow is
    /// measured from.
    ImageGrab(u16, u16),
    /// The pointer moved with the button down — the pixel grabbed follows it.
    ImageDragTo(u16, u16),
    /// The button came back up, so the next drag starts from its own press.
    ImageRelease,

    /// Opens `.git/config` in an editor tab (ADR-070).
    GitOpenConfig,

    /// Something changed on disk that the editor did not do (ADR-040). Like
    /// `GitJobFinished` no user can produce it: the run loop makes it out of an
    /// `AppEvent::FilesChanged` so that the watcher reaches `App` through the
    /// same door as everything else. It says nothing on the status bar — a
    /// refresh nobody asked for should not talk.
    ExternalChange(FsChange),

    /// A job the worker has finished. The only command no user can produce: the
    /// run loop makes it out of an `AppEvent::GitJob` so that a background
    /// result reaches `App` through the same door as everything else.
    GitJobFinished(JobOutcome),

    /// Asks for a name and then creates a file, a directory, or a new name for
    /// what is selected (SPEC §20).
    NewFilePrompt,
    NewDirectoryPrompt,
    RenamePrompt,
    /// Asks before deleting what is selected. `DeletePath` is what removes it.
    DeletePrompt,
    /// Opens the file browser (SPEC §6, ADR-051): a directory to walk, and a
    /// filter field that also takes a typed path.
    OpenPrompt,
    /// The browser's Open button: walk into the selected directory, or open the
    /// selected file. Only that button carries it.
    BrowserOpen,
    /// The browser's Open Folder button: make the selected directory — or, when
    /// a file is selected, the one being listed — the workspace.
    BrowserOpenFolder,
    /// Makes a directory the workspace: the explorer, the git panel and the
    /// filesystem watcher all move to it (ADR-051). Open tabs are left alone.
    OpenWorkspace(PathBuf),
    /// Asks for the path to write the active tab to, pre-filled with its own.
    SaveAsPrompt,
    /// Runs a file operation with whatever the open input dialog holds. Only a
    /// dialog button carries this; activating one turns it into `ApplyFileOp`.
    SubmitInput(FileOp),
    /// A file operation and the name it was given.
    ApplyFileOp(FileOp, String),
    /// Removes a path from disk — the delete dialog's second button.
    DeletePath(PathBuf),

    SelectTab(usize),
    /// Scrolls the tab strip sideways, in tabs. The wheel over the bar and its
    /// two overflow arrows are what produce it; the strip still follows the
    /// active tab on its own, so this only ever moves the *window* over the
    /// tabs and never which of them is in front.
    ScrollTabs(i16),
    NextTab,
    PrevTab,
    /// Closes the active tab, asking first when it has unsaved changes.
    CloseTab,
    /// The same for a given tab — the tab bar's close button and middle click.
    CloseTabAt(usize),
    /// Closes a tab without asking. Produced by the confirm dialog's
    /// "Don't Save", never bound to a key.
    CloseTabDiscarding(usize),
    /// Writes a tab to disk and closes it if the write succeeded — the confirm
    /// dialog's "Save".
    SaveAndCloseTab(usize),

    /// Re-reads the active tab from disk, asking first when it has unsaved
    /// changes (ADR-043). The File menu's Reload.
    Reload,
    /// Re-reads one tab whatever is in it — the "Changed on disk" dialog's
    /// Reload, and what the silent reload of a clean buffer runs. The step is
    /// undoable, so this is not the one-way door its name suggests.
    ReloadTab(usize),
    /// Keeps a buffer that has diverged from its file, and stops asking about
    /// it — the same dialog's Keep Mine.
    KeepBuffer(usize),

    /// Scrolls the editor viewport without moving the cursor — the wheel.
    ScrollEditor(i16),
    /// Moves the editor's window sideways, for the lines that are wider than
    /// the pane. Nothing to do while lines wrap: there is no sideways then
    /// (ADR-057).
    ScrollEditorHorizontal(i16),
    /// Breaks long lines onto the next row, or stops doing so, and remembers
    /// the answer in the settings file (SPEC §58).
    ToggleWordWrap,
    /// Asks which line to jump to (SPEC §58). `GotoLine` is what jumps.
    GotoLinePrompt,
    /// Runs the jump with whatever the open dialog holds. Only the dialog's
    /// button carries this; activating it turns it into `GotoLine`.
    SubmitGotoLine,
    /// Puts the cursor on a one-based line number, as it was typed: parsing it
    /// is the command's own job, because a dialog field holds text and a user
    /// can type anything into one.
    GotoLine(String),

    /// Asks what the active file's lines should be separated by (SPEC §17,
    /// ADR-058). The status bar's `LF` / `CRLF` is what opens it.
    LineEndingPrompt,
    /// Asks whether to rewrite the file with a chosen ending. The picker's rows
    /// carry this rather than the conversion itself: rewriting every line of a
    /// file is not something a click on a readout may do unasked.
    ConvertLineEndingPrompt(LineEnding),
    /// Sets the ending and saves, which is what the confirmation approved.
    ConvertLineEnding(LineEnding),
    /// Asks which encoding the file is read and written as (SPEC §17,
    /// ADR-059).
    EncodingPrompt,
    /// Asks what a chosen encoding should mean: re-read the file with it, or
    /// write the buffer out in it. The picker's rows carry this — there are two
    /// answers and neither is obvious enough to take without asking.
    EncodingChoice(String),
    /// Re-reads the file from disk, decoding it with the named encoding. The
    /// answer for a file that came out as mojibake; undoable, like a reload.
    ReopenWithEncoding(String),
    /// Writes the buffer out in the named encoding, keeping the text.
    ConvertEncoding(String),
    /// Asks which grammar highlights the active tab (SPEC §21, ADR-058).
    LanguagePrompt,
    /// Highlights the active tab with the named grammar, overriding what the
    /// file's name and first line suggested.
    SetLanguage(String),

    /// Swaps the active tab between the text and the CSV table (SPEC §65).
    ToggleTableView,
    /// Asks what separates the table's columns, and what quotes its fields
    /// (ADR-062). Both are opened from the status bar as well as the menu.
    CsvDelimiterPrompt,
    CsvQuotePrompt,
    /// Reads the file again with a different delimiter or quote character.
    /// `None` for the quote is a file with no quoting at all.
    SetCsvDelimiter(char),
    SetCsvQuote(Option<char>),
    /// Moves the table's window without moving the selected cell — the wheel.
    ScrollTable(i16),
    ScrollTableHorizontal(i16),
    /// Selects a cell by where it was clicked: a row of the drawn window —
    /// `None` for the header, which is a record like any other (SPEC §65) — and
    /// a column of the whole table.
    SelectCell {
        row: Option<usize>,
        column: usize,
    },
    /// Extends the table's selection to the cell under the pointer — the drag
    /// that makes a block of cells (SPEC §65).
    ExtendCellTo {
        row: Option<usize>,
        column: usize,
    },
    /// Selects a whole record by its number in the gutter, and a whole column
    /// by its name in the header — the two gestures every grid has.
    SelectRowAt {
        row: Option<usize>,
    },
    SelectColumnAt {
        column: usize,
    },
    /// Selects every cell of the record, or of the column, the cursor is on.
    SelectRow,
    SelectColumn,
    /// Opens the selected cell for typing, over the value it already holds
    /// (SPEC §65, ADR-063). Saving it and giving it up are not commands of
    /// their own: they are what `Enter` and `Esc` mean while a cell is open,
    /// and they are routed there rather than bound to keys the text view needs.
    EditCell,
    /// Adds a blank record under the selected one, and moves onto it.
    InsertRecord,
    /// Removes the selected record from the file.
    DeleteRecord,

    MoveCursor(Motion),
    /// The same motions with the anchor left where it is — Shift+navigation.
    ExtendSelection(Motion),
    /// Puts the cursor at a document line and *display* column: the only entry
    /// point that starts from a visual coordinate, because the mouse does.
    PlaceCursor {
        line: usize,
        col: usize,
    },
    /// Drags the head of the selection to a display position.
    ExtendCursorTo {
        line: usize,
        col: usize,
    },
    /// Selects the word under a display position — the double-click.
    SelectWordAt {
        line: usize,
        col: usize,
    },
    SelectAll,

    InsertChar(char),
    /// A whole string in one operation: a paste, from `Ctrl+V` or from the
    /// terminal's bracketed paste (SPEC §15).
    InsertText(String),
    InsertNewline,
    Backspace,
    Delete,
    /// Removes the caret's whole line, or every line a selection touches, as
    /// one undo step (SPEC §15).
    DeleteLine,

    /// Moves the dialog's button selection, wrapping at both ends.
    DialogMove(i16),
    /// Moves the selection in a dialog's list body, clamped at both ends.
    DialogListMove(i16),
    /// Selects a list row by its place in the visible window — a click on one.
    DialogSelectItem(usize),
    /// Runs the selected list row's command. Only a list dialog's confirm
    /// button carries it; activating one turns it into that row's own command.
    SubmitListChoice,
    /// Text entry in a dialog's input field. Separate from the editor's own
    /// insert commands because a file name is not a document: it has no undo
    /// history, no selection and no viewport.
    DialogInputChar(char),
    DialogInputText(String),
    DialogInputBackspace,
    DialogInputDelete,
    /// Moves the input field's caret by one grapheme cluster.
    DialogInputMove(i16),
    DialogInputHome,
    DialogInputEnd,
    /// Runs the selected button's command.
    DialogActivate,
    /// Runs a button's command by index — the mouse.
    DialogActivateButton(usize),
    /// Dismisses the dialog without doing anything else.
    DialogCancel,

    /// Opens the find bar, seeding it with the selection (SPEC §22).
    SearchOpen,
    /// The same bar with its replacement row (SPEC §23).
    ReplaceOpen,
    /// Closes the bar and puts focus back in the editor.
    SearchClose,
    /// Text entry in whichever of the bar's fields has the caret. Separate from
    /// the dialog's, because the two are open at different times and mean
    /// different things.
    SearchInputChar(char),
    SearchInputText(String),
    SearchInputBackspace,
    SearchInputDelete,
    SearchInputMove(i16),
    SearchInputHome,
    SearchInputEnd,
    /// Moves the caret between the find and replace fields — Tab.
    SearchToggleField,
    /// Puts the caret in one named field — a click on it.
    SearchFocusField(SearchField),
    SearchToggleCase,
    /// Selects the next or previous hit, wrapping at the ends of the document.
    FindNext,
    FindPrev,
    /// Rewrites the current hit and moves to the next one.
    ReplaceCurrent,
    /// Rewrites every hit as one undo step (SPEC §23).
    ReplaceAll,

    /// Reverses the last edit, or replays the last one reversed (SPEC §16).
    Undo,
    Redo,

    Copy,
    Cut,
    /// Inserts whatever the clipboard holds.
    Paste,

    Save,

    /// Switches the colour scheme and writes the choice to the settings file
    /// (SPEC §43). Re-selecting the theme already on screen is a no-op, so the
    /// file is not rewritten on every visit to the View menu.
    SetTheme(ThemeKind),

    /// The Help menu's About entry: a message dialog with the version in it.
    ShowAbout,

    /// The Help menu's Check for Updates entry: asks GitHub now, and answers
    /// either way because the user asked (SPEC §66, ADR-065).
    CheckForUpdates,
    /// Turns the start-up check on or off, and writes the choice to the
    /// settings file. A request to a third party that the user did not make is
    /// one they are entitled to stop.
    ToggleUpdateChecks,
    /// A finished check, arriving from its own thread through the same door
    /// every other producer uses (ARCHITECTURE invariant 3).
    UpdateCheckFinished(UpdateCheck),

    /// The Help menu's Shortcuts entry, and `F1`: the key tables on screen
    /// (SPEC §6). A pager, like the diff viewer, over `docs::sections()`.
    ShowHelp,
    HelpClose,
    HelpScroll(i16),
    HelpScrollPage(i16),
    HelpHome,
    HelpEnd,

    MenuOpen(usize),
    MenuClose,
    MenuNextMenu,
    MenuPrevMenu,
    MenuNextItem,
    MenuPrevItem,
    MenuActivate,
    MenuActivateItem(usize),
}

impl Command {
    /// One line of English for the generated `docs/SHORTCUTS.md` (ADR-028).
    ///
    /// The match is exhaustive on purpose: a command added without a
    /// description does not compile, which is what keeps the generated
    /// documentation from falling behind the keymap it is generated from.
    ///
    /// Commands that carry a coordinate or an index are described by what they
    /// do, not by the value — the mouse is their only producer and a table of
    /// key bindings never shows one.
    pub fn description(&self) -> String {
        let step = |delta: i16, forward: &str, back: &str| {
            if delta >= 0 { forward } else { back }.to_string()
        };
        match self {
            Self::Quit => "Quit, asking first when a tab has unsaved changes".into(),
            Self::SaveAndQuit(_) => "Save this tab and go on quitting".into(),
            Self::DiscardAndQuit(_) => "Close this tab without saving and go on quitting".into(),

            Self::FocusPane(target) => format!("Focus the {}", pane_name(*target)),
            Self::CycleFocus => "Cycle focus: editor → explorer → git panel".into(),

            Self::ToggleSidebarMode => "Switch the sidebar between explorer and git".into(),
            Self::MoveSidebarSelection(delta) => step(
                *delta,
                "Move the sidebar selection down",
                "Move the sidebar selection up",
            ),
            Self::ScrollSidebar(delta) => {
                step(*delta, "Scroll the sidebar down", "Scroll the sidebar up")
            }

            Self::ExplorerActivate => "Open the file, or fold the directory".into(),
            Self::ExplorerActivateRow(_) => "Open or fold the row under the pointer".into(),
            Self::ExplorerExpand => "Expand the directory, or step into an open one".into(),
            Self::ExplorerCollapse => "Collapse the directory, or step out to its parent".into(),
            Self::ExplorerRefresh => "Re-read the tree from disk".into(),
            Self::ToggleHiddenFiles => "Show or hide ignored and hidden files".into(),

            Self::GitRefresh => "Re-read the repository status".into(),
            Self::GitOpenSelected => "Open the selected changed file".into(),
            Self::GitDiffRow(_) => "Show the diff of a changed file".into(),
            Self::GitStage => "Stage the selected file".into(),
            Self::GitUnstage => "Unstage the selected file".into(),
            Self::GitToggleStage => {
                "Stage the selected file, or unstage it when it is staged".into()
            }
            Self::GitStageAll => "Stage every change".into(),
            Self::GitUnstageAll => "Unstage every change".into(),
            Self::GitCommitPrompt => "Commit what is staged — asks for a message".into(),
            Self::SubmitCommit => "Commit with the message that was typed".into(),
            Self::GitCommit(_) => "Commit what is staged".into(),
            Self::GitPull => "Pull from the upstream branch".into(),
            Self::GitPush => "Push to the upstream branch".into(),
            Self::GitCancel => "Stop the running git operation".into(),
            Self::GitBranchPrompt => "Switch branch — opens a picker".into(),
            Self::GitSwitchBranch(_) => "Switch to a branch".into(),
            Self::GitNewBranchPrompt => "New branch — asks for a name".into(),
            Self::SubmitBranch => "Create the branch that was named".into(),
            Self::GitCreateBranch(_) => "Create a branch and switch to it".into(),
            Self::GitMergePrompt => "Merge a branch — opens a picker".into(),
            Self::GitMerge(_) => "Merge a branch into the current one".into(),
            Self::GitStageResolved => "Stage the conflicted file as resolved".into(),
            Self::GitJobFinished(_) => "Report a finished background git job".into(),
            Self::ExternalChange(_) => "Re-read what changed on disk".into(),

            Self::GitDiff => "Show the diff of the selected file".into(),
            Self::GitDiffToggleSide => "Show the other side: staged or unstaged".into(),
            Self::DiffRefresh => "Re-read the diff".into(),
            Self::DiffClose => "Close the diff tab".into(),
            Self::DiffScroll(delta) => step(*delta, "Scroll down a line", "Scroll up a line"),
            Self::DiffScrollPage(delta) => step(*delta, "Scroll down a page", "Scroll up a page"),
            Self::DiffScrollHorizontal(delta) => step(*delta, "Scroll right", "Scroll left"),
            Self::DiffHome => "Go to the first line".into(),
            Self::DiffEnd => "Go to the last line".into(),

            Self::GitLog => "Show the repository's history".into(),
            Self::GitFileHistory => "Show the history of this file".into(),
            Self::GitLineHistory => "Show the history of the selected lines".into(),
            Self::LogRefresh => "Re-read the history".into(),
            Self::LogClose => "Close the log tab".into(),
            Self::LogMove(delta) => step(
                *delta,
                "Select the next commit",
                "Select the previous commit",
            ),
            Self::LogMovePage(delta) => step(*delta, "Move down a page", "Move up a page"),
            Self::LogHome => "Select the newest commit".into(),
            Self::LogEnd => "Select the oldest commit listed".into(),
            Self::LogShowRow(_) => "Show a commit as a diff".into(),
            Self::LogScroll(delta) => step(*delta, "Scroll down", "Scroll up"),
            Self::LogShowCommit => "Show the selected commit as a diff".into(),
            Self::LogSearchOpen => "Search the history".into(),
            Self::LogSearchClose => "Close the search field and show everything".into(),
            Self::LogSearchChar(_) => "Type it into the search field".into(),
            Self::LogSearchText(_) => "Paste into the search field".into(),
            Self::LogSearchBackspace => "Delete the cluster before the caret".into(),
            Self::LogSearchDelete => "Delete the cluster after the caret".into(),
            Self::LogSearchMove(delta) => {
                step(*delta, "Move the caret right", "Move the caret left")
            }
            Self::LogSearchHome => "Move the caret to the start".into(),
            Self::LogSearchEnd => "Move the caret to the end".into(),
            Self::LogSearchSubmit => "Ask git to search the whole history".into(),

            Self::ImageClose => "Close the image tab".into(),
            Self::ImageReload => "Re-read the image from disk".into(),
            Self::ImageZoom(delta) => step(*delta, "Zoom in", "Zoom out"),
            Self::ImageZoomActual => "Zoom to actual size".into(),
            Self::ImageZoomFit => "Fit the whole image in the pane".into(),
            Self::ImagePan(dx, dy) => match (dx, dy) {
                (0, d) if *d < 0 => "Pan up".into(),
                (0, _) => "Pan down".into(),
                (d, _) if *d < 0 => "Pan left".into(),
                _ => "Pan right".into(),
            },
            Self::ImagePanPage(delta) => step(*delta, "Pan down a pane", "Pan up a pane"),
            Self::ImageCentre => "Centre the image".into(),
            Self::ImageToggleMeta => "Show or hide the metadata column".into(),
            Self::ImageGrab(_, _) => "Take hold of the image".into(),
            Self::ImageDragTo(_, _) => "Drag the image".into(),
            Self::ImageRelease => "Let go of the image".into(),

            Self::GitOpenConfig => "Open .git/config in a tab".into(),

            Self::NewFilePrompt => "New file — asks for a name".into(),
            Self::NewDirectoryPrompt => "New folder — asks for a name".into(),
            Self::RenamePrompt => "Rename what is selected in the explorer".into(),
            Self::DeletePrompt => "Delete what is selected in the explorer (asks first)".into(),
            Self::OpenPrompt => "Open a file or a folder — browses for one".into(),
            Self::BrowserOpen => "Open what is selected, or go into it".into(),
            Self::BrowserOpenFolder => "Open the selected folder in the sidebar".into(),
            Self::OpenWorkspace(_) => "Show a folder in the sidebar".into(),
            Self::SaveAsPrompt => "Save the active file under another path".into(),
            Self::SubmitInput(_) => "Run the dialog's operation on what was typed".into(),
            Self::ApplyFileOp(_, _) => "Run a file operation".into(),
            Self::DeletePath(_) => "Delete a path from disk".into(),

            Self::SelectTab(_) => "Switch to a tab".into(),
            Self::ScrollTabs(delta) => {
                step(*delta, "Scroll the tabs right", "Scroll the tabs left")
            }
            Self::NextTab => "Next tab".into(),
            Self::PrevTab => "Previous tab".into(),
            Self::CloseTab => "Close the active tab, asking first when it is modified".into(),
            Self::CloseTabAt(_) => "Close a tab".into(),
            Self::CloseTabDiscarding(_) => "Close a tab without saving".into(),
            Self::SaveAndCloseTab(_) => "Save a tab and close it".into(),
            Self::Reload => "Re-read the active file from disk".into(),
            Self::ReloadTab(_) => "Re-read a tab from disk".into(),
            Self::KeepBuffer(_) => "Keep the buffer that has changed on disk".into(),

            Self::ScrollEditor(delta) => {
                step(*delta, "Scroll the editor down", "Scroll the editor up")
            }
            Self::ScrollEditorHorizontal(delta) => {
                step(*delta, "Scroll the editor right", "Scroll the editor left")
            }
            Self::ToggleWordWrap => "Wrap long lines, or stop wrapping them".into(),
            Self::GotoLinePrompt => "Go to a line — asks for the number".into(),
            Self::SubmitGotoLine => "Go to the line that was typed".into(),
            Self::GotoLine(_) => "Go to a line by number".into(),

            Self::LineEndingPrompt => "Choose how lines are separated on disk".into(),
            Self::ConvertLineEndingPrompt(ending) => {
                format!(
                    "Ask before rewriting the file with {} endings",
                    ending.label()
                )
            }
            Self::ConvertLineEnding(ending) => {
                format!("Rewrite the file with {} endings and save", ending.label())
            }
            Self::EncodingPrompt => "Choose the file's encoding".into(),
            Self::EncodingChoice(name) => format!("Ask what to do with {name}"),
            Self::ReopenWithEncoding(name) => format!("Re-read this file as {name}"),
            Self::ConvertEncoding(name) => format!("Write this file as {name}"),
            Self::LanguagePrompt => "Choose the grammar that highlights this file".into(),
            Self::SetLanguage(name) => format!("Highlight this file as {name}"),

            Self::ToggleTableView => "Show this file as a table, or as text".into(),
            Self::CsvDelimiterPrompt => "Choose what separates the table's columns".into(),
            Self::CsvQuotePrompt => "Choose what quotes the table's fields".into(),
            Self::SetCsvDelimiter(ch) => format!("Split the columns on {ch:?}"),
            Self::SetCsvQuote(Some(ch)) => format!("Quote the fields with {ch:?}"),
            Self::SetCsvQuote(None) => "Read the fields unquoted".into(),
            Self::ScrollTable(delta) => {
                step(*delta, "Scroll the table down", "Scroll the table up")
            }
            Self::ScrollTableHorizontal(delta) => {
                step(*delta, "Scroll the table right", "Scroll the table left")
            }
            Self::SelectCell { .. } => "Select the cell under the pointer".into(),
            Self::ExtendCellTo { .. } => "Extend the selection to the pointer".into(),
            Self::SelectRowAt { .. } => "Select the row under the pointer".into(),
            Self::SelectColumnAt { .. } => "Select the column under the pointer".into(),
            Self::SelectRow => "Select this row".into(),
            Self::SelectColumn => "Select this column".into(),
            Self::EditCell => "Type into the selected cell".into(),
            Self::InsertRecord => "Add a row under this one".into(),
            Self::DeleteRecord => "Delete this row".into(),

            Self::MoveCursor(motion) => format!("Move the cursor {}", motion_name(*motion)),
            Self::ExtendSelection(motion) => {
                format!("Extend the selection {}", motion_name(*motion))
            }
            Self::PlaceCursor { .. } => "Place the cursor".into(),
            Self::ExtendCursorTo { .. } => "Drag the selection".into(),
            Self::SelectWordAt { .. } => "Select the word under the pointer".into(),
            Self::SelectAll => "Select all".into(),

            Self::InsertChar('\t') => "Insert a tab".into(),
            Self::InsertChar(_) => "Insert the character".into(),
            Self::InsertText(_) => "Insert pasted text".into(),
            Self::InsertNewline => "Insert a newline".into(),
            Self::Backspace => "Delete the cluster before the cursor".into(),
            Self::Delete => "Delete the cluster after the cursor".into(),
            Self::DeleteLine => "Delete the whole line, or every selected line".into(),

            Self::DialogMove(delta) => step(
                *delta,
                "Move along the button row",
                "Move back along the button row",
            ),
            Self::DialogListMove(delta) => step(*delta, "Move down the list", "Move up the list"),
            Self::DialogSelectItem(_) => "Select a list row".into(),
            Self::SubmitListChoice => "Choose the selected row".into(),
            Self::DialogInputChar(_) => "Type it into the field".into(),
            Self::DialogInputText(_) => "Paste into the field".into(),
            Self::DialogInputBackspace => "Delete the cluster before the caret".into(),
            Self::DialogInputDelete => "Delete the cluster after the caret".into(),
            Self::DialogInputMove(delta) => {
                step(*delta, "Move the caret right", "Move the caret left")
            }
            Self::DialogInputHome => "Move the caret to the start".into(),
            Self::DialogInputEnd => "Move the caret to the end".into(),
            Self::DialogActivate => "Activate the selected button".into(),
            Self::DialogActivateButton(_) => "Activate a button".into(),
            Self::DialogCancel => "Dismiss the dialog".into(),

            Self::SearchOpen => "Open the find bar, seeded with the selection".into(),
            Self::ReplaceOpen => "Open the find bar with its replacement row".into(),
            Self::SearchClose => "Close the find bar".into(),
            Self::SearchInputChar(_) => "Type it into the field with the caret".into(),
            Self::SearchInputText(_) => "Paste into the field with the caret".into(),
            Self::SearchInputBackspace => "Delete the cluster before the caret".into(),
            Self::SearchInputDelete => "Delete the cluster after the caret".into(),
            Self::SearchInputMove(delta) => {
                step(*delta, "Move the caret right", "Move the caret left")
            }
            Self::SearchInputHome => "Move the caret to the start".into(),
            Self::SearchInputEnd => "Move the caret to the end".into(),
            Self::SearchToggleField => "Move between the find and replace fields".into(),
            Self::SearchFocusField(_) => "Put the caret in that field".into(),
            Self::SearchToggleCase => "Toggle match case".into(),
            Self::FindNext => "Next match".into(),
            Self::FindPrev => "Previous match".into(),
            Self::ReplaceCurrent => "Replace the current match".into(),
            Self::ReplaceAll => "Replace every match, as one undo step".into(),

            Self::Undo => "Undo".into(),
            Self::Redo => "Redo".into(),

            Self::Copy => "Copy the selection".into(),
            Self::Cut => "Cut the selection".into(),
            Self::Paste => "Paste".into(),

            Self::Save => "Save the active file".into(),
            Self::SetTheme(kind) => format!("Use the {} theme", kind.label()),
            Self::ShowAbout => "About FerroEdit".into(),
            Self::CheckForUpdates => "Ask GitHub whether there is a newer release".into(),
            Self::ToggleUpdateChecks => {
                "Look for a new release at start-up, or stop looking".into()
            }
            Self::UpdateCheckFinished(_) => "Report what the update check found".into(),

            Self::ShowHelp => "Show the keyboard shortcuts".into(),
            Self::HelpClose => "Close the help screen".into(),
            Self::HelpScroll(delta) => step(*delta, "Scroll down a line", "Scroll up a line"),
            Self::HelpScrollPage(delta) => step(*delta, "Scroll down a page", "Scroll up a page"),
            Self::HelpHome => "Go to the first line".into(),
            Self::HelpEnd => "Go to the last line".into(),

            Self::MenuOpen(_) => "Open the menu bar".into(),
            Self::MenuClose => "Close the menu".into(),
            Self::MenuNextMenu => "Next menu".into(),
            Self::MenuPrevMenu => "Previous menu".into(),
            Self::MenuNextItem => "Next item".into(),
            Self::MenuPrevItem => "Previous item".into(),
            Self::MenuActivate => "Activate the item".into(),
            Self::MenuActivateItem(_) => "Activate an item".into(),
        }
    }
}

/// The pane names the documentation uses, which are lower case because they
/// appear mid-sentence; `FocusTarget::label` is the status bar's capitalised
/// form and is not interchangeable with them.
fn pane_name(target: FocusTarget) -> &'static str {
    match target {
        FocusTarget::Editor => "editor",
        FocusTarget::Explorer => "explorer",
        FocusTarget::GitPanel => "git panel",
        FocusTarget::Menu => "menu",
        FocusTarget::Dialog => "dialog",
        FocusTarget::Search => "find bar",
        FocusTarget::Diff => "diff viewer",
        FocusTarget::Log => "log viewer",
        FocusTarget::Image => "image viewer",
        FocusTarget::LogSearch => "log search field",
        FocusTarget::Help => "help screen",
    }
}

fn motion_name(motion: Motion) -> &'static str {
    match motion {
        Motion::Left => "left",
        Motion::Right => "right",
        Motion::Up => "up",
        Motion::Down => "down",
        Motion::Home => "to the start of the line",
        Motion::End => "to the end of the line",
        Motion::WordLeft => "one word left",
        Motion::WordRight => "one word right",
        Motion::PageUp => "one page up",
        Motion::PageDown => "one page down",
        Motion::DocumentStart => "to the start of the document",
        Motion::DocumentEnd => "to the end of the document",
    }
}

pub struct MenuItem {
    pub label: &'static str,
    pub command: Command,
}

/// One row of a drop-down: an entry that runs something, or the rule drawn
/// between two groups of them.
///
/// The separator is a row rather than a property of the item under it because
/// that is what it is on screen — the popup's height counts it, the selection
/// steps over it, and a click on it does nothing.
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

impl MenuEntry {
    /// The entry, when it is one. A separator answers `None`, which is what
    /// every caller that resolves a row to a command wants.
    pub fn item(&self) -> Option<&MenuItem> {
        match self {
            Self::Item(item) => Some(item),
            Self::Separator => None,
        }
    }

    pub fn is_separator(&self) -> bool {
        matches!(self, Self::Separator)
    }
}

pub struct MenuDef {
    pub title: &'static str,
    pub items: &'static [MenuEntry],
}

impl MenuDef {
    /// The rows that resolve to a command, in row order.
    pub fn entries(&self) -> impl Iterator<Item = &MenuItem> {
        self.items.iter().filter_map(MenuEntry::item)
    }
}

/// Menu bar contents (SPEC §6, §24).
///
/// Every entry resolves to a real command. The help screen was the last one
/// that did not, and it landed in Phase 14 — with it went the `Unimplemented`
/// placeholder itself, so a menu entry that does nothing is now a thing the
/// type system has no way to express (ADR-038).
///
/// There is deliberately no shortcut column here: the menu reads the key labels
/// out of `event::keyboard::BINDINGS`, so an entry can never advertise a key
/// that is not actually bound.
pub static MENUS: &[MenuDef] = &[
    MenuDef {
        title: "File",
        items: &[
            item("New File", Command::NewFilePrompt),
            item("New Folder", Command::NewDirectoryPrompt),
            SEP,
            item("Open…", Command::OpenPrompt),
            item("Save", Command::Save),
            item("Save As…", Command::SaveAsPrompt),
            item("Reload", Command::Reload),
            SEP,
            item("Rename…", Command::RenamePrompt),
            SEP,
            // On its own between two rules: Delete is the one entry in this
            // menu that destroys something, and it used to sit one row above
            // Open, where a mis-aimed click found it.
            item("Delete", Command::DeletePrompt),
            SEP,
            item("Close Tab", Command::CloseTab),
            item("Quit", Command::Quit),
        ],
    },
    MenuDef {
        title: "Edit",
        items: &[
            item("Undo", Command::Undo),
            item("Redo", Command::Redo),
            SEP,
            item("Cut", Command::Cut),
            item("Copy", Command::Copy),
            item("Paste", Command::Paste),
            SEP,
            item("Delete Line", Command::DeleteLine),
            // The three the table adds (SPEC §65). They are in the Edit menu
            // rather than in View because they change the file, and they are
            // spelled "row" rather than "record" because that is what the grid
            // on screen calls them.
            SEP,
            item("Edit Cell", Command::EditCell),
            item("Insert Row", Command::InsertRecord),
            item("Delete Row", Command::DeleteRecord),
        ],
    },
    MenuDef {
        title: "Selection",
        items: &[
            item("Select All", Command::SelectAll),
            // The two shapes a grid has and text has not (SPEC §65).
            SEP,
            item("Select Row", Command::SelectRow),
            item("Select Column", Command::SelectColumn),
        ],
    },
    MenuDef {
        title: "Search",
        items: &[
            item("Find…", Command::SearchOpen),
            item("Replace…", Command::ReplaceOpen),
            SEP,
            item("Find Next", Command::FindNext),
            item("Find Previous", Command::FindPrev),
            SEP,
            item("Go to Line…", Command::GotoLinePrompt),
            SEP,
            item("Match Case", Command::SearchToggleCase),
            SEP,
            item("Replace Match", Command::ReplaceCurrent),
            item("Replace All", Command::ReplaceAll),
        ],
    },
    MenuDef {
        title: "View",
        items: &[
            item("Toggle Sidebar", Command::ToggleSidebarMode),
            item("Refresh Explorer", Command::ExplorerRefresh),
            item("Hidden and Ignored Files", Command::ToggleHiddenFiles),
            item("Word Wrap", Command::ToggleWordWrap),
            SEP,
            // The three readouts on the status bar that are now questions
            // (ADR-058). They are here as well as there because a feature that
            // can only be reached with a mouse is a feature a terminal user
            // does not have.
            item("Syntax Mode…", Command::LanguagePrompt),
            item("Line Endings…", Command::LineEndingPrompt),
            item("Encoding…", Command::EncodingPrompt),
            SEP,
            // The CSV view and the two questions it puts on the status bar
            // (SPEC §65). Here for the same reason the three above are: the
            // readouts are only there while the table is showing, and turning
            // it on has to be reachable from somewhere that always is.
            item("Table View", Command::ToggleTableView),
            item("Column Delimiter…", Command::CsvDelimiterPrompt),
            item("Quote Character…", Command::CsvQuotePrompt),
            SEP,
            // The sideways window needs entries of its own for the same reason
            // Word Wrap does: `Alt` is a modifier several terminals never
            // deliver, and a command reachable only through one is a command
            // some users do not have (ADR-008).
            item("Scroll Left", Command::ScrollEditorHorizontal(-1)),
            item("Scroll Right", Command::ScrollEditorHorizontal(1)),
            SEP,
            item("Focus Explorer", Command::FocusPane(FocusTarget::Explorer)),
            item("Focus Git", Command::FocusPane(FocusTarget::GitPanel)),
            item("Focus Editor", Command::FocusPane(FocusTarget::Editor)),
            SEP,
            item("Theme: Dark", Command::SetTheme(ThemeKind::Dark)),
            item("Theme: Light", Command::SetTheme(ThemeKind::Light)),
            item(
                "Theme: Dark Simple",
                Command::SetTheme(ThemeKind::DarkSimple),
            ),
            item(
                "Theme: Light Simple",
                Command::SetTheme(ThemeKind::LightSimple),
            ),
            item("Theme: Retro", Command::SetTheme(ThemeKind::Retro)),
        ],
    },
    MenuDef {
        title: "Git",
        items: &[
            item("Refresh", Command::GitRefresh),
            SEP,
            item("Stage", Command::GitStage),
            item("Unstage", Command::GitUnstage),
            item("Stage All", Command::GitStageAll),
            item("Unstage All", Command::GitUnstageAll),
            SEP,
            item("Commit…", Command::GitCommitPrompt),
            item("Pull", Command::GitPull),
            item("Push", Command::GitPush),
            item("Cancel", Command::GitCancel),
            SEP,
            item("Branch…", Command::GitBranchPrompt),
            item("New Branch…", Command::GitNewBranchPrompt),
            item("Merge…", Command::GitMergePrompt),
            SEP,
            item("Diff", Command::GitDiff),
            item("Log", Command::GitLog),
            item("File History", Command::GitFileHistory),
            item("Line History", Command::GitLineHistory),
            SEP,
            item("Config", Command::GitOpenConfig),
        ],
    },
    MenuDef {
        title: "Help",
        items: &[
            item("Shortcuts", Command::ShowHelp),
            item("About", Command::ShowAbout),
            SEP,
            item("Check for Updates", Command::CheckForUpdates),
            // A switch rather than a submenu: the entry runs the same command
            // whichever way it is pointing, and the status bar says which way
            // it went. The View menu's Word Wrap is the same shape.
            item("Check on Start", Command::ToggleUpdateChecks),
        ],
    },
];

/// Which way a menu entry that carries a state is pointing (SPEC §24).
///
/// `None` is an entry that does something rather than switching something: the
/// mark column only exists in a menu that has at least one entry answering
/// `Some`, and its width does not change as the state flips, so the popup does
/// not resize under the cursor when a mark appears.
///
/// The themes answer here too. They are one choice out of five rather than five
/// switches, but on screen a mark against the one in use says the same thing,
/// and a menu that marks Word Wrap and not the live theme reads as if the theme
/// were not a state at all.
pub fn menu_state(app: &App, command: &Command) -> Option<bool> {
    Some(match command {
        Command::ToggleWordWrap => app.settings.word_wrap,
        Command::ToggleHiddenFiles => app.sidebar.tree.show_hidden(),
        // The table is a property of the tab in front, so with no file open
        // there is nothing to be on: an unmarked entry, not a missing one.
        Command::ToggleTableView => app.active().is_some_and(|tab| tab.shows_table()),
        Command::SearchToggleCase => app.search.case_sensitive,
        Command::ToggleUpdateChecks => app.settings.check_for_updates,
        Command::SetTheme(kind) => app.settings.theme == *kind,
        _ => return None,
    })
}

const fn item(label: &'static str, command: Command) -> MenuEntry {
    MenuEntry::Item(MenuItem { label, command })
}

/// The rule between two groups of entries.
const SEP: MenuEntry = MenuEntry::Separator;

#[cfg(test)]
mod tests {
    use super::*;

    /// A theme nobody can reach is a theme that does not exist: every one of
    /// them has to be in the View menu.
    #[test]
    fn every_theme_has_a_menu_entry() {
        let view = MENUS
            .iter()
            .find(|menu| menu.title == "View")
            .expect("a View menu");
        for kind in ThemeKind::ALL {
            assert!(
                view.entries()
                    .any(|item| item.command == Command::SetTheme(kind)),
                "no entry for the {} theme",
                kind.label()
            );
        }
    }

    /// `Alt` is the modifier several terminals never deliver — macOS
    /// Terminal.app without "Use Option as Meta", and some SSH clients — so a
    /// command reachable only through it is a command those users do not have
    /// (ADR-008). Every one of them has to be on a menu as well.
    #[test]
    fn every_alt_binding_is_also_a_menu_entry() {
        let on_a_menu = |command: &Command| {
            MENUS
                .iter()
                .flat_map(MenuDef::entries)
                .any(|item| &item.command == command)
        };
        for binding in crate::event::keyboard::BINDINGS {
            if !binding.mods.contains(crossterm::event::KeyModifiers::ALT) {
                continue;
            }
            assert!(
                on_a_menu(&binding.command),
                "{} runs {:?}, which no menu offers",
                binding.label,
                binding.command
            );
        }
    }

    /// The update check has no key binding and never will — it is a network
    /// request, not a keystroke — so the menu is the only way to reach it, and
    /// the switch that turns it off has to be next to it (ADR-065).
    #[test]
    fn the_update_check_and_its_switch_are_both_on_the_help_menu() {
        let help = MENUS
            .iter()
            .find(|menu| menu.title == "Help")
            .expect("a Help menu");
        for command in [Command::CheckForUpdates, Command::ToggleUpdateChecks] {
            assert!(
                help.entries().any(|item| item.command == command),
                "no entry runs {command:?}"
            );
        }
    }

    /// A drop-down that opened on a rule would have nothing selected, and the
    /// first `Down` would step off it rather than onto the second entry.
    #[test]
    fn no_menu_starts_or_ends_with_a_rule() {
        for menu in MENUS {
            let first = menu.items.first().expect("a menu with entries in it");
            let last = menu.items.last().expect("a menu with entries in it");
            assert!(!first.is_separator(), "{} starts with a rule", menu.title);
            assert!(!last.is_separator(), "{} ends with a rule", menu.title);
        }
    }

    /// Two rules in a row would draw as a two-line gap that means nothing.
    #[test]
    fn no_menu_has_two_rules_in_a_row() {
        for menu in MENUS {
            for pair in menu.items.windows(2) {
                assert!(
                    !(pair[0].is_separator() && pair[1].is_separator()),
                    "{} has two rules in a row",
                    menu.title
                );
            }
        }
    }
}
