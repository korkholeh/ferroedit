//! Modal dialog state.
//!
//! One shape for every popup (SPEC §40): a title, a body, and a row of buttons
//! where each button carries the `Command` that choosing it runs. A dialog
//! therefore needs no event handling of its own — it feeds the same mutation
//! path as the keyboard, the menu and the mouse.
//!
//! Phase 6 added the second kind of body: a text field, for the names that New
//! File, New Folder and Rename need. The variation is in the body alone, so the
//! enum is `DialogBody` and not `DialogState` (ADR-019).

use std::path::Path;

use crate::app::focus::FocusTarget;
use crate::app::input_field::InputField;
use crate::commands::{Command, FileOp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogButton {
    pub label: String,
    /// What choosing this button runs. `None` simply dismisses the dialog,
    /// which is what Cancel is.
    pub command: Option<Command>,
}

impl DialogButton {
    fn new(label: &str, command: Option<Command>) -> Self {
        Self {
            label: label.into(),
            command,
        }
    }
}

/// What a dialog shows above its buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogBody {
    /// A question or a statement — the confirmation dialogs.
    Message(String),
    /// A prompt and a text field — the name a file operation needs.
    Input { prompt: String, field: InputField },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogState {
    pub title: String,
    pub body: DialogBody,
    pub buttons: Vec<DialogButton>,
    pub selected: usize,
    /// Where focus goes when the dialog closes.
    pub return_focus: FocusTarget,
}

impl DialogState {
    /// Asked before a modified tab is closed (SPEC §11).
    ///
    /// Save is the default because it is the answer that cannot lose work, and
    /// because a user who reaches for Enter without reading has then not been
    /// punished for it.
    pub fn unsaved_changes(index: usize, title: &str, return_focus: FocusTarget) -> Self {
        Self::confirm(
            "Unsaved changes",
            format!("{title} has unsaved changes."),
            vec![
                DialogButton::new("Save", Some(Command::SaveAndCloseTab(index))),
                DialogButton::new("Don't Save", Some(Command::CloseTabDiscarding(index))),
                DialogButton::new("Cancel", None),
            ],
            return_focus,
        )
    }

    /// Asked before quitting with modified tabs open.
    ///
    /// Cancel is the default here rather than Save: quitting discards every
    /// dirty buffer at once, so the safe answer is the one that does nothing.
    pub fn unsaved_on_quit(dirty: usize, return_focus: FocusTarget) -> Self {
        let files = if dirty == 1 { "file has" } else { "files have" };
        Self::confirm(
            "Unsaved changes",
            format!("{dirty} {files} unsaved changes."),
            vec![
                DialogButton::new("Cancel", None),
                DialogButton::new("Quit Anyway", Some(Command::QuitDiscarding)),
            ],
            return_focus,
        )
    }

    /// Asked before anything is removed from disk (SPEC §20).
    ///
    /// A directory goes with everything inside it, so the question says so
    /// rather than leaving the user to find out. Cancel is the default: this is
    /// the one dialog in the editor whose other answer cannot be undone.
    pub fn confirm_delete(path: &Path, is_dir: bool, return_focus: FocusTarget) -> Self {
        let name = display_name(path);
        let message = if is_dir {
            format!("Delete {name} and everything in it?")
        } else {
            format!("Delete {name}?")
        };
        Self::confirm(
            "Delete",
            message,
            vec![
                DialogButton::new("Cancel", None),
                DialogButton::new("Delete", Some(Command::DeletePath(path.to_path_buf()))),
            ],
            return_focus,
        )
    }

    /// Asks for the name of a new file in `parent`.
    pub fn new_file(parent: &Path, return_focus: FocusTarget) -> Self {
        Self::input(
            "New File",
            format!("Create in {}", display_name(parent)),
            "",
            "Create",
            FileOp::CreateFile {
                parent: parent.to_path_buf(),
            },
            return_focus,
        )
    }

    /// Asks for the name of a new directory in `parent`.
    pub fn new_directory(parent: &Path, return_focus: FocusTarget) -> Self {
        Self::input(
            "New Folder",
            format!("Create in {}", display_name(parent)),
            "",
            "Create",
            FileOp::CreateDirectory {
                parent: parent.to_path_buf(),
            },
            return_focus,
        )
    }

    /// Asks for a new name, pre-filled with the current one.
    pub fn rename(path: &Path, return_focus: FocusTarget) -> Self {
        let current = display_name(path);
        Self::input(
            "Rename",
            format!("Rename {current} to"),
            &current,
            "Rename",
            FileOp::Rename {
                path: path.to_path_buf(),
            },
            return_focus,
        )
    }

    /// Asks for a path to open (SPEC §6).
    ///
    /// The explorer is how a project is browsed; this is how a path outside it
    /// — or one nobody wants to click down to — is reached (ADR-029). A
    /// relative answer is resolved against `base`, which is the directory the
    /// prompt names.
    pub fn open_path(base: &Path, return_focus: FocusTarget) -> Self {
        Self::input(
            "Open",
            format!("Open in {}", display_name(base)),
            "",
            "Open",
            FileOp::Open {
                base: base.to_path_buf(),
            },
            return_focus,
        )
    }

    /// Asks where to write a tab, pre-filled with the name it already has.
    ///
    /// The field starts as a name rather than a full path because that is the
    /// answer to the common question — "the same place, another name" — and a
    /// path typed over it still works.
    pub fn save_as(index: usize, base: &Path, name: &str, return_focus: FocusTarget) -> Self {
        Self::input(
            "Save As",
            format!("Save in {}", display_name(base)),
            name,
            "Save",
            FileOp::SaveAs {
                index,
                base: base.to_path_buf(),
            },
            return_focus,
        )
    }

    /// The Help menu's About entry. One button, and it only dismisses.
    pub fn about(version: &str, return_focus: FocusTarget) -> Self {
        Self::confirm(
            "About",
            format!("FerroEdit {version} — a terminal text editor."),
            vec![DialogButton::new("OK", None)],
            return_focus,
        )
    }

    fn confirm(
        title: &str,
        message: String,
        buttons: Vec<DialogButton>,
        return_focus: FocusTarget,
    ) -> Self {
        Self {
            title: title.into(),
            body: DialogBody::Message(message),
            buttons,
            selected: 0,
            return_focus,
        }
    }

    /// The shape every file operation shares: a prompt, a field, and the
    /// confirm button that submits whatever is in it.
    ///
    /// The button carries `SubmitInput`, not the text — the text does not exist
    /// yet when the dialog is built. Activating a button is what pairs the two
    /// (see `commands::execute`).
    fn input(
        title: &str,
        prompt: String,
        value: &str,
        confirm_label: &str,
        operation: FileOp,
        return_focus: FocusTarget,
    ) -> Self {
        Self {
            title: title.into(),
            body: DialogBody::Input {
                prompt,
                field: InputField::new(value),
            },
            buttons: vec![
                DialogButton::new(confirm_label, Some(Command::SubmitInput(operation))),
                DialogButton::new("Cancel", None),
            ],
            selected: 0,
            return_focus,
        }
    }

    /// The text field, when this dialog has one. `None` is what makes a
    /// confirmation dialog route Left and Right to its buttons instead of to a
    /// caret.
    pub fn field(&self) -> Option<&InputField> {
        match &self.body {
            DialogBody::Input { field, .. } => Some(field),
            DialogBody::Message(_) => None,
        }
    }

    pub fn field_mut(&mut self) -> Option<&mut InputField> {
        match &mut self.body {
            DialogBody::Input { field, .. } => Some(field),
            DialogBody::Message(_) => None,
        }
    }

    /// The line above the buttons: the message, or the input's prompt.
    pub fn prompt(&self) -> &str {
        match &self.body {
            DialogBody::Message(message) => message,
            DialogBody::Input { prompt, .. } => prompt,
        }
    }

    /// Moves the selection along the button row, wrapping at both ends.
    pub fn step(&mut self, delta: i16) {
        if self.buttons.is_empty() {
            return;
        }
        let len = self.buttons.len() as i16;
        self.selected = (self.selected as i16 + delta).rem_euclid(len) as usize;
    }

    /// The command behind a button, or `None` for a button that only dismisses.
    ///
    /// An index past the end is not an error: it is a click on the padding
    /// between buttons, and dismissing is the least surprising answer to that.
    pub fn command_at(&self, index: usize) -> Option<Command> {
        self.buttons.get(index).and_then(|b| b.command.clone())
    }
}

/// A path as the user should read it in a dialog: its own name, since the
/// dialog is about one entry in a tree they are looking at.
fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::coords::CharIdx;
    use std::path::PathBuf;

    #[test]
    fn the_close_dialog_defaults_to_the_answer_that_keeps_the_work() {
        let dialog = DialogState::unsaved_changes(2, "main.rs", FocusTarget::Editor);
        assert_eq!(dialog.buttons[dialog.selected].label, "Save");
        assert_eq!(
            dialog.command_at(0),
            Some(Command::SaveAndCloseTab(2)),
            "and Save closes the tab it was asked about, not the active one"
        );
        assert_eq!(dialog.command_at(2), None, "Cancel only dismisses");
    }

    #[test]
    fn the_quit_dialog_defaults_to_doing_nothing() {
        let dialog = DialogState::unsaved_on_quit(3, FocusTarget::Editor);
        assert_eq!(dialog.buttons[dialog.selected].label, "Cancel");
        assert_eq!(dialog.prompt(), "3 files have unsaved changes.");
        assert_eq!(
            DialogState::unsaved_on_quit(1, FocusTarget::Editor).prompt(),
            "1 file has unsaved changes."
        );
    }

    #[test]
    fn the_button_selection_wraps_in_both_directions() {
        let mut dialog = DialogState::unsaved_changes(0, "a.txt", FocusTarget::Editor);
        dialog.step(-1);
        assert_eq!(dialog.selected, 2);
        dialog.step(1);
        assert_eq!(dialog.selected, 0);
    }

    #[test]
    fn a_click_past_the_last_button_dismisses_rather_than_guessing() {
        let dialog = DialogState::unsaved_changes(0, "a.txt", FocusTarget::Editor);
        assert_eq!(dialog.command_at(99), None);
    }

    #[test]
    fn the_delete_dialog_says_what_goes_with_a_directory() {
        let file =
            DialogState::confirm_delete(Path::new("/p/notes.md"), false, FocusTarget::Explorer);
        assert_eq!(file.prompt(), "Delete notes.md?");
        assert_eq!(file.buttons[file.selected].label, "Cancel");

        let dir = DialogState::confirm_delete(Path::new("/p/src"), true, FocusTarget::Explorer);
        assert_eq!(dir.prompt(), "Delete src and everything in it?");
        assert_eq!(
            dir.command_at(1),
            Some(Command::DeletePath(PathBuf::from("/p/src")))
        );
    }

    #[test]
    fn rename_starts_from_the_current_name_with_the_caret_after_it() {
        let dialog = DialogState::rename(Path::new("/p/main.rs"), FocusTarget::Explorer);
        let field = dialog.field().expect("a text field");
        assert_eq!(field.value, "main.rs");
        assert_eq!(field.cursor, CharIdx(7));
        assert_eq!(dialog.prompt(), "Rename main.rs to");
        assert_eq!(dialog.buttons[0].label, "Rename");
    }

    #[test]
    fn a_new_file_dialog_starts_empty_and_submits_into_the_directory_it_was_opened_in() {
        let dialog = DialogState::new_file(Path::new("/p/src"), FocusTarget::Explorer);
        assert_eq!(dialog.field().unwrap().value, "");
        assert_eq!(
            dialog.command_at(0),
            Some(Command::SubmitInput(FileOp::CreateFile {
                parent: PathBuf::from("/p/src")
            }))
        );
    }

    #[test]
    fn a_confirmation_dialog_has_no_field_at_all() {
        assert!(DialogState::unsaved_on_quit(1, FocusTarget::Editor)
            .field()
            .is_none());
    }
}
