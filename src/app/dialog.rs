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
use crate::git::models::Branch;

/// The most rows a list body shows at once. Past it the list scrolls: a picker
/// taller than a short terminal is a dialog that cannot be closed.
pub const MAX_LIST_ROWS: usize = 10;

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

/// One row of a list body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub label: String,
    /// What choosing this row runs.
    pub command: Command,
    /// Drawn with git's own `*`: the branch `HEAD` is already on. It is not the
    /// selection — the selection is the highlight, and it starts here.
    pub current: bool,
}

/// What a dialog shows above its buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogBody {
    /// A question or a statement — the confirmation dialogs.
    Message(String),
    /// A prompt and a text field — the name a file operation needs.
    Input { prompt: String, field: InputField },
    /// A prompt and a list to choose from — the branch picker (SPEC §33).
    ///
    /// ADR-019 left this open and ADR-029 deferred it, both for the same
    /// reason: every list the editor needed until now had a pane behind it that
    /// already listed the same things better. A branch list has no such pane
    /// (ADR-035).
    List {
        prompt: String,
        items: Vec<ListItem>,
        selected: usize,
        scroll: usize,
    },
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

    /// Asked once per modified tab while quitting (ADR-047).
    ///
    /// The same three answers as closing one tab, because it is the same
    /// question: a quit with four dirty files is four closes and then an exit.
    /// `remaining` counts this file and the ones still to be asked about. It
    /// goes in the *title* and not the message: a walk that says nothing about
    /// its length is a dialog that looks like it reappeared, and a sentence
    /// with a counter bolted on is what pushes the box past a 40-column
    /// terminal for any ordinary file name.
    ///
    /// Save is the default, which the all-at-once prompt this replaced could
    /// not afford: its Enter discarded every dirty buffer, so the safe answer
    /// there was the one that did nothing. Here Enter saves this file and asks
    /// about the next, so holding it down saves everything and quits.
    pub fn unsaved_on_quit(
        index: usize,
        title: &str,
        remaining: usize,
        return_focus: FocusTarget,
    ) -> Self {
        let heading = if remaining > 1 {
            format!("Unsaved changes ({remaining} left)")
        } else {
            "Unsaved changes".to_string()
        };
        Self::confirm(
            &heading,
            format!("{title} has unsaved changes."),
            vec![
                DialogButton::new("Save", Some(Command::SaveAndQuit(index))),
                DialogButton::new("Don't Save", Some(Command::DiscardAndQuit(index))),
                DialogButton::new("Cancel", None),
            ],
            return_focus,
        )
    }

    /// Asked when a file changed underneath a buffer that has unsaved changes
    /// (ADR-043).
    ///
    /// Keep Mine is the default because it is the answer that does nothing:
    /// the question is asked by a filesystem event rather than by the user, so
    /// it can arrive mid-keystroke, and the reflex `Enter` must not be the one
    /// that throws away what was being typed. Reloading is still undoable, and
    /// the message says so — but an undo the user has to think of is worse than
    /// a default that never needed one.
    pub fn file_changed(index: usize, title: &str, gone: bool, return_focus: FocusTarget) -> Self {
        let message = if gone {
            format!("{title} is gone from disk, and this tab has unsaved changes.")
        } else {
            format!("{title} changed on disk, and this tab has unsaved changes.")
        };
        let mut buttons = vec![DialogButton::new(
            "Keep Mine",
            Some(Command::KeepBuffer(index)),
        )];
        // Nothing to reload from when the file is gone: the buffer is the only
        // copy left, and saving it is what puts it back.
        if !gone {
            buttons.push(DialogButton::new(
                "Reload (undoable)",
                Some(Command::ReloadTab(index)),
            ));
        }
        Self::confirm("Changed on disk", message, buttons, return_focus)
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

    /// Asks for a commit message (SPEC §32).
    ///
    /// The prompt says how many files the commit will include, because the
    /// panel behind the dialog is covered by it and "commit what, exactly?" is
    /// the question a user has at this moment. Commit is the default button:
    /// the dialog was opened on purpose and nothing here is destructive.
    pub fn commit(staged: usize, return_focus: FocusTarget) -> Self {
        let files = if staged == 1 { "file" } else { "files" };
        Self::with_input(
            "Commit",
            format!("Message for {staged} staged {files}"),
            "",
            "Commit",
            Command::SubmitCommit,
            return_focus,
        )
    }

    /// The branch picker (SPEC §33).
    ///
    /// The selection starts on the branch `HEAD` is already on, so opening the
    /// picker and pressing Enter without reading is a no-op rather than a
    /// checkout. New… is a button rather than the `n` of SPEC §33's sketch: a
    /// letter bound in the dialog table would be bound in *every* dialog, and
    /// `n` on a quit prompt must not create a branch.
    pub fn switch_branch(branches: &[Branch], return_focus: FocusTarget) -> Self {
        let items = Self::branch_items(branches, |branch| {
            Command::GitSwitchBranch(branch.switch_target().to_string())
        });
        let selected = items.iter().position(|item| item.current).unwrap_or(0);
        Self::list(
            "Switch Branch",
            branch_count(items.len()),
            items,
            selected,
            vec![
                DialogButton::new("Switch", Some(Command::SubmitListChoice)),
                DialogButton::new("New…", Some(Command::GitNewBranchPrompt)),
                DialogButton::new("Cancel", None),
            ],
            return_focus,
        )
    }

    /// The merge picker (SPEC §35).
    ///
    /// The current branch is not in it: merging a branch into itself is the one
    /// choice with no meaning, and leaving it out is better than a row that
    /// reports "Already up to date."
    pub fn merge_branch(branches: &[Branch], return_focus: FocusTarget) -> Self {
        let others: Vec<Branch> = branches.iter().filter(|b| !b.is_head).cloned().collect();
        let items = Self::branch_items(&others, |branch| Command::GitMerge(branch.name.clone()));
        Self::list(
            "Merge Branch",
            branch_count(items.len()),
            items,
            0,
            vec![
                DialogButton::new("Merge", Some(Command::SubmitListChoice)),
                DialogButton::new("Cancel", None),
            ],
            return_focus,
        )
    }

    /// Asks for the name of a branch to create at `HEAD`.
    pub fn new_branch(head: &str, return_focus: FocusTarget) -> Self {
        Self::with_input(
            "New Branch",
            format!("Branch from {head}"),
            "",
            "Create",
            Command::SubmitBranch,
            return_focus,
        )
    }

    /// Asked before a conflicted file is staged with its markers still in it.
    ///
    /// Staging a conflicted file is how git is told the conflict is resolved,
    /// and the editor now has a merge flow that needs saying so (ADR-036). What
    /// it must not do is let `<<<<<<<` reach a commit unremarked.
    pub fn confirm_conflict_markers(path: &Path, return_focus: FocusTarget) -> Self {
        let name = display_name(path);
        Self::confirm(
            "Conflict markers",
            format!("{name} still has conflict markers."),
            vec![
                DialogButton::new("Cancel", None),
                DialogButton::new("Stage Anyway", Some(Command::GitStageResolved)),
            ],
            return_focus,
        )
    }

    fn branch_items(branches: &[Branch], command: impl Fn(&Branch) -> Command) -> Vec<ListItem> {
        branches
            .iter()
            .map(|branch| ListItem {
                label: branch.name.clone(),
                command: command(branch),
                current: branch.is_head,
            })
            .collect()
    }

    fn list(
        title: &str,
        prompt: String,
        items: Vec<ListItem>,
        selected: usize,
        buttons: Vec<DialogButton>,
        return_focus: FocusTarget,
    ) -> Self {
        Self {
            title: title.into(),
            body: DialogBody::List {
                prompt,
                items,
                selected,
                scroll: 0,
            },
            buttons,
            selected: 0,
            return_focus,
        }
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
        Self::with_input(
            title,
            prompt,
            value,
            confirm_label,
            Command::SubmitInput(operation),
            return_focus,
        )
    }

    /// The same shape for a submission that is not a file operation.
    ///
    /// Phase 11's commit message is the first: it is a prompt, a field and a
    /// confirm button like every other input dialog, but what it submits is a
    /// git job and not a path. `submit` is the command the button carries, and
    /// `activate_dialog_button` is what pairs it with the typed text.
    fn with_input(
        title: &str,
        prompt: String,
        value: &str,
        confirm_label: &str,
        submit: Command,
        return_focus: FocusTarget,
    ) -> Self {
        Self {
            title: title.into(),
            body: DialogBody::Input {
                prompt,
                field: InputField::new(value),
            },
            buttons: vec![
                DialogButton::new(confirm_label, Some(submit)),
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
            _ => None,
        }
    }

    pub fn field_mut(&mut self) -> Option<&mut InputField> {
        match &mut self.body {
            DialogBody::Input { field, .. } => Some(field),
            _ => None,
        }
    }

    /// The line above the buttons: the message, or the prompt of a body that
    /// has one.
    pub fn prompt(&self) -> &str {
        match &self.body {
            DialogBody::Message(message) => message,
            DialogBody::Input { prompt, .. } | DialogBody::List { prompt, .. } => prompt,
        }
    }

    /// The rows a list body holds, and an empty slice for the bodies that are
    /// not one — so the renderer and the layout can ask without matching.
    pub fn items(&self) -> &[ListItem] {
        match &self.body {
            DialogBody::List { items, .. } => items,
            _ => &[],
        }
    }

    /// Which row the list highlights, and the first row it draws.
    pub fn list_view(&self) -> (usize, usize) {
        match &self.body {
            DialogBody::List {
                selected, scroll, ..
            } => (*selected, *scroll),
            _ => (0, 0),
        }
    }

    /// The row the confirm button would act on.
    pub fn selected_item(&self) -> Option<&ListItem> {
        match &self.body {
            DialogBody::List {
                items, selected, ..
            } => items.get(*selected),
            _ => None,
        }
    }

    /// Moves the list selection, clamped at both ends, and scrolls it into
    /// view.
    ///
    /// Clamped rather than wrapping, unlike the button row: a picker is a list
    /// like the explorer's, and a list that jumps from its last row to its
    /// first under a held key is a list that loses the reader's place.
    pub fn step_list(&mut self, delta: i16) {
        let DialogBody::List {
            items,
            selected,
            scroll,
            ..
        } = &mut self.body
        else {
            return;
        };
        if items.is_empty() {
            *selected = 0;
            *scroll = 0;
            return;
        }
        let last = items.len() - 1;
        *selected = if delta < 0 {
            selected.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (*selected + delta as usize).min(last)
        };
        let height = MAX_LIST_ROWS.min(items.len());
        if *selected < *scroll {
            *scroll = *selected;
        } else if *selected >= *scroll + height {
            *scroll = *selected + 1 - height;
        }
        *scroll = (*scroll).min(items.len().saturating_sub(height));
    }

    /// Selects a row by its place in the *visible* window.
    ///
    /// The mouse reports a screen row and not an index into a list it cannot
    /// see all of, so the scroll is added here rather than at the hit-test,
    /// which has the rects but not the dialog. A visible row is by definition
    /// already in view, so nothing has to be scrolled afterwards.
    pub fn select_visible_row(&mut self, row: usize) {
        let DialogBody::List {
            items,
            selected,
            scroll,
            ..
        } = &mut self.body
        else {
            return;
        };
        if items.is_empty() {
            return;
        }
        *selected = (*scroll + row).min(items.len() - 1);
    }

    /// How many rows the body needs, which is what the box's height is built
    /// from.
    pub fn body_height(&self) -> usize {
        match &self.body {
            // A message and a blank row under it: the box would look cramped
            // with the buttons directly beneath the sentence.
            DialogBody::Message(_) => 2,
            DialogBody::Input { .. } => 2,
            DialogBody::List { items, .. } => 1 + MAX_LIST_ROWS.min(items.len()).max(1),
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

/// `3 branches` — the line above a picker's list.
fn branch_count(count: usize) -> String {
    match count {
        1 => "1 branch".to_string(),
        count => format!("{count} branches"),
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

    /// ADR-047: the quit walk asks the close-tab question once per file, so
    /// Enter is the safe answer rather than the destructive one.
    #[test]
    fn the_quit_dialog_asks_about_one_file_and_defaults_to_saving_it() {
        let dialog = DialogState::unsaved_on_quit(2, "a.txt", 3, FocusTarget::Editor);
        assert_eq!(dialog.buttons[dialog.selected].label, "Save");
        assert_eq!(dialog.title, "Unsaved changes (3 left)");
        assert_eq!(dialog.prompt(), "a.txt has unsaved changes.");
        assert_eq!(dialog.command_at(0), Some(Command::SaveAndQuit(2)));
        assert_eq!(dialog.command_at(1), Some(Command::DiscardAndQuit(2)));
        assert_eq!(dialog.command_at(2), None, "Cancel only dismisses");
    }

    /// Every question of a walk asks the close-tab dialog's own sentence; only
    /// the title counts, and the last one does not even do that.
    #[test]
    fn the_last_file_of_a_walk_is_not_counted_at_the_reader() {
        let last = DialogState::unsaved_on_quit(0, "a.txt", 1, FocusTarget::Editor);
        assert_eq!(last.title, "Unsaved changes");
        assert_eq!(last.prompt(), "a.txt has unsaved changes.");
        assert_eq!(
            DialogState::unsaved_changes(0, "a.txt", FocusTarget::Editor).prompt(),
            last.prompt(),
            "a quit is closing every dirty tab, so it is the same question"
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
    fn the_commit_dialog_asks_for_a_message_and_says_what_it_will_commit() {
        let dialog = DialogState::commit(3, FocusTarget::GitPanel);
        assert_eq!(dialog.prompt(), "Message for 3 staged files");
        assert_eq!(dialog.field().unwrap().value, "");
        assert_eq!(dialog.buttons[dialog.selected].label, "Commit");
        assert_eq!(dialog.command_at(0), Some(Command::SubmitCommit));
        assert_eq!(dialog.command_at(1), None);
        assert_eq!(
            DialogState::commit(1, FocusTarget::GitPanel).prompt(),
            "Message for 1 staged file"
        );
    }

    fn branches() -> Vec<Branch> {
        vec![
            Branch {
                name: "main".into(),
                is_head: true,
                remote: false,
            },
            Branch {
                name: "topic".into(),
                is_head: false,
                remote: false,
            },
            Branch {
                name: "origin/topic".into(),
                is_head: false,
                remote: true,
            },
        ]
    }

    /// The picker opens on the branch you are already on, so Enter without
    /// reading is a no-op rather than a checkout.
    #[test]
    fn the_branch_picker_starts_on_the_branch_head_is_already_on() {
        let dialog = DialogState::switch_branch(&branches(), FocusTarget::GitPanel);
        assert_eq!(dialog.prompt(), "3 branches");
        assert_eq!(dialog.list_view(), (0, 0));
        assert_eq!(dialog.selected_item().unwrap().label, "main");
        assert!(dialog.selected_item().unwrap().current);
        assert_eq!(dialog.buttons[dialog.selected].label, "Switch");
        assert_eq!(dialog.command_at(0), Some(Command::SubmitListChoice));
        assert_eq!(dialog.command_at(1), Some(Command::GitNewBranchPrompt));
        assert_eq!(dialog.command_at(2), None);
    }

    /// A remote row switches by its short name: `git switch origin/topic`
    /// would detach HEAD, which is not what clicking it means.
    #[test]
    fn a_remote_row_switches_to_the_local_branch_it_would_create() {
        let dialog = DialogState::switch_branch(&branches(), FocusTarget::GitPanel);
        let items = dialog.items();
        assert_eq!(items[1].command, Command::GitSwitchBranch("topic".into()));
        assert_eq!(items[2].label, "origin/topic");
        assert_eq!(items[2].command, Command::GitSwitchBranch("topic".into()));
    }

    /// Merging a branch into itself has no meaning, so the current one is not
    /// offered — and a merge takes the ref's full name, remote and all.
    #[test]
    fn the_merge_picker_leaves_out_the_branch_you_are_on() {
        let dialog = DialogState::merge_branch(&branches(), FocusTarget::GitPanel);
        let labels: Vec<&str> = dialog.items().iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["topic", "origin/topic"]);
        assert_eq!(
            dialog.items()[1].command,
            Command::GitMerge("origin/topic".into())
        );
        assert_eq!(dialog.buttons[0].label, "Merge");
    }

    #[test]
    fn the_list_selection_is_clamped_at_both_ends_rather_than_wrapping() {
        let mut dialog = DialogState::merge_branch(&branches(), FocusTarget::GitPanel);
        dialog.step_list(-1);
        assert_eq!(
            dialog.list_view().0,
            0,
            "the top does not wrap to the bottom"
        );
        dialog.step_list(5);
        assert_eq!(dialog.list_view().0, 1, "and it stops at the last row");
    }

    #[test]
    fn a_list_taller_than_the_box_scrolls_under_the_selection() {
        let many: Vec<Branch> = (0..20)
            .map(|i| Branch {
                name: format!("b{i:02}"),
                is_head: i == 0,
                remote: false,
            })
            .collect();
        let mut dialog = DialogState::switch_branch(&many, FocusTarget::GitPanel);
        assert_eq!(dialog.body_height(), 1 + MAX_LIST_ROWS);

        dialog.step_list(MAX_LIST_ROWS as i16);
        let (selected, scroll) = dialog.list_view();
        assert_eq!(selected, MAX_LIST_ROWS);
        assert_eq!(scroll, 1, "the selected row is the last one shown");

        // A click reports a screen row, which is an index into what is drawn.
        dialog.select_visible_row(0);
        assert_eq!(dialog.list_view(), (1, 1));
        dialog.select_visible_row(99);
        assert_eq!(
            dialog.list_view().0,
            19,
            "past the end selects the last row"
        );
    }

    #[test]
    fn a_message_and_an_input_body_are_the_same_height_they_always_were() {
        assert_eq!(
            DialogState::unsaved_on_quit(0, "a.txt", 1, FocusTarget::Editor).body_height(),
            2
        );
        assert_eq!(
            DialogState::rename(Path::new("/p/a.rs"), FocusTarget::Explorer).body_height(),
            2
        );
    }

    #[test]
    fn the_conflict_dialog_defaults_to_not_staging_the_markers() {
        let dialog =
            DialogState::confirm_conflict_markers(Path::new("/p/c.txt"), FocusTarget::GitPanel);
        assert_eq!(dialog.prompt(), "c.txt still has conflict markers.");
        assert_eq!(dialog.buttons[dialog.selected].label, "Cancel");
        assert_eq!(dialog.command_at(1), Some(Command::GitStageResolved));
    }

    #[test]
    fn a_body_that_is_not_a_list_answers_the_list_questions_harmlessly() {
        let mut dialog = DialogState::unsaved_on_quit(0, "a.txt", 1, FocusTarget::Editor);
        assert!(dialog.items().is_empty());
        assert!(dialog.selected_item().is_none());
        dialog.step_list(3);
        dialog.select_visible_row(2);
        assert_eq!(dialog.list_view(), (0, 0));
    }

    #[test]
    fn a_confirmation_dialog_has_no_field_at_all() {
        assert!(
            DialogState::unsaved_on_quit(0, "a.txt", 1, FocusTarget::Editor)
                .field()
                .is_none()
        );
    }
}
