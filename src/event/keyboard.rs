//! Keymap table: (modifiers, code, focus) -> Command.
//!
//! One table, consulted by nothing but `resolve`. Phase 9 generates
//! `docs/SHORTCUTS.md` from it, which is why every entry carries a printable
//! label: the documentation cannot then drift away from the bindings.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::focus::FocusTarget;
use crate::commands::Command;
use crate::editor::cursor::Motion;

pub struct Binding {
    pub mods: KeyModifiers,
    pub code: KeyCode,
    /// `None` binds globally; `Some(f)` only while `f` has focus.
    pub focus: Option<FocusTarget>,
    pub command: Command,
    pub label: &'static str,
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
const CTRL_SHIFT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::SHIFT);
const ALT: KeyModifiers = KeyModifiers::ALT;

/// Modifiers we compare on. Terminals speaking the kitty protocol also report
/// super/hyper/meta, which must not defeat an otherwise exact match.
const SIGNIFICANT: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SHIFT);

const fn binding(
    mods: KeyModifiers,
    code: KeyCode,
    focus: Option<FocusTarget>,
    command: Command,
    label: &'static str,
) -> Binding {
    Binding {
        mods,
        code,
        focus,
        command,
        label,
    }
}

pub static BINDINGS: &[Binding] = &[
    // --- global -----------------------------------------------------------
    binding(CTRL, KeyCode::Char('q'), None, Command::Quit, "Ctrl+Q"),
    binding(NONE, KeyCode::F(6), None, Command::CycleFocus, "F6"),
    binding(NONE, KeyCode::F(10), None, Command::MenuOpen(0), "F10"),
    binding(
        CTRL,
        KeyCode::Char('b'),
        None,
        Command::ToggleSidebarMode,
        "Ctrl+B",
    ),
    // Ctrl+Tab is not deliverable by every terminal, so Ctrl+PageDown/PageUp
    // are kept as equivalents rather than as second-class fallbacks.
    binding(CTRL, KeyCode::Tab, None, Command::NextTab, "Ctrl+Tab"),
    binding(
        CTRL,
        KeyCode::PageDown,
        None,
        Command::NextTab,
        "Ctrl+PageDown",
    ),
    binding(
        CTRL_SHIFT,
        KeyCode::BackTab,
        None,
        Command::PrevTab,
        "Ctrl+Shift+Tab",
    ),
    // BackTab *is* the shifted Tab, so a terminal that reports it with Ctrl
    // means Ctrl+Shift+Tab whether or not it also reports the Shift.
    binding(
        CTRL,
        KeyCode::BackTab,
        None,
        Command::PrevTab,
        "Ctrl+Shift+Tab",
    ),
    binding(CTRL, KeyCode::PageUp, None, Command::PrevTab, "Ctrl+PageUp"),
    binding(CTRL, KeyCode::Char('w'), None, Command::CloseTab, "Ctrl+W"),
    binding(CTRL, KeyCode::Char('s'), None, Command::Save, "Ctrl+S"),
    binding(
        CTRL,
        KeyCode::Char('n'),
        None,
        Command::NewFilePrompt,
        "Ctrl+N",
    ),
    binding(
        CTRL,
        KeyCode::Char('o'),
        None,
        Command::OpenPrompt,
        "Ctrl+O",
    ),
    // Save As has no key: a legacy terminal cannot tell Ctrl+Shift+S from
    // Ctrl+S, and a key that saves in one terminal and asks for a name in
    // another is worse than a menu entry that always works (ADR-008).
    binding(
        CTRL,
        KeyCode::Char('f'),
        None,
        Command::SearchOpen,
        "Ctrl+F",
    ),
    binding(
        CTRL,
        KeyCode::Char('h'),
        None,
        Command::ReplaceOpen,
        "Ctrl+H",
    ),
    // Next and previous are global on purpose: repeating a search after the bar
    // has been dismissed is the common case, and reopening it to press Enter
    // would be a step nobody wants.
    binding(NONE, KeyCode::F(3), None, Command::FindNext, "F3"),
    binding(SHIFT, KeyCode::F(3), None, Command::FindPrev, "Shift+F3"),
    // --- search bar -------------------------------------------------------
    // The bar is a text field, so the arrows, Home, End and the two delete keys
    // move a caret here rather than a cursor. It is *not* modal: Ctrl+S still
    // saves while it is open.
    search(NONE, KeyCode::Esc, Command::SearchClose, "Esc"),
    search(NONE, KeyCode::Enter, Command::FindNext, "Enter"),
    search(SHIFT, KeyCode::Enter, Command::FindPrev, "Shift+Enter"),
    search(NONE, KeyCode::Down, Command::FindNext, "Down"),
    search(NONE, KeyCode::Up, Command::FindPrev, "Up"),
    search(NONE, KeyCode::Tab, Command::SearchToggleField, "Tab"),
    search(
        SHIFT,
        KeyCode::BackTab,
        Command::SearchToggleField,
        "Shift+Tab",
    ),
    search(NONE, KeyCode::Left, Command::SearchInputMove(-1), "Left"),
    search(NONE, KeyCode::Right, Command::SearchInputMove(1), "Right"),
    search(NONE, KeyCode::Home, Command::SearchInputHome, "Home"),
    search(NONE, KeyCode::End, Command::SearchInputEnd, "End"),
    search(
        NONE,
        KeyCode::Backspace,
        Command::SearchInputBackspace,
        "Backspace",
    ),
    search(NONE, KeyCode::Delete, Command::SearchInputDelete, "Delete"),
    // Alt is the only modifier left that does not collide with editing the
    // field. Terminals that swallow it are why every one of these is also on
    // the Search menu.
    search(ALT, KeyCode::Char('c'), Command::SearchToggleCase, "Alt+C"),
    search(ALT, KeyCode::Char('r'), Command::ReplaceCurrent, "Alt+R"),
    search(ALT, KeyCode::Char('a'), Command::ReplaceAll, "Alt+A"),
    // --- menu -------------------------------------------------------------
    menu(KeyCode::Esc, Command::MenuClose, "Esc"),
    menu(KeyCode::Left, Command::MenuPrevMenu, "Left"),
    menu(KeyCode::Right, Command::MenuNextMenu, "Right"),
    menu(KeyCode::Up, Command::MenuPrevItem, "Up"),
    menu(KeyCode::Down, Command::MenuNextItem, "Down"),
    menu(KeyCode::Enter, Command::MenuActivate, "Enter"),
    // --- dialog -----------------------------------------------------------
    // A dialog is modal: `resolve` consults these bindings and nothing else
    // while it has focus, so no global shortcut can act behind an open prompt.
    dialog(KeyCode::Left, Command::DialogMove(-1), "Left"),
    dialog(KeyCode::Right, Command::DialogMove(1), "Right"),
    dialog(KeyCode::Tab, Command::DialogMove(1), "Tab"),
    dialog(KeyCode::Enter, Command::DialogActivate, "Enter"),
    dialog(KeyCode::Esc, Command::DialogCancel, "Esc"),
    // --- editor -----------------------------------------------------------
    // Escape dismisses the find bar from the editor too: the caret is usually
    // back in the document by the time the user is done with it, and having to
    // click into the bar to close it is a step nobody expects. With no bar open
    // it does nothing, which is what Escape does in a text editor.
    editor(KeyCode::Esc, Command::SearchClose, "Esc"),
    // Navigation moves the cursor; the viewport follows it (SPEC §13). The
    // wheel is the only thing that scrolls without moving the cursor.
    editor(KeyCode::Left, Command::MoveCursor(Motion::Left), "Left"),
    editor(KeyCode::Right, Command::MoveCursor(Motion::Right), "Right"),
    editor(KeyCode::Up, Command::MoveCursor(Motion::Up), "Up"),
    editor(KeyCode::Down, Command::MoveCursor(Motion::Down), "Down"),
    editor(KeyCode::Home, Command::MoveCursor(Motion::Home), "Home"),
    editor(KeyCode::End, Command::MoveCursor(Motion::End), "End"),
    editor(
        KeyCode::PageUp,
        Command::MoveCursor(Motion::PageUp),
        "PageUp",
    ),
    editor(
        KeyCode::PageDown,
        Command::MoveCursor(Motion::PageDown),
        "PageDown",
    ),
    ctrl_editor(
        KeyCode::Left,
        Command::MoveCursor(Motion::WordLeft),
        "Ctrl+Left",
    ),
    ctrl_editor(
        KeyCode::Right,
        Command::MoveCursor(Motion::WordRight),
        "Ctrl+Right",
    ),
    ctrl_editor(
        KeyCode::Home,
        Command::MoveCursor(Motion::DocumentStart),
        "Ctrl+Home",
    ),
    ctrl_editor(
        KeyCode::End,
        Command::MoveCursor(Motion::DocumentEnd),
        "Ctrl+End",
    ),
    // Selection. Shift plus the motions above, which is why they are one
    // command taking a `Motion` rather than eight of their own (SPEC §14).
    extend(KeyCode::Left, Motion::Left, "Shift+Left"),
    extend(KeyCode::Right, Motion::Right, "Shift+Right"),
    extend(KeyCode::Up, Motion::Up, "Shift+Up"),
    extend(KeyCode::Down, Motion::Down, "Shift+Down"),
    extend(KeyCode::Home, Motion::Home, "Shift+Home"),
    extend(KeyCode::End, Motion::End, "Shift+End"),
    extend(KeyCode::PageUp, Motion::PageUp, "Shift+PageUp"),
    extend(KeyCode::PageDown, Motion::PageDown, "Shift+PageDown"),
    ctrl_extend(KeyCode::Left, Motion::WordLeft, "Ctrl+Shift+Left"),
    ctrl_extend(KeyCode::Right, Motion::WordRight, "Ctrl+Shift+Right"),
    ctrl_extend(KeyCode::Home, Motion::DocumentStart, "Ctrl+Shift+Home"),
    ctrl_extend(KeyCode::End, Motion::DocumentEnd, "Ctrl+Shift+End"),
    ctrl_editor(KeyCode::Char('a'), Command::SelectAll, "Ctrl+A"),
    // Undo/redo. Ctrl+Y rather than Ctrl+Shift+Z as the redo key: a legacy
    // terminal cannot tell Ctrl+Shift+Z from Ctrl+Z, so binding it would
    // advertise a key that only some terminals deliver (SPEC §16).
    ctrl_editor(KeyCode::Char('z'), Command::Undo, "Ctrl+Z"),
    ctrl_editor(KeyCode::Char('y'), Command::Redo, "Ctrl+Y"),
    // Clipboard. Editor-only: Ctrl+C anywhere else would be a surprising way
    // to copy nothing, and the terminal's own interrupt is not ours to take.
    ctrl_editor(KeyCode::Char('c'), Command::Copy, "Ctrl+C"),
    ctrl_editor(KeyCode::Char('x'), Command::Cut, "Ctrl+X"),
    ctrl_editor(KeyCode::Char('v'), Command::Paste, "Ctrl+V"),
    // Text entry. Printable characters are not in the table — see `resolve`.
    editor(KeyCode::Enter, Command::InsertNewline, "Enter"),
    editor(KeyCode::Backspace, Command::Backspace, "Backspace"),
    editor(KeyCode::Delete, Command::Delete, "Delete"),
    editor(KeyCode::Tab, Command::InsertChar('\t'), "Tab"),
    // --- sidebar ----------------------------------------------------------
    sidebar(
        FocusTarget::Explorer,
        KeyCode::Up,
        Command::MoveSidebarSelection(-1),
        "Up",
    ),
    sidebar(
        FocusTarget::Explorer,
        KeyCode::Down,
        Command::MoveSidebarSelection(1),
        "Down",
    ),
    sidebar(
        FocusTarget::GitPanel,
        KeyCode::Up,
        Command::MoveSidebarSelection(-1),
        "Up",
    ),
    sidebar(
        FocusTarget::GitPanel,
        KeyCode::Down,
        Command::MoveSidebarSelection(1),
        "Down",
    ),
    // --- explorer ---------------------------------------------------------
    // Enter opens a file and folds a directory; Right and Left are the tree's
    // own axis (SPEC §18). The file operations of SPEC §20 are here too, so
    // they act on what is selected rather than on what is being edited.
    explorer(KeyCode::Enter, Command::ExplorerActivate, "Enter"),
    explorer(KeyCode::Right, Command::ExplorerExpand, "Right"),
    explorer(KeyCode::Left, Command::ExplorerCollapse, "Left"),
    explorer(KeyCode::F(2), Command::RenamePrompt, "F2"),
    explorer(KeyCode::Delete, Command::DeletePrompt, "Delete"),
    explorer(KeyCode::F(5), Command::ExplorerRefresh, "F5"),
    // --- git panel --------------------------------------------------------
    // `F5` is "show me what is really there" in whichever panel has focus.
    sidebar(
        FocusTarget::GitPanel,
        KeyCode::F(5),
        Command::GitRefresh,
        "F5",
    ),
];

