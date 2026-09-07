//! The single `Command` enum produced by keyboard, menu, mouse and dialogs.
//!
//! Nothing else may mutate `App` (see `docs/ARCHITECTURE.md` invariant 3). The
//! menu table lives here rather than in `ui/` because it *is* command data: the
//! renderer reads it, and so does `execute_command`.

pub mod execute;

use std::path::PathBuf;

use crate::app::focus::FocusTarget;
use crate::app::search::SearchField;
use crate::editor::cursor::Motion;

/// An operation that needs a name or a path typed before it can run
/// (SPEC §20, §40).
///
/// The text is not part of it: the operation is decided when the dialog opens
/// and the name only exists once the user has typed one, so `SubmitInput`
/// carries the operation and `ApplyFileOp` carries both.
///
/// Phase 9 added the two that take a whole path rather than a name — Open and
/// Save As. They are here rather than in a second enum because the dialog that
/// asks for them is the same dialog: a prompt, a field, and a button that
/// submits whatever is in it.
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
    /// Opens the typed path, resolved against `base` when it is relative.
    Open {
        base: PathBuf,
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
    /// Open and Save As never reach it — both report through the paths that
    /// already say "Opened …" and "Saved …" — but the match is exhaustive so
    /// that a new operation cannot be added without an answer here.
    pub fn past_tense(&self) -> &'static str {
        match self {
            Self::CreateFile { .. } => "Created",
            Self::CreateDirectory { .. } => "Created directory",
            Self::Rename { .. } => "Renamed to",
            Self::Open { .. } => "Opened",
            Self::SaveAs { .. } => "Saved",
        }
    }
}