/// Keys of a dialog that types (SPEC §40).
///
/// A second table rather than more rows in `BINDINGS`, because these are the
/// same keys with a different meaning: in a confirmation dialog `Left` moves
/// the button selection, and in an input dialog it moves the caret. `resolve`
/// picks the table from the dialog that is open, so the two can never both
/// match.
pub static INPUT_BINDINGS: &[Binding] = &[
    dialog(KeyCode::Left, Command::DialogInputMove(-1), "Left"),
    dialog(KeyCode::Right, Command::DialogInputMove(1), "Right"),
    dialog(KeyCode::Home, Command::DialogInputHome, "Home"),
    dialog(KeyCode::End, Command::DialogInputEnd, "End"),
    dialog(
        KeyCode::Backspace,
        Command::DialogInputBackspace,
        "Backspace",
    ),
    dialog(KeyCode::Delete, Command::DialogInputDelete, "Delete"),
    // Tab is what reaches the buttons, since Left and Right are taken.
    dialog(KeyCode::Tab, Command::DialogMove(1), "Tab"),
    dialog(KeyCode::Enter, Command::DialogActivate, "Enter"),
    dialog(KeyCode::Esc, Command::DialogCancel, "Esc"),
];

const fn search(
    mods: KeyModifiers,
    code: KeyCode,
    command: Command,
    label: &'static str,
) -> Binding {
    binding(mods, code, Some(FocusTarget::Search), command, label)
}

const fn menu(code: KeyCode, command: Command, label: &'static str) -> Binding {
    binding(NONE, code, Some(FocusTarget::Menu), command, label)
}

const fn dialog(code: KeyCode, command: Command, label: &'static str) -> Binding {
    binding(NONE, code, Some(FocusTarget::Dialog), command, label)
}

const fn editor(code: KeyCode, command: Command, label: &'static str) -> Binding {
    binding(NONE, code, Some(FocusTarget::Editor), command, label)
}

const fn ctrl_editor(code: KeyCode, command: Command, label: &'static str) -> Binding {
    binding(CTRL, code, Some(FocusTarget::Editor), command, label)
}

const fn extend(code: KeyCode, motion: Motion, label: &'static str) -> Binding {
    binding(
        SHIFT,
        code,
        Some(FocusTarget::Editor),
        Command::ExtendSelection(motion),
        label,
    )
}

const fn ctrl_extend(code: KeyCode, motion: Motion, label: &'static str) -> Binding {
    binding(
        CTRL_SHIFT,
        code,
        Some(FocusTarget::Editor),
        Command::ExtendSelection(motion),
        label,
    )
}

const fn explorer(code: KeyCode, command: Command, label: &'static str) -> Binding {
    binding(NONE, code, Some(FocusTarget::Explorer), command, label)
}