/// Deliberately `Clone` and not `Copy`: `InsertText` carries the pasted text,
/// and a paste of a megabyte must be moved rather than silently copied on every
/// match arm it passes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Asks first when any tab has unsaved changes; `QuitDiscarding` is what
    /// actually ends the run.
    Quit,
    /// Quits without asking — the confirm dialog's "Quit Anyway".
    QuitDiscarding,

    FocusPane(FocusTarget),
    CycleFocus,

    ToggleSidebarMode,
    MoveSidebarSelection(i16),
    SelectSidebarRow(usize),
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
    /// Shows or hides the files `ignore` filters out (SPEC §19).
    ToggleHiddenFiles,

    /// Asks for a name and then creates a file, a directory, or a new name for
    /// what is selected (SPEC §20).
    NewFilePrompt,
    NewDirectoryPrompt,
    RenamePrompt,
    /// Asks before deleting what is selected. `DeletePath` is what removes it.
    DeletePrompt,
    /// Asks for a path to open (SPEC §6). The explorer is the way to browse;
    /// this is the way to type a path that is not in the tree (ADR-029).
    OpenPrompt,
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

    /// Scrolls the editor viewport without moving the cursor — the wheel.
    ScrollEditor(i16),

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

    /// Moves the dialog's button selection, wrapping at both ends.
    DialogMove(i16),
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

    /// The Help menu's About entry: a message dialog with the version in it.
    ShowAbout,

    MenuOpen(usize),
    MenuClose,
    MenuNextMenu,
    MenuPrevMenu,
    MenuNextItem,
    MenuPrevItem,
    MenuActivate,
    MenuActivateItem(usize),

    /// A menu entry or shortcut that is wired up but whose feature lands in a
    /// later phase. Surfacing it as a notification is better than a key that
    /// silently does nothing.
    Unimplemented(&'static str),
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
            Self::QuitDiscarding => "Quit without saving".into(),

            Self::FocusPane(target) => format!("Focus the {}", pane_name(*target)),
            Self::CycleFocus => "Cycle focus: editor → explorer → git panel".into(),

            Self::ToggleSidebarMode => "Switch the sidebar between explorer and git".into(),
            Self::MoveSidebarSelection(delta) => step(
                *delta,
                "Move the sidebar selection down",
                "Move the sidebar selection up",
            ),
            Self::SelectSidebarRow(_) => "Select a sidebar row".into(),
            Self::ScrollSidebar(delta) => {
                step(*delta, "Scroll the sidebar down", "Scroll the sidebar up")
            }

            Self::ExplorerActivate => "Open the file, or fold the directory".into(),
            Self::ExplorerActivateRow(_) => "Open or fold the row under the pointer".into(),
            Self::ExplorerExpand => "Expand the directory, or step into an open one".into(),
            Self::ExplorerCollapse => "Collapse the directory, or step out to its parent".into(),
            Self::ExplorerRefresh => "Re-read the tree from disk".into(),
            Self::ToggleHiddenFiles => "Show or hide ignored and hidden files".into(),

            Self::NewFilePrompt => "New file — asks for a name".into(),
            Self::NewDirectoryPrompt => "New folder — asks for a name".into(),
            Self::RenamePrompt => "Rename what is selected in the explorer".into(),
            Self::DeletePrompt => "Delete what is selected in the explorer (asks first)".into(),
            Self::OpenPrompt => "Open a file — asks for a path".into(),
            Self::SaveAsPrompt => "Save the active file under another path".into(),
            Self::SubmitInput(_) => "Run the dialog's operation on what was typed".into(),
            Self::ApplyFileOp(_, _) => "Run a file operation".into(),
            Self::DeletePath(_) => "Delete a path from disk".into(),

            Self::SelectTab(_) => "Switch to a tab".into(),
            Self::NextTab => "Next tab".into(),
            Self::PrevTab => "Previous tab".into(),
            Self::CloseTab => "Close the active tab, asking first when it is modified".into(),
            Self::CloseTabAt(_) => "Close a tab".into(),
            Self::CloseTabDiscarding(_) => "Close a tab without saving".into(),
            Self::SaveAndCloseTab(_) => "Save a tab and close it".into(),

            Self::ScrollEditor(delta) => {
                step(*delta, "Scroll the editor down", "Scroll the editor up")
            }

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

            Self::DialogMove(delta) => step(
                *delta,
                "Move along the button row",
                "Move back along the button row",
            ),
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
            Self::ShowAbout => "About FerroEdit".into(),

            Self::MenuOpen(_) => "Open the menu bar".into(),
            Self::MenuClose => "Close the menu".into(),
            Self::MenuNextMenu => "Next menu".into(),
            Self::MenuPrevMenu => "Previous menu".into(),
            Self::MenuNextItem => "Next item".into(),
            Self::MenuPrevItem => "Previous item".into(),
            Self::MenuActivate => "Activate the item".into(),
            Self::MenuActivateItem(_) => "Activate an item".into(),

            Self::Unimplemented(what) => format!("{what} — not implemented yet"),
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

pub struct MenuDef {
    pub title: &'static str,
    pub items: &'static [MenuItem],
}

/// Menu bar contents (SPEC §6, §24).
///
/// Every entry resolves to a real command except the four Git ones and the Help
/// screen, which are Phase 11 and Phase 14; `docs/SHORTCUTS.md` is generated
/// from this table and says so, so the list cannot quietly grow.
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
            item("Rename…", Command::RenamePrompt),
            item("Delete", Command::DeletePrompt),
            item("Open…", Command::OpenPrompt),
            item("Save", Command::Save),
            item("Save As…", Command::SaveAsPrompt),
            item("Close Tab", Command::CloseTab),
            item("Quit", Command::Quit),
        ],
    },
    MenuDef {
        title: "Edit",
        items: &[
            item("Undo", Command::Undo),
            item("Redo", Command::Redo),
            item("Cut", Command::Cut),
            item("Copy", Command::Copy),
            item("Paste", Command::Paste),
        ],
    },
    MenuDef {
        title: "Selection",
        items: &[item("Select All", Command::SelectAll)],
    },
    MenuDef {
        title: "Search",
        items: &[
            item("Find…", Command::SearchOpen),
            item("Replace…", Command::ReplaceOpen),
            item("Find Next", Command::FindNext),
            item("Find Previous", Command::FindPrev),
            item("Match Case", Command::SearchToggleCase),
            item("Replace Match", Command::ReplaceCurrent),
            item("Replace All", Command::ReplaceAll),
        ],
    },
    MenuDef {
        title: "View",
        items: &[
            item("Toggle Sidebar", Command::ToggleSidebarMode),
            item("Refresh Explorer", Command::ExplorerRefresh),
            item("Show Hidden Files", Command::ToggleHiddenFiles),
            item("Focus Explorer", Command::FocusPane(FocusTarget::Explorer)),
            item("Focus Git", Command::FocusPane(FocusTarget::GitPanel)),
            item("Focus Editor", Command::FocusPane(FocusTarget::Editor)),
        ],
    },
    MenuDef {
        title: "Git",
        items: &[
            item("Stage All", Command::Unimplemented("Stage All")),
            item("Commit…", Command::Unimplemented("Commit")),
            item("Pull", Command::Unimplemented("Pull")),
            item("Push", Command::Unimplemented("Push")),
        ],
    },
    MenuDef {
        title: "Help",
        items: &[
            item("Shortcuts", Command::Unimplemented("Shortcuts")),
            item("About", Command::ShowAbout),
        ],
    },
];

const fn item(label: &'static str, command: Command) -> MenuItem {
    MenuItem { label, command }
}