const fn sidebar(
    focus: FocusTarget,
    code: KeyCode,
    command: Command,
    label: &'static str,
) -> Binding {
    binding(NONE, code, Some(focus), command, label)
}

/// Resolves a key press for the currently focused pane.
///
/// Focus-specific bindings win over global ones regardless of table order, so
/// entries can be grouped for readability instead of by precedence.
///
/// A dialog is the exception: it takes its own bindings and stops there. A
/// modal window that let `Ctrl+Q` through would let the user quit out of the
/// very prompt asking whether they meant to.
///
/// `text_input` says whether the open dialog is one with a field in it, which
/// is what decides between the two dialog tables — and, with it, whether a
/// printable key types a file name or does nothing at all.
pub fn resolve(key: KeyEvent, focus: FocusTarget, text_input: bool) -> Option<Command> {
    let matches = |b: &&Binding| b.code == key.code && b.mods == key.modifiers & SIGNIFICANT;
    if focus == FocusTarget::Dialog {
        let table = if text_input { INPUT_BINDINGS } else { BINDINGS };
        return table
            .iter()
            .find(|b| b.focus == Some(FocusTarget::Dialog) && matches(b))
            .map(|b| b.command.clone())
            .or_else(|| text_input.then(|| typed_name(key)).flatten());
    }
    BINDINGS
        .iter()
        .find(|b| b.focus == Some(focus) && matches(b))
        .or_else(|| BINDINGS.iter().find(|b| b.focus.is_none() && matches(b)))
        .map(|b| b.command.clone())
        .or_else(|| typed_character(key, focus))
}

/// A printable character typed into a dialog's text field.
///
/// The same rule as the editor's: the table is consulted first, and a modifier
/// that is not Shift means the key was meant as a command, not as a letter.
fn typed_name(key: KeyEvent) -> Option<Command> {
    let KeyCode::Char(ch) = key.code else {
        return None;
    };
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    Some(Command::DialogInputChar(ch))
}

/// Printable characters typed into the editor.
///
/// These cannot live in the table — there is one entry per binding and a
/// million printable characters — but they are still resolved into the same
/// `Command`, so text entry takes the same path as everything else (SPEC §25).
///
/// The table is consulted first, so a bound combination such as `Ctrl+S` can
/// never arrive here; the modifier check is what keeps an *unbound* `Ctrl+…`
/// from typing a stray letter into the document.
fn typed_character(key: KeyEvent, focus: FocusTarget) -> Option<Command> {
    let KeyCode::Char(ch) = key.code else {
        return None;
    };
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    match focus {
        FocusTarget::Editor => Some(Command::InsertChar(ch)),
        // The search bar takes typing too, into whichever of its fields has the
        // caret — the same rule, one table lookup earlier.
        FocusTarget::Search => Some(Command::SearchInputChar(ch)),
        _ => None,
    }
}

/// The printable key label bound to a command, if any.
///
/// The menu renders its shortcut column from this rather than from its own
/// strings, so a menu entry cannot advertise a key that is not bound — the
/// anti-drift requirement in SPEC §25.
pub fn shortcut_for(command: &Command) -> Option<&'static str> {
    binding_for(command).map(|b| b.label)
}

/// The whole binding, for the readers that also need its scope.
///
/// `docs/SHORTCUTS.md` is one: a menu entry may advertise a key that is bound
/// only while some other pane has focus — `Alt+C` works in the find bar and
/// `F2` in the explorer — and a table of shortcuts that does not say so is
/// telling half the truth (ADR-028).
pub fn binding_for(command: &Command) -> Option<&'static Binding> {
    BINDINGS.iter().find(|b| &b.command == command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// Most tests are about panes and about dialogs that only have buttons, so
    /// they call a two-argument form; the input-dialog tests below pass the
    /// flag themselves.
    fn resolve(key: KeyEvent, focus: FocusTarget) -> Option<Command> {
        super::resolve(key, focus, false)
    }

    #[test]
    fn undo_and_redo_are_bound_in_the_editor_only() {
        assert_eq!(
            resolve(key(KeyCode::Char('z'), CTRL), FocusTarget::Editor),
            Some(Command::Undo)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('y'), CTRL), FocusTarget::Editor),
            Some(Command::Redo)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('z'), CTRL), FocusTarget::Explorer),
            None,
            "there is nothing to undo in the explorer yet"
        );
    }

    #[test]
    fn a_dialog_swallows_every_key_it_does_not_bind_itself() {
        assert_eq!(
            resolve(key(KeyCode::Left, NONE), FocusTarget::Dialog),
            Some(Command::DialogMove(-1))
        );
        assert_eq!(
            resolve(key(KeyCode::Enter, NONE), FocusTarget::Dialog),
            Some(Command::DialogActivate)
        );
        assert_eq!(
            resolve(key(KeyCode::Esc, NONE), FocusTarget::Dialog),
            Some(Command::DialogCancel)
        );
        for code in [KeyCode::Char('q'), KeyCode::Char('s'), KeyCode::Char('w')] {
            assert_eq!(
                resolve(key(code, CTRL), FocusTarget::Dialog),
                None,
                "no global shortcut may act behind an open prompt"
            );
        }
        assert_eq!(
            resolve(key(KeyCode::Char('a'), NONE), FocusTarget::Dialog),
            None,
            "and nothing types into the document underneath it"
        );
    }

    #[test]
    fn ctrl_w_closes_a_tab_from_every_pane_but_a_dialog() {
        for focus in [
            FocusTarget::Editor,
            FocusTarget::Explorer,
            FocusTarget::GitPanel,
            FocusTarget::Menu,
        ] {
            assert_eq!(
                resolve(key(KeyCode::Char('w'), CTRL), focus),
                Some(Command::CloseTab),
                "Ctrl+W must close a tab while focus is {focus:?}"
            );
        }
    }

    #[test]
    fn ctrl_o_asks_for_a_path_from_every_pane_but_a_dialog() {
        for focus in [
            FocusTarget::Editor,
            FocusTarget::Explorer,
            FocusTarget::GitPanel,
            FocusTarget::Search,
        ] {
            assert_eq!(
                resolve(key(KeyCode::Char('o'), CTRL), focus),
                Some(Command::OpenPrompt),
                "Ctrl+O must open the prompt while focus is {focus:?}"
            );
        }
        assert_eq!(
            resolve(key(KeyCode::Char('o'), CTRL), FocusTarget::Dialog),
            None,
            "and not from behind a prompt that is already open"
        );
    }

    /// Save As is menu-only on purpose (ADR-028): a legacy terminal reports
    /// Ctrl+Shift+S as Ctrl+S, so binding it would make Save ambiguous.
    #[test]
    fn save_as_is_not_bound_to_a_key() {
        assert!(shortcut_for(&Command::SaveAsPrompt).is_none());
        assert_eq!(
            resolve(key(KeyCode::Char('s'), CTRL_SHIFT), FocusTarget::Editor),
            None
        );
    }

    #[test]
    fn ctrl_q_quits_from_every_pane() {
        for focus in [
            FocusTarget::Editor,
            FocusTarget::Explorer,
            FocusTarget::GitPanel,
            FocusTarget::Menu,
        ] {
            assert_eq!(
                resolve(key(KeyCode::Char('q'), CTRL), focus),
                Some(Command::Quit),
                "Ctrl+Q must quit while focus is {focus:?}"
            );
        }
    }

    #[test]
    fn the_same_key_resolves_differently_per_focus() {
        assert_eq!(
            resolve(key(KeyCode::Down, NONE), FocusTarget::Editor),
            Some(Command::MoveCursor(Motion::Down))
        );
        assert_eq!(
            resolve(key(KeyCode::Down, NONE), FocusTarget::Explorer),
            Some(Command::MoveSidebarSelection(1))
        );
        assert_eq!(
            resolve(key(KeyCode::Down, NONE), FocusTarget::Menu),
            Some(Command::MenuNextItem)
        );
    }

    #[test]
    fn irrelevant_modifiers_do_not_defeat_a_match() {
        let mut event = key(KeyCode::Char('q'), CTRL);
        event.modifiers |= KeyModifiers::META;
        assert_eq!(resolve(event, FocusTarget::Editor), Some(Command::Quit));
    }

    #[test]
    fn an_unbound_key_resolves_to_nothing() {
        assert_eq!(
            resolve(key(KeyCode::Char('x'), NONE), FocusTarget::Explorer),
            None
        );
        assert_eq!(resolve(key(KeyCode::F(4), NONE), FocusTarget::Editor), None);
    }

    #[test]
    fn typing_in_the_editor_inserts_the_character() {
        assert_eq!(
            resolve(key(KeyCode::Char('x'), NONE), FocusTarget::Editor),
            Some(Command::InsertChar('x'))
        );
        // Shift is already applied by the terminal; the char arrives uppercase.
        assert_eq!(
            resolve(
                key(KeyCode::Char('X'), KeyModifiers::SHIFT),
                FocusTarget::Editor
            ),
            Some(Command::InsertChar('X'))
        );
        assert_eq!(
            resolve(key(KeyCode::Char('й'), NONE), FocusTarget::Editor),
            Some(Command::InsertChar('й'))
        );
    }

    #[test]
    fn an_unbound_control_combination_does_not_type_a_letter() {
        assert_eq!(
            resolve(key(KeyCode::Char('k'), CTRL), FocusTarget::Editor),
            None
        );
        assert_eq!(
            resolve(
                key(KeyCode::Char('k'), KeyModifiers::ALT),
                FocusTarget::Editor
            ),
            None
        );
    }

    #[test]
    fn a_bound_shortcut_wins_over_typing_it() {
        assert_eq!(
            resolve(key(KeyCode::Char('q'), CTRL), FocusTarget::Editor),
            Some(Command::Quit)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('s'), CTRL), FocusTarget::Editor),
            Some(Command::Save)
        );
    }

    #[test]
    fn typing_only_happens_where_there_is_something_to_type_into() {
        for focus in [
            FocusTarget::Explorer,
            FocusTarget::GitPanel,
            FocusTarget::Menu,
        ] {
            assert_eq!(resolve(key(KeyCode::Char('a'), NONE), focus), None);
        }
        // The two that do take a letter, and put it in different places.
        assert_eq!(
            resolve(key(KeyCode::Char('a'), NONE), FocusTarget::Editor),
            Some(Command::InsertChar('a'))
        );
        assert_eq!(
            resolve(key(KeyCode::Char('a'), NONE), FocusTarget::Search),
            Some(Command::SearchInputChar('a'))
        );
    }

    #[test]
    fn shift_and_a_motion_extend_the_selection() {
        assert_eq!(
            resolve(key(KeyCode::Right, SHIFT), FocusTarget::Editor),
            Some(Command::ExtendSelection(Motion::Right))
        );
        assert_eq!(
            resolve(key(KeyCode::End, SHIFT), FocusTarget::Editor),
            Some(Command::ExtendSelection(Motion::End))
        );
        assert_eq!(
            resolve(key(KeyCode::Left, CTRL_SHIFT), FocusTarget::Editor),
            Some(Command::ExtendSelection(Motion::WordLeft))
        );
        // The same key without Shift still only moves.
        assert_eq!(
            resolve(key(KeyCode::Right, NONE), FocusTarget::Editor),
            Some(Command::MoveCursor(Motion::Right))
        );
    }

    #[test]
    fn selection_keys_do_nothing_outside_the_editor() {
        assert_eq!(
            resolve(key(KeyCode::Right, SHIFT), FocusTarget::Explorer),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('a'), CTRL), FocusTarget::Explorer),
            None
        );
    }

    #[test]
    fn the_clipboard_keys_are_bound_in_the_editor() {
        for (code, command) in [
            (KeyCode::Char('c'), Command::Copy),
            (KeyCode::Char('x'), Command::Cut),
            (KeyCode::Char('v'), Command::Paste),
            (KeyCode::Char('a'), Command::SelectAll),
        ] {
            assert_eq!(resolve(key(code, CTRL), FocusTarget::Editor), Some(command));
        }
    }

    #[test]
    fn the_menu_advertises_the_clipboard_shortcuts_it_now_has() {
        assert_eq!(shortcut_for(&Command::Copy), Some("Ctrl+C"));
        assert_eq!(shortcut_for(&Command::Cut), Some("Ctrl+X"));
        assert_eq!(shortcut_for(&Command::Paste), Some("Ctrl+V"));
        assert_eq!(shortcut_for(&Command::SelectAll), Some("Ctrl+A"));
    }

    #[test]
    fn every_binding_has_a_printable_label() {
        // Phase 9 generates docs/SHORTCUTS.md from this table.
        assert!(BINDINGS.iter().all(|b| !b.label.is_empty()));
    }

    #[test]
    fn the_menu_reads_its_shortcuts_from_the_keymap() {
        assert_eq!(shortcut_for(&Command::Quit), Some("Ctrl+Q"));
        assert_eq!(shortcut_for(&Command::ToggleSidebarMode), Some("Ctrl+B"));
        assert_eq!(shortcut_for(&Command::Save), Some("Ctrl+S"));
        // Not bound yet, so the menu shows no key for it.
        assert_eq!(shortcut_for(&Command::Unimplemented("Undo")), None);
        assert_eq!(shortcut_for(&Command::Undo), Some("Ctrl+Z"));
        assert_eq!(shortcut_for(&Command::Redo), Some("Ctrl+Y"));
        assert_eq!(shortcut_for(&Command::CloseTab), Some("Ctrl+W"));
    }

    #[test]
    fn the_explorer_binds_the_tree_and_the_file_operations() {
        for (code, command) in [
            (KeyCode::Enter, Command::ExplorerActivate),
            (KeyCode::Right, Command::ExplorerExpand),
            (KeyCode::Left, Command::ExplorerCollapse),
            (KeyCode::F(2), Command::RenamePrompt),
            (KeyCode::Delete, Command::DeletePrompt),
            (KeyCode::F(5), Command::ExplorerRefresh),
        ] {
            assert_eq!(
                resolve(key(code, NONE), FocusTarget::Explorer),
                Some(command.clone())
            );
            assert_ne!(
                resolve(key(code, NONE), FocusTarget::Editor),
                Some(command),
                "{code:?} means something else in the editor"
            );
        }
    }

    #[test]
    fn new_file_is_bound_everywhere_because_it_needs_no_selection() {
        for focus in [
            FocusTarget::Editor,
            FocusTarget::Explorer,
            FocusTarget::GitPanel,
        ] {
            assert_eq!(
                resolve(key(KeyCode::Char('n'), CTRL), focus),
                Some(Command::NewFilePrompt)
            );
        }
    }

    #[test]
    fn an_input_dialog_types_where_a_confirmation_would_move_its_buttons() {
        let left = key(KeyCode::Left, NONE);
        assert_eq!(
            super::resolve(left, FocusTarget::Dialog, false),
            Some(Command::DialogMove(-1))
        );
        assert_eq!(
            super::resolve(left, FocusTarget::Dialog, true),
            Some(Command::DialogInputMove(-1))
        );

        assert_eq!(
            super::resolve(key(KeyCode::Char('a'), NONE), FocusTarget::Dialog, true),
            Some(Command::DialogInputChar('a'))
        );
        assert_eq!(
            super::resolve(key(KeyCode::Char('a'), NONE), FocusTarget::Dialog, false),
            None,
            "a confirmation dialog has nothing to type into"
        );
        assert_eq!(
            super::resolve(key(KeyCode::Backspace, NONE), FocusTarget::Dialog, true),
            Some(Command::DialogInputBackspace)
        );
        // Tab is what reaches the buttons once Left and Right are the caret's.
        assert_eq!(
            super::resolve(key(KeyCode::Tab, NONE), FocusTarget::Dialog, true),
            Some(Command::DialogMove(1))
        );
    }

    #[test]
    fn an_input_dialog_is_still_modal() {
        for code in [KeyCode::Char('q'), KeyCode::Char('s'), KeyCode::Char('w')] {
            assert_eq!(
                super::resolve(key(code, CTRL), FocusTarget::Dialog, true),
                None,
                "no global shortcut acts behind a prompt"
            );
        }
        assert_eq!(
            super::resolve(key(KeyCode::Enter, NONE), FocusTarget::Dialog, true),
            Some(Command::DialogActivate)
        );
        assert_eq!(
            super::resolve(key(KeyCode::Esc, NONE), FocusTarget::Dialog, true),
            Some(Command::DialogCancel)
        );
    }
    #[test]
    fn the_search_shortcuts_are_global() {
        for focus in [
            FocusTarget::Editor,
            FocusTarget::Explorer,
            FocusTarget::GitPanel,
        ] {
            assert_eq!(
                resolve(key(KeyCode::Char('f'), CTRL), focus),
                Some(Command::SearchOpen)
            );
            assert_eq!(
                resolve(key(KeyCode::Char('h'), CTRL), focus),
                Some(Command::ReplaceOpen)
            );
            // Repeating a search after the bar is gone is the common case.
            assert_eq!(
                resolve(key(KeyCode::F(3), NONE), focus),
                Some(Command::FindNext)
            );
            assert_eq!(
                resolve(key(KeyCode::F(3), SHIFT), focus),
                Some(Command::FindPrev)
            );
        }
    }

    #[test]
    fn the_search_bar_edits_its_field_rather_than_the_document() {
        let search = FocusTarget::Search;
        assert_eq!(
            resolve(key(KeyCode::Char('x'), NONE), search),
            Some(Command::SearchInputChar('x'))
        );
        assert_eq!(
            resolve(key(KeyCode::Backspace, NONE), search),
            Some(Command::SearchInputBackspace)
        );
        assert_eq!(
            resolve(key(KeyCode::Left, NONE), search),
            Some(Command::SearchInputMove(-1))
        );
        assert_eq!(
            resolve(key(KeyCode::Home, NONE), search),
            Some(Command::SearchInputHome)
        );
        assert_eq!(
            resolve(key(KeyCode::Enter, NONE), search),
            Some(Command::FindNext)
        );
        assert_eq!(
            resolve(key(KeyCode::Up, NONE), search),
            Some(Command::FindPrev)
        );
        assert_eq!(
            resolve(key(KeyCode::Tab, NONE), search),
            Some(Command::SearchToggleField)
        );
        assert_eq!(
            resolve(key(KeyCode::Esc, NONE), search),
            Some(Command::SearchClose)
        );
    }

    #[test]
    fn the_bar_is_not_modal_the_way_a_dialog_is() {
        // A dialog swallows every global key; the search bar does not, so a
        // save while hunting for a word still saves.
        assert_eq!(
            resolve(key(KeyCode::Char('s'), CTRL), FocusTarget::Search),
            Some(Command::Save)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('s'), CTRL), FocusTarget::Dialog),
            None
        );
    }

    #[test]
    fn the_bars_alt_shortcuts_do_not_type_letters() {
        assert_eq!(
            resolve(
                key(KeyCode::Char('c'), KeyModifiers::ALT),
                FocusTarget::Search
            ),
            Some(Command::SearchToggleCase)
        );
        assert_eq!(
            resolve(
                key(KeyCode::Char('a'), KeyModifiers::ALT),
                FocusTarget::Search
            ),
            Some(Command::ReplaceAll)
        );
        // An *unbound* Alt combination types nothing rather than a stray letter.
        assert_eq!(
            resolve(
                key(KeyCode::Char('z'), KeyModifiers::ALT),
                FocusTarget::Search
            ),
            None
        );
    }
}
