//! `execute_command` — the only place `App` is mutated.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::app::dialog::DialogState;
use crate::app::focus::FocusTarget;
use crate::app::input_field::InputField;
use crate::app::search::SearchField;
use crate::app::tabs::active_after_close;
use crate::app::{App, EditorView, LastClick, SidebarMode};
use crate::commands::{Command, FileOp, MENUS};
use crate::editor::coords::VisualCol;
use crate::editor::document::Document;
use crate::filesystem;
use crate::git::{GitJob, JobOutcome};

pub fn execute_command(app: &mut App, command: Command) {
    log::debug!("command {command:?} (focus {:?})", app.focus);
    match command {
        Command::Quit => quit(app),
        Command::QuitDiscarding => app.should_quit = true,

        Command::FocusPane(target) => focus_pane(app, target),
        Command::CycleFocus => focus_pane(app, app.focus.next()),

        Command::ToggleSidebarMode => {
            app.sidebar.mode = match app.sidebar.mode {
                SidebarMode::Explorer => SidebarMode::Git,
                SidebarMode::Git => SidebarMode::Explorer,
            };
            let target = match app.sidebar.mode {
                SidebarMode::Explorer => FocusTarget::Explorer,
                SidebarMode::Git => FocusTarget::GitPanel,
            };
            focus_pane(app, target);
        }

        Command::MoveSidebarSelection(delta) => move_sidebar_selection(app, delta),
        Command::SelectSidebarRow(row) => select_sidebar_row(app, row),
        Command::ScrollSidebar(delta) => scroll_sidebar(app, delta),

        Command::ExplorerActivate => explorer_activate(app),
        Command::ExplorerActivateRow(row) => explorer_activate_row(app, row),
        Command::ExplorerExpand => explorer_expand(app),
        Command::ExplorerCollapse => explorer_collapse(app),
        Command::ExplorerRefresh => {
            refresh_tree(app, None);
            // The tree and the status describe the same directory, so a
            // deliberate "show me what is really there" refreshes both.
            refresh_git(app);
            app.notifications.info("Explorer refreshed");
        }
        Command::GitRefresh => git_rescan(app),
        Command::GitOpenSelected => git_open_selected(app),
        Command::GitStage => git_stage_selected(app, true),
        Command::GitUnstage => git_stage_selected(app, false),
        Command::GitToggleStage => git_toggle_stage(app),
        Command::GitStageAll => start_git_job(app, GitJob::StageAll),
        Command::GitUnstageAll => start_git_job(app, GitJob::UnstageAll),
        Command::GitCommitPrompt => prompt_commit(app),
        // The dialog is what pairs this with the message typed into it, exactly
        // as `SubmitInput` is paired with a name; see `activate_dialog_button`.
        Command::SubmitCommit => log::warn!("a commit was submitted with no dialog open"),
        Command::GitCommit(message) => git_commit(app, &message),
        Command::GitPull => start_git_job(app, GitJob::Pull),
        Command::GitPush => start_git_job(app, GitJob::Push),
        Command::GitJobFinished(outcome) => finish_git_job(app, &outcome),
        Command::ToggleHiddenFiles => toggle_hidden_files(app),

        Command::NewFilePrompt => prompt_new(app, false),
        Command::NewDirectoryPrompt => prompt_new(app, true),
        Command::RenamePrompt => prompt_rename(app),
        Command::DeletePrompt => prompt_delete(app),
        Command::OpenPrompt => prompt_open(app),
        Command::SaveAsPrompt => prompt_save_as(app),
        // The dialog is what pairs this with the name typed into it; see
        // `activate_dialog_button`. Reaching the dispatcher means a producer
        // sent it from somewhere that has no input field to read.
        Command::SubmitInput(operation) => {
            log::warn!("{operation:?} was submitted with no dialog open");
        }
        Command::ApplyFileOp(operation, name) => apply_file_op(app, operation, &name),
        Command::DeletePath(path) => delete_path(app, &path),

        Command::SelectTab(index) => {
            if index < app.tabs.len() {
                app.active_tab = Some(index);
            }
        }
        Command::NextTab => step_tab(app, 1),
        Command::PrevTab => step_tab(app, -1),
        Command::CloseTab => match app.active_tab {
            Some(index) => close_tab(app, index),
            None => app.notifications.info("No tab to close"),
        },
        Command::CloseTabAt(index) => close_tab(app, index),
        Command::CloseTabDiscarding(index) => remove_tab(app, index),
        Command::SaveAndCloseTab(index) => {
            if save_tab(app, index) {
                remove_tab(app, index);
            }
        }

        Command::ScrollEditor(delta) => {
            if let Some(tab) = app.active_mut() {
                let last_line = tab.document.line_count().saturating_sub(1);
                tab.viewport.scroll_lines(delta, last_line);
            }
        }

        Command::MoveCursor(motion) => {
            let page = app.editor_view.height as usize;
            edit(app, |document| document.move_cursor(motion, page));
        }
        Command::ExtendSelection(motion) => {
            let page = app.editor_view.height as usize;
            edit(app, |document| document.extend_cursor(motion, page));
        }
        Command::PlaceCursor { line, col } => {
            focus_pane(app, FocusTarget::Editor);
            remember_click(app, line, col);
            edit(app, |document| document.place_cursor(line, VisualCol(col)));
        }
        Command::ExtendCursorTo { line, col } => {
            edit(app, |document| document.extend_to(line, VisualCol(col)));
        }
        Command::SelectWordAt { line, col } => {
            focus_pane(app, FocusTarget::Editor);
            remember_click(app, line, col);
            edit(app, |document| {
                document.select_word_at(line, VisualCol(col))
            });
        }
        Command::SelectAll => edit(app, Document::select_all),

        Command::InsertChar(ch) => edit(app, |document| document.insert_char(ch)),
        Command::InsertText(text) => edit(app, |document| document.insert_text(&text)),
        Command::InsertNewline => edit(app, Document::insert_newline),
        Command::Backspace => edit(app, Document::backspace),
        Command::Delete => edit(app, Document::delete),

        Command::DialogMove(delta) => {
            if let Some(dialog) = app.dialog.as_mut() {
                dialog.step(delta);
            }
        }
        Command::DialogInputChar(ch) => edit_field(app, |field| field.insert(ch)),
        Command::DialogInputText(text) => edit_field(app, |field| field.insert_str(&text)),
        Command::DialogInputBackspace => edit_field(app, InputField::backspace),
        Command::DialogInputDelete => edit_field(app, InputField::delete),
        Command::DialogInputMove(delta) => edit_field(app, |field| field.step(delta)),
        Command::DialogInputHome => edit_field(app, InputField::home),
        Command::DialogInputEnd => edit_field(app, InputField::end),
        Command::DialogActivate => {
            if let Some(selected) = app.dialog.as_ref().map(|d| d.selected) {
                activate_dialog_button(app, selected);
            }
        }
        Command::DialogActivateButton(index) => activate_dialog_button(app, index),
        Command::DialogCancel => close_dialog(app),

        Command::SearchOpen => open_search(app, false),
        Command::ReplaceOpen => open_search(app, true),
        Command::SearchClose => close_search(app),
        Command::SearchInputChar(ch) => search_field(app, |field| field.insert(ch)),
        Command::SearchInputText(text) => {
            // A pasted path or a copied line arrives with its newlines; a
            // one-line field cannot hold them, and a query never contains one.
            let text = text.replace(['\n', '\r'], " ");
            search_field(app, |field| field.insert_str(&text));
        }
        Command::SearchInputBackspace => search_field(app, InputField::backspace),
        Command::SearchInputDelete => search_field(app, InputField::delete),
        Command::SearchInputMove(delta) => search_field(app, |field| field.step(delta)),
        Command::SearchInputHome => search_field(app, InputField::home),
        Command::SearchInputEnd => search_field(app, InputField::end),
        Command::SearchToggleField => app.search.toggle_field(),
        Command::SearchFocusField(field) => {
            focus_pane(app, FocusTarget::Search);
            app.search.focus_field(field);
        }
        Command::SearchToggleCase => {
            app.search.case_sensitive = !app.search.case_sensitive;
            app.search.invalidate();
            app.notifications.info(if app.search.case_sensitive {
                "Match case: on"
            } else {
                "Match case: off"
            });
        }
        Command::FindNext => step_match(app, 1),
        Command::FindPrev => step_match(app, -1),
        Command::ReplaceCurrent => replace_current(app),
        Command::ReplaceAll => replace_all(app),

        Command::Undo => undo_redo(app, true),
        Command::Redo => undo_redo(app, false),

        Command::Copy => copy_selection(app, false),
        Command::Cut => copy_selection(app, true),
        Command::Paste => paste(app),

        Command::Save => match app.active_tab {
            Some(index) => {
                save_tab(app, index);
            }
            None => app.notifications.warning("No file to save"),
        },

        Command::ShowAbout => show_about(app),

        Command::MenuOpen(index) => open_menu(app, index),
        Command::MenuClose => close_menu(app),
        Command::MenuNextMenu => step_menu(app, 1),
        Command::MenuPrevMenu => step_menu(app, -1),
        Command::MenuNextItem => step_menu_item(app, 1),
        Command::MenuPrevItem => step_menu_item(app, -1),
        Command::MenuActivate => activate_menu_item(app),
        Command::MenuActivateItem(item) => {
            app.menu.item = item;
            activate_menu_item(app);
        }

        Command::Unimplemented(what) => {
            app.notifications
                .warning(format!("{what}: not implemented yet"));
        }
    }
}

fn focus_pane(app: &mut App, target: FocusTarget) {
    if target != FocusTarget::Menu {
        app.menu.open = None;
    }
    // The sidebar mode follows focus so the focused pane is always the visible
    // one; without this, focusing Git while the explorer is shown is invisible.
    match target {
        FocusTarget::Explorer => app.sidebar.mode = SidebarMode::Explorer,
        FocusTarget::GitPanel => app.sidebar.mode = SidebarMode::Git,
        _ => {}
    }
    app.focus = target;
}

fn step_tab(app: &mut App, delta: i16) {
    if app.tabs.is_empty() {
        return;
    }
    let len = app.tabs.len() as i16;
    let current = app.active_tab.unwrap_or(0) as i16;
    let next = (current + delta).rem_euclid(len) as usize;
    app.active_tab = Some(next);
}

/// Runs a document operation on the active tab and scrolls the pane to wherever
/// it left the cursor.
///
/// Every movement and every edit goes through here, so "the cursor is always
/// visible after acting on it" is one rule in one place rather than a line
/// repeated in a dozen match arms.
fn edit(app: &mut App, operation: impl FnOnce(&mut Document)) {
    let view: EditorView = app.editor_view;
    let Some(tab) = app.active_mut() else { return };
    operation(&mut tab.document);
    tab.follow_cursor(view);
}

// --- search ----------------------------------------------------------------

/// Opens the bar, seeded from the selection, with the caret in the query field.
///
/// `Ctrl+H` on an already-open find bar grows it a replacement row rather than
/// starting again, so the query survives changing your mind about what you are
/// doing with it.
fn open_search(app: &mut App, replacing: bool) {
    let seed = app.active().and_then(|tab| tab.document.selected_text());
    app.search.open(replacing, seed);
    // The hits are found by the next `sync_search`; nothing here has to.
    app.search.invalidate();
    focus_pane(app, FocusTarget::Search);
}

fn close_search(app: &mut App) {
    app.search.close();
    focus_pane(app, FocusTarget::Editor);
}

/// Applies an edit to whichever field has the caret, and re-finds the hits.
///
/// Typing into the *replacement* field cannot change what matches, but
/// invalidating anyway is one line against a rule with an exception in it.
fn search_field(app: &mut App, operation: impl FnOnce(&mut InputField)) {
    operation(app.search.active_field_mut());
    app.search.invalidate();
}

/// Selects the next or previous hit and scrolls it into view.
fn step_match(app: &mut App, delta: isize) {
    // Next/Previous are bound globally, so they work with the bar closed — but
    // there is nothing to step through until a query has been typed.
    if app.search.query.value.is_empty() {
        open_search(app, false);
        return;
    }
    app.sync_search();
    let Some(hit) = app.search.step(delta) else {
        app.notifications
            .warning(format!("No matches for {}", app.search.query.value));
        return;
    };
    select_match(app, hit);
    let label = app.search.count_label();
    app.notifications.info(format!("Match {label}"));
}

/// Puts the document's selection on a hit and scrolls to it.
fn select_match(app: &mut App, hit: crate::editor::search::Match) {
    let view: EditorView = app.editor_view;
    if let Some(tab) = app.active_mut() {
        tab.document.select_match(hit);
        tab.follow_cursor(view);
    }
}

/// Rewrites the current hit and steps to the next one.
fn replace_current(app: &mut App) {
    if !ready_to_replace(app) {
        return;
    }
    app.sync_search();
    let Some(hit) = app.search.current_match() else {
        app.notifications
            .warning(format!("No matches for {}", app.search.query.value));
        return;
    };
    let replacement = app.search.replacement.value.clone();
    let view: EditorView = app.editor_view;
    if let Some(tab) = app.active_mut() {
        tab.document.replace_matches(&[hit], &replacement);
        tab.follow_cursor(view);
    }
    // The hits below the one just rewritten have moved, so the list is found
    // again before anything points into it.
    app.sync_search();
    app.notifications.info("Replaced 1 match");
}

/// Rewrites every hit as one undo step (SPEC §23).
fn replace_all(app: &mut App) {
    if !ready_to_replace(app) {
        return;
    }
    app.sync_search();
    if app.search.matches.is_empty() {
        app.notifications
            .warning(format!("No matches for {}", app.search.query.value));
        return;
    }
    let matches = app.search.matches.clone();
    let truncated = app.search.truncated;
    let replacement = app.search.replacement.value.clone();
    let view: EditorView = app.editor_view;
    let Some(tab) = app.active_mut() else { return };
    let count = tab.document.replace_matches(&matches, &replacement);
    tab.follow_cursor(view);
    app.sync_search();
    // A truncated list means there are hits this pass did not see. Saying so is
    // the difference between "run it again" and "it silently half worked".
    if truncated {
        app.notifications.warning(format!(
            "Replaced {count} matches — more than {} were found, run it again",
            crate::app::search::MAX_MATCHES
        ));
    } else {
        app.notifications.info(format!("Replaced {count} matches"));
    }
}

/// Whether a replace can go ahead, complaining on the status bar if not.
fn ready_to_replace(app: &mut App) -> bool {
    if app.active().is_none() {
        app.notifications.warning("No file to replace in");
        return false;
    }
    if app.search.query.value.is_empty() {
        open_search(app, true);
        return false;
    }
    if !app.search.replacing {
        // Reached from the menu with only the find bar open: show the row the
        // replacement is typed into rather than replacing with nothing.
        app.search.open(true, None);
        app.search.focus_field(SearchField::Replacement);
        focus_pane(app, FocusTarget::Search);
        return false;
    }
    true
}

/// Writes one tab to disk, reporting either outcome on the status bar.
///
/// Returns whether the file is now on disk, which is what the confirm dialog's
/// "Save" needs: a tab whose save failed must stay open, or the edit the user
/// asked to keep is the one they lose.
fn save_tab(app: &mut App, index: usize) -> bool {
    let Some(tab) = app.tabs.get_mut(index) else {
        app.notifications.warning("No file to save");
        return false;
    };
    let outcome = tab.document.save();
    let title = tab.document.title().to_string();
    match outcome {
        Ok(()) => {
            // What was just written is a change git has an opinion about.
            refresh_git(app);
            app.notifications.info(format!("Saved {title}"));
            true
        }
        Err(err) => {
            log::error!("save failed: {err}");
            app.notifications.error(format!("Failed to save: {err}"));
            false
        }
    }
}

/// Quits, or asks first when something would be lost by it.
///
/// `is_dirty` is history-aware (ADR-014), so a file edited and then undone back
/// to what is on disk does not hold up the exit.
fn quit(app: &mut App) {
    let dirty = app.tabs.iter().filter(|t| t.document.is_dirty()).count();
    if dirty == 0 {
        app.should_quit = true;
        return;
    }
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::unsaved_on_quit(dirty, return_focus));
}

/// Closes a tab, asking first when it has unsaved changes (SPEC §11).
fn close_tab(app: &mut App, index: usize) {
    let Some(tab) = app.tabs.get(index) else {
        return;
    };
    if !tab.document.is_dirty() {
        remove_tab(app, index);
        return;
    }
    let title = tab.document.title().to_string();
    let return_focus = dialog_return_focus(app);
    open_dialog(
        app,
        DialogState::unsaved_changes(index, &title, return_focus),
    );
}

/// Drops a tab and picks whichever one takes its place.
///
/// The tab owns its `History`, so closing it is also what frees the undo stack;
/// nothing else holds one, so there is no cross-tab undo to reason about.
fn remove_tab(app: &mut App, index: usize) {
    if index >= app.tabs.len() {
        return;
    }
    let closed = app.tabs.remove(index);
    app.active_tab = active_after_close(app.active_tab, index, app.tabs.len());
    app.notifications
        .info(format!("Closed {}", closed.document.title()));
    // The tab that came forward was last scrolled for whatever the pane size
    // was then, which need not be what it is now.
    let view = app.editor_view;
    if let Some(tab) = app.active_mut() {
        tab.follow_cursor(view);
    }
}

/// Where focus returns to once a dialog is answered.
///
/// A dialog opened from the menu returns to whatever the menu was opened from,
/// not to the menu: the menu bar is closed underneath it, and landing back on a
/// closed menu would leave the keyboard talking to nothing.
fn dialog_return_focus(app: &App) -> FocusTarget {
    match app.focus {
        FocusTarget::Menu => app.menu.return_focus,
        FocusTarget::Dialog => FocusTarget::Editor,
        other => other,
    }
}

fn open_dialog(app: &mut App, dialog: DialogState) {
    app.menu.open = None;
    app.dialog = Some(dialog);
    app.focus = FocusTarget::Dialog;
}

fn close_dialog(app: &mut App) {
    if let Some(dialog) = app.dialog.take() {
        app.focus = dialog.return_focus;
    }
}

/// Runs a dialog button's command, with the dialog already closed.
///
/// Closing first is what keeps this from recursing: no command a button can
/// carry opens another dialog, and the modal state is gone before any of them
/// runs.
///
/// An input dialog's confirm button is the one command that is completed here
/// rather than carried whole: the operation was decided when the dialog opened
/// and the name only exists now, in a field that is about to be dropped.
fn activate_dialog_button(app: &mut App, index: usize) {
    let Some(dialog) = app.dialog.as_ref() else {
        return;
    };
    let typed = || dialog.field().map(|f| f.value.clone()).unwrap_or_default();
    let command = match dialog.command_at(index) {
        Some(Command::SubmitInput(operation)) => Some(Command::ApplyFileOp(operation, typed())),
        Some(Command::SubmitCommit) => Some(Command::GitCommit(typed())),
        other => other,
    };
    close_dialog(app);
    if let Some(command) = command {
        execute_command(app, command);
    }
}

/// Runs an edit on the open dialog's text field, if it has one.
fn edit_field(app: &mut App, operation: impl FnOnce(&mut InputField)) {
    if let Some(field) = app.dialog.as_mut().and_then(DialogState::field_mut) {
        operation(field);
    }
}

/// Undoes or redoes one step on the active tab.
///
/// An empty stack is not a failure — it is the beginning (or the end) of the
/// document's history, and saying so is friendlier than a key that appears to
/// have missed.
fn undo_redo(app: &mut App, undo: bool) {
    let view: EditorView = app.editor_view;
    let Some(tab) = app.active_mut() else { return };
    let moved = if undo {
        tab.document.undo()
    } else {
        tab.document.redo()
    };
    tab.follow_cursor(view);
    if !moved {
        let what = if undo { "undo" } else { "redo" };
        app.notifications.info(format!("Nothing to {what}"));
    }
}

/// Copies the selection, and removes it when this is a cut.
///
/// Nothing selected is not a failure: there is simply nothing to copy, and
/// saying so on the status bar is friendlier than a silent no-op. A clipboard
/// that cannot reach the terminal is reported the same way and the editor
/// carries on — a copy failing must never cost the user their edit (ADR-005).
fn copy_selection(app: &mut App, cut: bool) {
    let Some(text) = app.active().and_then(|tab| tab.document.selected_text()) else {
        app.notifications.info("Nothing selected");
        return;
    };
    let chars = text.chars().count();
    let outcome = app.clipboard.set(&text);
    if cut {
        edit(app, |document| {
            document.delete_selection();
        });
    }
    match outcome {
        Ok(()) => {
            let verb = if cut { "Cut" } else { "Copied" };
            app.notifications.info(format!("{verb} {chars} characters"));
        }
        Err(err) => {
            log::warn!("clipboard write failed: {err}");
            app.notifications
                .warning(format!("Copied to the internal clipboard only: {err}"));
        }
    }
}

/// Inserts whatever the clipboard holds, replacing the selection.
fn paste(app: &mut App) {
    match app.clipboard.get() {
        Ok(text) => execute_command(app, Command::InsertText(text)),
        Err(err) => {
            log::debug!("nothing to paste: {err}");
            app.notifications.info("The clipboard is empty");
        }
    }
}

/// Records where the editor was last clicked, so the next click can tell
/// whether it is the second half of a double-click.
fn remember_click(app: &mut App, line: usize, col: usize) {
    app.last_click = Some(LastClick {
        at: Instant::now(),
        line,
        col,
    });
}

// --- explorer --------------------------------------------------------------

/// Keeps the explorer's selection on a row that exists and on screen.
fn follow_explorer(app: &mut App) {
    let rows = app.explorer_rows as usize;
    app.sidebar.follow_selection(rows);
}

/// Opens the selected file, or opens and closes the selected directory.
///
/// One command for Enter and for a click, because "activate what is selected"
/// is one idea: a tree where the mouse opened files and the keyboard only
/// expanded them would be two trees.
fn explorer_activate(app: &mut App) {
    let Some(row) = app.sidebar.selected_row() else {
        app.notifications.info("The explorer is empty");
        return;
    };
    let (path, is_dir) = (row.path.clone(), row.kind.is_dir());
    if is_dir {
        app.sidebar.tree.toggle(&path);
        follow_explorer(app);
        return;
    }
    open_and_report(app, &path);
}

fn explorer_activate_row(app: &mut App, row: usize) {
    focus_pane(app, FocusTarget::Explorer);
    if row >= app.sidebar.tree.len() {
        // A click on the empty space below the last row selects nothing; it has
        // already done the one useful thing, which is to focus the pane.
        return;
    }
    app.sidebar.selected = row;
    follow_explorer(app);
    explorer_activate(app);
}

/// Right: opens a closed directory, and steps into an open one.
fn explorer_expand(app: &mut App) {
    let Some(row) = app.sidebar.selected_row() else {
        return;
    };
    if !row.kind.is_dir() {
        return;
    }
    let (path, expanded, depth) = (row.path.clone(), row.expanded, row.depth);
    if !expanded {
        app.sidebar.tree.expand(&path);
        follow_explorer(app);
        return;
    }
    // Stepping in only makes sense when there is something inside: the row
    // after an empty open directory is its next sibling.
    let next = app.sidebar.selected + 1;
    if app.sidebar.tree.row(next).is_some_and(|r| r.depth > depth) {
        app.sidebar.selected = next;
    }
    follow_explorer(app);
}

/// Left: closes an open directory, and steps out of anything else.
fn explorer_collapse(app: &mut App) {
    let Some(row) = app.sidebar.selected_row() else {
        return;
    };
    let (path, open_dir) = (row.path.clone(), row.kind.is_dir() && row.expanded);
    if open_dir {
        app.sidebar.tree.collapse(&path);
    } else if let Some(parent) = path.parent() {
        if let Some(index) = app.sidebar.tree.index_of(parent) {
            app.sidebar.selected = index;
        }
    }
    follow_explorer(app);
}

fn toggle_hidden_files(app: &mut App) {
    let show = !app.sidebar.tree.show_hidden();
    let selected = app.sidebar.selected_row().map(|row| row.path.clone());
    app.sidebar.tree.set_show_hidden(show);
    select_path(app, selected.as_deref());
    app.notifications.info(if show {
        "Showing hidden and ignored files"
    } else {
        "Hiding hidden and ignored files"
    });
}

/// Re-reads the tree and puts the selection back where it should be.
///
/// `select` is where a file operation wants the selection to land; without one,
/// the row that was selected before the refresh is looked for again. A path
/// that is no longer in the tree — it was deleted, or it is ignored — leaves the
/// selection clamped to what is there.
fn refresh_tree(app: &mut App, select: Option<&Path>) {
    let previous = app.sidebar.selected_row().map(|row| row.path.clone());
    app.sidebar.tree.refresh();
    let wanted = select.map(Path::to_path_buf).or(previous);
    select_path(app, wanted.as_deref());
}

fn select_path(app: &mut App, path: Option<&Path>) {
    if let Some(index) = path.and_then(|path| app.sidebar.tree.reveal(path)) {
        app.sidebar.selected = index;
    }
    follow_explorer(app);
}

/// Opens a file — from the explorer, from a new file just created, or from
/// the Open dialog — reporting whatever went wrong.
fn open_and_report(app: &mut App, path: &Path) {
    match app.open_path(path, None) {
        Ok(()) => {
            let title = display_name(path);
            app.notifications.info(format!("Opened {title}"));
        }
        Err(err) => {
            log::error!("could not open {}: {err}", path.display());
            app.notifications.error(format!("{err}"));
        }
    }
}

// --- file operations (SPEC §20) ---------------------------------------------

/// The directory a new entry goes into.
///
/// A directory is selected: inside it. A file is selected: next to it. Nothing
/// is selected, or the pane is empty: the workspace root, which is the only
/// directory that is certainly there.
fn explorer_parent(app: &App) -> PathBuf {
    let root = || app.workspace.root().to_path_buf();
    match app.sidebar.selected_row() {
        Some(row) if row.kind.is_dir() => row.path.clone(),
        Some(row) => row.path.parent().map_or_else(root, Path::to_path_buf),
        None => root(),
    }
}

fn prompt_new(app: &mut App, directory: bool) {
    let parent = explorer_parent(app);
    let return_focus = dialog_return_focus(app);
    let dialog = if directory {
        DialogState::new_directory(&parent, return_focus)
    } else {
        DialogState::new_file(&parent, return_focus)
    };
    open_dialog(app, dialog);
}

/// Asks for a path to open, relative to the directory the explorer is in.
///
/// The base is the explorer's directory rather than the process's: the user is
/// looking at a tree, and a bare name means the place they are looking at.
fn prompt_open(app: &mut App) {
    let base = explorer_parent(app);
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::open_path(&base, return_focus));
}

/// Asks where to write the active tab (SPEC §6).
///
/// The prompt starts in the file's own directory under its own name, so
/// confirming it unchanged is a plain save rather than a surprise.
fn prompt_save_as(app: &mut App) {
    let Some(index) = app.active_tab.filter(|i| *i < app.tabs.len()) else {
        app.notifications.warning("No file to save");
        return;
    };
    let document = &app.tabs[index].document;
    let (base, name) = match document.path() {
        Some(path) => (
            path.parent()
                .map_or_else(|| app.workspace.root().to_path_buf(), Path::to_path_buf),
            display_name(path),
        ),
        None => (
            app.workspace.root().to_path_buf(),
            document.title().to_string(),
        ),
    };
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::save_as(index, &base, &name, return_focus));
}

fn show_about(app: &mut App) {
    let return_focus = dialog_return_focus(app);
    open_dialog(
        app,
        DialogState::about(env!("CARGO_PKG_VERSION"), return_focus),
    );
}

fn prompt_rename(app: &mut App) {
    let Some(path) = app.sidebar.selected_row().map(|row| row.path.clone()) else {
        app.notifications
            .warning("Select something in the explorer first");
        return;
    };
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::rename(&path, return_focus));
}

fn prompt_delete(app: &mut App) {
    let Some(row) = app.sidebar.selected_row() else {
        app.notifications
            .warning("Select something in the explorer first");
        return;
    };
    let (path, is_dir) = (row.path.clone(), row.kind.is_dir());
    let return_focus = dialog_return_focus(app);
    open_dialog(
        app,
        DialogState::confirm_delete(&path, is_dir, return_focus),
    );
}

/// Runs a file operation and puts the tree, the tabs and the status bar back in
/// step with the disk.
fn apply_file_op(app: &mut App, operation: FileOp, name: &str) {
    // Open and Save As are answered with a path rather than a name, and both
    // report through the paths that already say "Opened …" and "Saved …", so
    // neither reaches the create/rename tail below.
    match &operation {
        FileOp::Open { base } => return open_typed_path(app, base, name),
        FileOp::SaveAs { index, base } => return save_tab_as(app, *index, base, name),
        _ => {}
    }

    let outcome = match &operation {
        FileOp::CreateFile { parent } => filesystem::create_file(parent, name),
        FileOp::CreateDirectory { parent } => filesystem::create_directory(parent, name),
        FileOp::Rename { path } => filesystem::rename(path, name),
        // Handled above; the match stays exhaustive rather than defaulting.
        FileOp::Open { .. } | FileOp::SaveAs { .. } => return,
    };
    let path = match outcome {
        Ok(path) => path,
        Err(err) => {
            log::error!("{operation:?} failed: {err}");
            app.notifications.error(format!("{err}"));
            return;
        }
    };

    if let FileOp::Rename { path: old } = &operation {
        rename_open_tabs(app, old, &path);
    }
    refresh_tree(app, Some(&path));
    refresh_git(app);
    // A new file is opened straight away: naming one and then having to find it
    // in the tree is a step nobody wants. The notification comes after, because
    // opening writes one of its own and there is only one status line.
    if matches!(operation, FileOp::CreateFile { .. }) {
        open_and_report(app, &path);
    }
    app.notifications.info(format!(
        "{} {}",
        operation.past_tense(),
        display_name(&path)
    ));
}

/// Opens whatever was typed into the Open dialog.
///
/// A path with nothing behind it opens as an empty buffer, exactly as a file
/// named on the command line does: "open a file that is not there yet" is how
/// a new file outside the tree gets started.
fn open_typed_path(app: &mut App, base: &Path, typed: &str) {
    let Some(path) = resolve_typed_path(base, typed) else {
        app.notifications.warning("No path given");
        return;
    };
    if path.is_dir() {
        app.notifications
            .error(format!("{} is a directory", display_name(&path)));
        return;
    }
    open_and_report(app, &path);
}

/// Writes a tab under a new path and keeps editing it there (SPEC §6).
///
/// The buffer, its history and its save point are untouched — only the name
/// changes — so an undo after Save As still walks back through the edits that
/// were made under the old one.
fn save_tab_as(app: &mut App, index: usize, base: &Path, typed: &str) {
    let Some(path) = resolve_typed_path(base, typed).map(|path| crate::app::absolute(&path)) else {
        app.notifications.warning("No path given");
        return;
    };
    if index >= app.tabs.len() {
        app.notifications.warning("No file to save");
        return;
    }
    if path.is_dir() {
        app.notifications
            .error(format!("{} is a directory", display_name(&path)));
        return;
    }
    // Two tabs at one path would be two histories over one file, and the second
    // save would silently undo the first (SPEC §11).
    if let Some(other) = app.tabs.iter().position(|tab| tab.is_at(&path)) {
        if other != index {
            app.notifications.error(format!(
                "{} is already open in another tab",
                display_name(&path)
            ));
            return;
        }
    }
    app.tabs[index].document.set_path(path.clone());
    if save_tab(app, index) {
        // The file may be new, and the explorer is showing the directory it
        // landed in.
        refresh_tree(app, Some(&path));
    }
}

/// The path a dialog's answer means: absolute as typed, or relative to the
/// directory the prompt named.
///
/// An empty answer is `None` rather than the base directory: an empty field
/// confirmed with Enter is a dialog dismissed, not a request to open a folder.
fn resolve_typed_path(base: &Path, typed: &str) -> Option<PathBuf> {
    let trimmed = typed.trim();
    if trimmed.is_empty() {
        return None;
    }
    let typed = Path::new(trimmed);
    Some(if typed.is_absolute() {
        typed.to_path_buf()
    } else {
        base.join(typed)
    })
}

fn delete_path(app: &mut App, path: &Path) {
    if let Err(err) = filesystem::delete(path) {
        log::error!("could not delete {}: {err}", path.display());
        app.notifications.error(format!("{err}"));
        return;
    }
    let parent = path.parent().map(Path::to_path_buf);
    refresh_tree(app, parent.as_deref());
    refresh_git(app);
    app.notifications
        .info(format!("Deleted {}", display_name(path)));
}

/// Points the open tabs at a renamed file, or at their file inside a renamed
/// directory.
///
/// Without this, saving a file that was renamed underneath the editor would
/// write the old name back and leave two copies on disk.
fn rename_open_tabs(app: &mut App, old: &Path, new: &Path) {
    for tab in &mut app.tabs {
        let Some(path) = tab.document.path() else {
            continue;
        };
        let moved = if path == old {
            Some(new.to_path_buf())
        } else {
            path.strip_prefix(old).ok().map(|rest| new.join(rest))
        };
        if let Some(moved) = moved {
            tab.document.set_path(moved);
        }
    }
}

/// A path as it is named on the status bar: the entry itself, not the route to
/// it.
fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| path.display().to_string(), str::to_string)
}

fn sidebar_len(app: &App) -> usize {
    match app.sidebar.mode {
        SidebarMode::Explorer => app.sidebar.tree.len(),
        SidebarMode::Git => app.git.entries().len(),
    }
}

fn move_sidebar_selection(app: &mut App, delta: i16) {
    let max = sidebar_len(app).saturating_sub(1);
    match app.sidebar.mode {
        SidebarMode::Explorer => {
            app.sidebar.selected = shift(app.sidebar.selected, delta, max);
            // The explorer is the only panel that can be taller than its own
            // window, so it is the only one whose view follows its selection.
            follow_explorer(app);
        }
        SidebarMode::Git => {
            app.git.selected = shift(app.git.selected, delta, max);
            follow_git(app);
        }
    }
}

fn select_sidebar_row(app: &mut App, row: usize) {
    let max = sidebar_len(app).saturating_sub(1);
    match app.sidebar.mode {
        SidebarMode::Explorer => {
            app.sidebar.selected = row.min(max);
            follow_explorer(app);
        }
        SidebarMode::Git => {
            app.git.selected = row.min(max);
            follow_git(app);
        }
    }
}

/// Keeps the git panel's selection on an entry and in view.
fn follow_git(app: &mut App) {
    let rows = app.git_rows as usize;
    app.git.follow_selection(rows);
}

/// Re-reads `git status` after something changed the working tree.
///
/// Silent on purpose: it runs after every save and every file operation, and
/// the status bar already says what happened. Outside a repository it is not
/// even a subprocess — `GitState::refresh` returns without running anything.
fn refresh_git(app: &mut App) {
    app.git.refresh();
    follow_git(app);
}

/// The Git menu's Refresh, and `F5` in the panel: looks for the repository
/// again as well as re-reading its status, so a `git init` in the terminal next
/// door does not need a restart to show up.
fn git_rescan(app: &mut App) {
    let root = app.workspace.root().to_path_buf();
    app.git.discover(&root);
    follow_git(app);
    app.notifications.info(app.git.summary());
}

/// Opens the file the panel's selection is on.
///
/// The status lists repository-relative paths, so the root is what turns one
/// back into something to open — and the root is the repository's, not the
/// workspace's, which matters when the editor was opened in a subdirectory.
fn git_open_selected(app: &mut App) {
    let Some(entry) = app.git.selected_entry() else {
        app.notifications.info("Nothing selected in the Git panel");
        return;
    };
    let Some(root) = app.git.root() else {
        app.notifications.warning(app.git.summary());
        return;
    };
    let path = root.join(&entry.path);
    match app.open_path(&path, None) {
        Ok(()) => app.notifications.info(format!("Opened {}", path.display())),
        Err(err) => {
            log::error!("could not open {}: {err}", path.display());
            app.notifications.error(format!("{err}"));
        }
    }
}

/// Stages or unstages the selected file (SPEC §31).
fn git_stage_selected(app: &mut App, stage: bool) {
    let Some(entry) = app.git.selected_entry() else {
        app.notifications.info("Nothing selected in the Git panel");
        return;
    };
    // `git add` on a conflicted file is git's way of saying "I resolved this",
    // and the editor has no diff and no conflict view to justify that claim
    // yet. Refusing is honest; asserting a resolution the user has not made is
    // not (ADR-034).
    if entry.is_conflicted() {
        let name = entry.path.display().to_string();
        app.notifications.warning(format!(
            "{name} is conflicted — resolve it outside the editor"
        ));
        return;
    }
    let paths = vec![entry.path.clone()];
    let job = if stage {
        GitJob::Stage(paths)
    } else {
        GitJob::Unstage(paths)
    };
    start_git_job(app, job);
}

/// The one key that does the obvious thing: stage what is not staged, and
/// unstage what is.
///
/// "Not staged" is the worktree side of the `XY` pair being anything but
/// unmodified — which covers an untracked file, whose index side is blank
/// because there is nothing in the index to describe.
fn git_toggle_stage(app: &mut App) {
    let unstaged = app
        .git
        .selected_entry()
        .is_some_and(|entry| entry.worktree.is_change());
    git_stage_selected(app, unstaged);
}

/// Asks for a commit message, when there is something to commit (SPEC §32).
///
/// Refusing an empty commit here rather than letting git refuse it costs one
/// check and saves the user typing a message for a commit that was never going
/// to happen.
fn prompt_commit(app: &mut App) {
    if !app.git.is_repository() {
        app.notifications.warning(app.git.summary());
        return;
    }
    let staged = app.git.staged_count();
    if staged == 0 {
        app.notifications.info("Nothing staged to commit");
        return;
    }
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::commit(staged, return_focus));
}

fn git_commit(app: &mut App, message: &str) {
    let message = message.trim();
    if message.is_empty() {
        app.notifications.warning("A commit needs a message");
        return;
    }
    start_git_job(app, GitJob::Commit(message.to_string()));
}

/// Hands a job to the worker and says so (SPEC §34).
///
/// This returns immediately in every case: the subprocess runs on the worker
/// thread and the answer arrives later as `GitJobFinished`, so the frame that
/// started a push is drawn without waiting for the network (ADR-033).
fn start_git_job(app: &mut App, job: GitJob) {
    match app.git.start(job) {
        Ok(progress) => app.notifications.info(progress),
        Err(why) => app.notifications.error(why),
    }
}

/// Reports a finished job and re-reads the status it changed.
fn finish_git_job(app: &mut App, outcome: &JobOutcome) {
    app.git.finish(outcome);
    match &outcome.result {
        Ok(said) => app.notifications.info(said.clone()),
        Err(why) => {
            let what = outcome.job.label();
            app.notifications.error(format!("{what} failed: {why}"));
        }
    }
    // Even a failure can have changed the repository — a pull that fetched and
    // then refused to fast-forward has moved the remote-tracking branch — so
    // the status is re-read either way.
    refresh_git(app);
}

fn scroll_sidebar(app: &mut App, delta: i16) {
    let max = sidebar_len(app).saturating_sub(1);
    match app.sidebar.mode {
        SidebarMode::Explorer => app.sidebar.scroll = shift(app.sidebar.scroll, delta, max),
        SidebarMode::Git => app.git.scroll = shift(app.git.scroll, delta, max),
    }
}

fn open_menu(app: &mut App, index: usize) {
    if index >= MENUS.len() {
        return;
    }
    if app.focus != FocusTarget::Menu {
        app.menu.return_focus = app.focus;
    }
    app.menu.open = Some(index);
    app.menu.item = 0;
    app.focus = FocusTarget::Menu;
}

fn close_menu(app: &mut App) {
    app.menu.open = None;
    app.menu.item = 0;
    app.focus = app.menu.return_focus;
}

fn step_menu(app: &mut App, delta: i16) {
    let Some(open) = app.menu.open else { return };
    let len = MENUS.len() as i16;
    let next = (open as i16 + delta).rem_euclid(len) as usize;
    app.menu.open = Some(next);
    app.menu.item = 0;
}

fn step_menu_item(app: &mut App, delta: i16) {
    let Some(open) = app.menu.open else { return };
    let len = MENUS[open].items.len() as i16;
    app.menu.item = (app.menu.item as i16 + delta).rem_euclid(len) as usize;
}

fn activate_menu_item(app: &mut App) {
    let Some(open) = app.menu.open else { return };
    let Some(item) = MENUS[open].items.get(app.menu.item) else {
        return;
    };
    let command = item.command.clone();
    close_menu(app);
    // One level of recursion only: no menu entry produces another Menu* command,
    // so this cannot loop.
    execute_command(app, command);
}

/// Clamped signed step over a `usize` index.
fn shift(value: usize, delta: i16, max: usize) -> usize {
    if delta < 0 {
        value.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        (value + delta as usize).min(max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::coords::CharIdx;
    use crate::editor::cursor::Motion;
    use crate::event::AppEvent;

    fn app() -> App {
        App::fixture()
    }

    // --- git panel (SPEC §28, §30) ----------------------------------------

    #[test]
    fn refreshing_reads_the_branch_and_the_changes_of_a_real_repository() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.write("b.txt", "b\n");

        let mut app = App::fixture_in(repo.path());
        execute_command(&mut app, Command::GitRefresh);

        assert!(app.git.is_repository());
        assert_eq!(app.git.branch_label(), "main");
        assert_eq!(app.git.entries().len(), 1);
        assert_eq!(app.git.summary(), "main — 1 change");
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("main — 1 change")
        );
    }

    #[test]
    fn refreshing_outside_a_repository_says_so_and_leaves_the_editor_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::GitRefresh);

        assert!(!app.git.is_repository());
        assert!(app.git.entries().is_empty());
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Not a Git repository")
        );
    }

    /// Saving is the commonest way the working tree changes from inside the
    /// editor, so the panel has to be right afterwards without being asked.
    #[test]
    fn saving_a_file_puts_it_in_the_git_panel() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let mut app = App::fixture_in(repo.path());
        execute_command(&mut app, Command::GitRefresh);
        assert!(app.git.entries().is_empty(), "the tree starts clean");

        app.open_path(&repo.path().join("a.txt"), None).unwrap();
        execute_command(&mut app, Command::InsertChar('x'));
        execute_command(&mut app, Command::Save);

        assert_eq!(app.git.entries().len(), 1, "{:?}", app.git.entries());
        assert_eq!(
            app.git.entries()[0].worktree,
            crate::git::models::Change::Modified
        );
    }

    #[test]
    fn creating_a_file_from_the_explorer_shows_it_as_untracked() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let mut app = App::fixture_in(repo.path());
        execute_command(&mut app, Command::GitRefresh);
        execute_command(
            &mut app,
            Command::ApplyFileOp(
                FileOp::CreateFile {
                    parent: repo.path().to_path_buf(),
                },
                "new.rs".to_string(),
            ),
        );

        let entries = app.git.entries();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].path, Path::new("new.rs"));
        assert_eq!(entries[0].worktree, crate::git::models::Change::Untracked);
    }

    #[test]
    fn deleting_a_tracked_file_shows_it_as_deleted() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let mut app = App::fixture_in(repo.path());
        execute_command(&mut app, Command::GitRefresh);
        execute_command(&mut app, Command::DeletePath(repo.path().join("a.txt")));

        assert_eq!(app.git.entries().len(), 1);
        assert_eq!(
            app.git.entries()[0].worktree,
            crate::git::models::Change::Deleted
        );
    }

    // --- git actions (SPEC §31, §32, §34) ---------------------------------

    /// An app over a real repository, with the git panel focused and a worker
    /// attached — the state the editor is in when these keys are pressed.
    ///
    /// The receiver comes back with it because a job's answer arrives on the
    /// run loop's channel, and a headless test is its own run loop.
    fn app_over(
        repo: &crate::git::testing::TestRepo,
    ) -> (App, std::sync::mpsc::Receiver<AppEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = App::fixture_in(repo.path());
        app.git.attach_worker(tx);
        app.sidebar.mode = SidebarMode::Git;
        app.focus = FocusTarget::GitPanel;
        app.git_rows = 10;
        execute_command(&mut app, Command::GitRefresh);
        (app, rx)
    }

    /// Runs the loop's job-draining step: block for one outcome, feed it back
    /// through `execute_command`, and keep going while anything is in flight.
    fn settle(app: &mut App, rx: &std::sync::mpsc::Receiver<AppEvent>) {
        while app.git.busy().is_some() {
            let event = rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the git worker answers");
            let AppEvent::GitJob(outcome) = event else {
                panic!("expected a git outcome, got {event:?}");
            };
            execute_command(app, Command::GitJobFinished(outcome));
        }
    }

    /// A repository with one commit, one modified file and one untracked file.
    fn changed_repo() -> crate::git::testing::TestRepo {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.write("a.txt", "b\n");
        repo.write("new.txt", "new\n");
        repo
    }

    /// The Phase 11 acceptance: the command that starts a git job returns
    /// before git has finished, and the panel says what is running.
    #[test]
    fn a_job_returns_immediately_and_the_panel_says_what_is_running() {
        let repo = changed_repo();
        let (mut app, rx) = app_over(&repo);

        execute_command(&mut app, Command::GitStageAll);
        assert_eq!(app.git.busy(), Some("Staging everything…"));
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Staging everything…"),
            "SPEC §34's in-progress line"
        );

        settle(&mut app, &rx);
        assert_eq!(app.git.busy(), None);
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Staged every change")
        );
    }

    #[test]
    fn stage_all_and_unstage_all_move_every_row_and_refresh_the_panel() {
        let repo = changed_repo();
        let (mut app, rx) = app_over(&repo);
        assert_eq!(app.git.entries().len(), 2);

        execute_command(&mut app, Command::GitStageAll);
        settle(&mut app, &rx);
        assert_eq!(app.git.staged_count(), 2, "{:?}", app.git.entries());

        execute_command(&mut app, Command::GitUnstageAll);
        settle(&mut app, &rx);
        assert_eq!(app.git.staged_count(), 0, "{:?}", app.git.entries());
    }

    /// Space is the panel's one-key workflow: stage what is not staged, and
    /// unstage it again when it is.
    #[test]
    fn the_space_key_stages_the_selected_row_and_then_unstages_it() {
        let repo = changed_repo();
        let (mut app, rx) = app_over(&repo);
        let selected = app.git.selected_entry().unwrap().path.clone();

        execute_command(&mut app, Command::GitToggleStage);
        settle(&mut app, &rx);
        let entry = app
            .git
            .entries()
            .iter()
            .find(|e| e.path == selected)
            .expect("still listed");
        assert!(entry.index.is_change(), "{entry:?}");
        assert!(!entry.worktree.is_change(), "{entry:?}");

        execute_command(&mut app, Command::GitToggleStage);
        settle(&mut app, &rx);
        let entry = app
            .git
            .entries()
            .iter()
            .find(|e| e.path == selected)
            .expect("still listed");
        assert!(!entry.index.is_change(), "{entry:?}");
    }

    #[test]
    fn staging_with_an_empty_panel_says_so_and_starts_nothing() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        let (mut app, _rx) = app_over(&repo);

        execute_command(&mut app, Command::GitStage);
        assert_eq!(app.git.busy(), None);
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Nothing selected in the Git panel")
        );
    }

    /// ADR-034: `git add` on a conflicted file is an assertion the editor has
    /// no view to justify yet, so it refuses rather than making it.
    #[test]
    fn a_conflicted_file_is_not_staged() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("c.txt", "base\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["checkout", "-q", "-b", "other"]);
        repo.write("c.txt", "theirs\n");
        repo.commit("theirs");
        repo.run(&["checkout", "-q", "main"]);
        repo.write("c.txt", "ours\n");
        repo.commit("ours");
        let _ = repo.try_run(&["merge", "other"]);

        let (mut app, _rx) = app_over(&repo);
        assert_eq!(app.git.status.conflicts(), 1);
        execute_command(&mut app, Command::GitToggleStage);

        assert_eq!(app.git.busy(), None, "nothing was submitted");
        let message = app.notifications.current().unwrap().message.clone();
        assert!(message.contains("conflicted"), "{message}");
    }

    #[test]
    fn committing_asks_for_a_message_and_then_commits_what_is_staged() {
        let repo = changed_repo();
        let (mut app, rx) = app_over(&repo);
        execute_command(&mut app, Command::GitStageAll);
        settle(&mut app, &rx);

        execute_command(&mut app, Command::GitCommitPrompt);
        let dialog = app.dialog.as_ref().expect("a commit dialog");
        assert_eq!(dialog.prompt(), "Message for 2 staged files");
        assert_eq!(app.focus, FocusTarget::Dialog);

        for character in "add both".chars() {
            execute_command(&mut app, Command::DialogInputChar(character));
        }
        execute_command(&mut app, Command::DialogActivate);
        assert!(app.dialog.is_none(), "the dialog closed before the job ran");
        settle(&mut app, &rx);

        assert!(app.git.status.is_clean(), "{:?}", app.git.entries());
        assert_eq!(repo.run(&["log", "-1", "--pretty=%s"]).trim(), "add both");
        assert_eq!(app.focus, FocusTarget::GitPanel, "focus came back");
    }

    #[test]
    fn committing_with_nothing_staged_never_opens_the_dialog() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);

        execute_command(&mut app, Command::GitCommitPrompt);
        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Nothing staged to commit")
        );
    }

    #[test]
    fn an_empty_commit_message_is_refused_before_git_sees_it() {
        let repo = changed_repo();
        let (mut app, rx) = app_over(&repo);
        execute_command(&mut app, Command::GitStageAll);
        settle(&mut app, &rx);

        execute_command(&mut app, Command::GitCommit("   ".to_string()));
        assert_eq!(app.git.busy(), None);
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("A commit needs a message")
        );
    }

    /// The other half of the acceptance: a failure that `GIT_TERMINAL_PROMPT=0`
    /// turned into a sentence reaches the status bar with the job's name on it.
    #[test]
    fn a_push_with_no_remote_reports_gits_reason_on_the_status_bar() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        let (mut app, rx) = app_over(&repo);

        execute_command(&mut app, Command::GitPush);
        assert_eq!(app.git.busy(), Some("Pushing…"));
        settle(&mut app, &rx);

        let notification = app.notifications.current().expect("a message");
        assert_eq!(
            notification.kind,
            crate::app::notifications::NotificationKind::Error
        );
        assert!(
            notification.message.starts_with("Push failed: "),
            "{}",
            notification.message
        );
    }

    /// A push against a real remote — a bare repository on disk, which is a
    /// remote as far as git is concerned and needs no network.
    #[test]
    fn a_push_to_a_remote_clears_the_ahead_count_in_the_panel() {
        let repo = crate::git::testing::TestRepo::new();
        let remote = tempfile::tempdir().unwrap();
        repo.run(&[
            "-c",
            "init.defaultBranch=main",
            "init",
            "-q",
            "--bare",
            remote.path().to_str().unwrap(),
        ]);
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        repo.run(&["push", "-q", "-u", "origin", "main"]);
        repo.write("a.txt", "b\n");
        repo.commit("second");

        let (mut app, rx) = app_over(&repo);
        assert_eq!(app.git.status.ahead, 1);

        execute_command(&mut app, Command::GitPush);
        settle(&mut app, &rx);
        assert_eq!(app.git.status.ahead, 0, "the panel followed the push");

        execute_command(&mut app, Command::GitPull);
        settle(&mut app, &rx);
        let message = app.notifications.current().unwrap().message.clone();
        assert!(message.contains("up to date"), "{message}");
    }

    /// Enter in the panel opens the file the row is about, and it opens the
    /// one in the repository even when the workspace is a subdirectory of it.
    #[test]
    fn enter_in_the_panel_opens_the_selected_file() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        let selected = app.git.selected_entry().unwrap().path.clone();

        execute_command(&mut app, Command::GitOpenSelected);
        assert_eq!(app.focus, FocusTarget::Editor);
        let title = app.active().expect("a tab").document.title().to_string();
        assert_eq!(title, selected.file_name().unwrap().to_str().unwrap());
    }

    /// Outside a repository nothing is submitted and the panel's own sentence
    /// is what the user is told.
    #[test]
    fn a_git_action_outside_a_repository_is_refused_with_the_panels_own_words() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::fixture_in(dir.path());
        app.git.attach_worker(tx);
        execute_command(&mut app, Command::GitRefresh);

        execute_command(&mut app, Command::GitStageAll);
        assert_eq!(app.git.busy(), None);
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Not a Git repository")
        );
    }

    /// A `GitState` with no worker refuses rather than spawning a thread. Every
    /// headless fixture is in this state, so it must be a message and not a
    /// panic.
    #[test]
    fn a_panel_with_no_worker_refuses_the_job_instead_of_running_it() {
        let repo = changed_repo();
        let mut app = App::fixture_in(repo.path());
        execute_command(&mut app, Command::GitRefresh);
        execute_command(&mut app, Command::GitStageAll);

        assert_eq!(app.git.busy(), None);
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Git operations need the worker thread")
        );
    }

    #[test]
    fn the_git_panel_selection_moves_and_stays_inside_the_list() {
        let mut app = app();
        app.sidebar.mode = SidebarMode::Git;
        app.focus = FocusTarget::GitPanel;
        app.git_rows = 2;

        execute_command(&mut app, Command::MoveSidebarSelection(1));
        assert_eq!(app.git.selected, 1);
        // Four entries, two rows: selecting the last one scrolls the panel.
        execute_command(&mut app, Command::MoveSidebarSelection(1));
        execute_command(&mut app, Command::MoveSidebarSelection(1));
        assert_eq!(app.git.selected, 3);
        assert_eq!(app.git.scroll, 2);
        // And it stops at the end rather than wrapping.
        execute_command(&mut app, Command::MoveSidebarSelection(1));
        assert_eq!(app.git.selected, 3);
    }

    #[test]
    fn quit_sets_the_flag_when_nothing_would_be_lost() {
        let mut app = app();
        app.tabs.retain(|tab| !tab.document.is_dirty());
        assert!(!app.should_quit);
        execute_command(&mut app, Command::Quit);
        assert!(app.should_quit);
        assert!(app.dialog.is_none(), "and nothing was asked");
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_before_it_does() {
        let mut app = app();
        execute_command(&mut app, Command::Quit);
        assert!(!app.should_quit, "the editor is still running");
        let dialog = app.dialog.as_ref().expect("a prompt");
        assert_eq!(dialog.prompt(), "1 file has unsaved changes.");
        assert_eq!(app.focus, FocusTarget::Dialog);

        // Cancel is the default, so Enter without reading changes nothing.
        execute_command(&mut app, Command::DialogActivate);
        assert!(!app.should_quit);
        assert!(app.dialog.is_none());
        assert_eq!(app.focus, FocusTarget::Editor);

        execute_command(&mut app, Command::Quit);
        execute_command(&mut app, Command::DialogMove(1));
        execute_command(&mut app, Command::DialogActivate);
        assert!(app.should_quit, "Quit Anyway does");
    }

    #[test]
    fn tab_stepping_wraps_in_both_directions() {
        let mut app = app();
        let last = app.tabs.len() - 1;
        execute_command(&mut app, Command::PrevTab);
        assert_eq!(app.active_tab, Some(last));
        execute_command(&mut app, Command::NextTab);
        assert_eq!(app.active_tab, Some(0));
    }

    #[test]
    fn selecting_an_out_of_range_tab_is_ignored() {
        let mut app = app();
        execute_command(&mut app, Command::SelectTab(99));
        assert_eq!(app.active_tab, Some(0));
    }

    #[test]
    fn toggling_the_sidebar_moves_focus_with_it() {
        let mut app = app();
        execute_command(&mut app, Command::ToggleSidebarMode);
        assert_eq!(app.sidebar.mode, SidebarMode::Git);
        assert_eq!(app.focus, FocusTarget::GitPanel);
        execute_command(&mut app, Command::ToggleSidebarMode);
        assert_eq!(app.sidebar.mode, SidebarMode::Explorer);
        assert_eq!(app.focus, FocusTarget::Explorer);
    }

    #[test]
    fn closing_the_menu_restores_the_previous_focus() {
        let mut app = app();
        execute_command(&mut app, Command::FocusPane(FocusTarget::Explorer));
        execute_command(&mut app, Command::MenuOpen(0));
        assert_eq!(app.focus, FocusTarget::Menu);
        execute_command(&mut app, Command::MenuClose);
        assert_eq!(app.focus, FocusTarget::Explorer);
    }

    #[test]
    fn the_file_quit_menu_item_quits() {
        let mut app = app();
        app.tabs.retain(|tab| !tab.document.is_dirty());
        execute_command(&mut app, Command::MenuOpen(0));
        let quit_index = MENUS[0]
            .items
            .iter()
            .position(|i| i.command == Command::Quit)
            .unwrap();
        execute_command(&mut app, Command::MenuActivateItem(quit_index));
        assert!(app.should_quit);
        assert_eq!(app.menu.open, None);
    }

    #[test]
    fn undo_and_redo_travel_the_command_path() {
        let mut app = app();
        for ch in "let ".chars() {
            execute_command(&mut app, Command::InsertChar(ch));
        }
        assert_eq!(app.active().unwrap().document.line(0), "let fn main() {");

        execute_command(&mut app, Command::Undo);
        assert_eq!(app.active().unwrap().document.line(0), "letfn main() {");
        execute_command(&mut app, Command::Undo);
        assert_eq!(app.active().unwrap().document.line(0), "fn main() {");

        execute_command(&mut app, Command::Redo);
        execute_command(&mut app, Command::Redo);
        assert_eq!(app.active().unwrap().document.line(0), "let fn main() {");
    }

    #[test]
    fn an_empty_history_says_so_rather_than_appearing_to_miss() {
        let mut app = app();
        execute_command(&mut app, Command::Undo);
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Nothing to undo"
        );
        execute_command(&mut app, Command::Redo);
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Nothing to redo"
        );
    }

    #[test]
    fn the_edit_menu_undo_item_undoes() {
        let mut app = app();
        execute_command(&mut app, Command::InsertChar('x'));
        let edit_menu = MENUS
            .iter()
            .position(|menu| menu.title == "Edit")
            .expect("an Edit menu");
        let undo_index = MENUS[edit_menu]
            .items
            .iter()
            .position(|i| i.command == Command::Undo)
            .expect("an Undo item");
        execute_command(&mut app, Command::MenuOpen(edit_menu));
        execute_command(&mut app, Command::MenuActivateItem(undo_index));
        assert_eq!(app.active().unwrap().document.line(0), "fn main() {");
    }

    #[test]
    fn unimplemented_commands_notify_instead_of_doing_nothing() {
        let mut app = app();
        execute_command(&mut app, Command::Unimplemented("Undo"));
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Undo: not implemented yet"
        );
    }

    #[test]
    fn sidebar_selection_is_clamped_at_both_ends() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let mut app = App::fixture_in(dir.path());
        let last = app.sidebar.rows().len() - 1;
        assert_eq!(last, 2);

        execute_command(&mut app, Command::MoveSidebarSelection(-10));
        assert_eq!(app.sidebar.selected, 0);
        execute_command(&mut app, Command::MoveSidebarSelection(100));
        assert_eq!(app.sidebar.selected, last);
    }

    #[test]
    fn editor_scroll_cannot_run_past_the_last_line() {
        let mut app = app();
        let last = app.active().unwrap().document.line_count() - 1;
        execute_command(&mut app, Command::ScrollEditor(100));
        assert_eq!(app.active().unwrap().viewport.top_line, last);
        execute_command(&mut app, Command::ScrollEditor(-100));
        assert_eq!(app.active().unwrap().viewport.top_line, 0);
    }

    #[test]
    fn the_wheel_scrolls_without_moving_the_cursor() {
        let mut app = app();
        execute_command(&mut app, Command::ScrollEditor(2));
        assert_eq!(app.active().unwrap().document.cursor().line, 0);
    }

    #[test]
    fn typing_goes_into_the_active_document_and_marks_it_dirty() {
        let mut app = app();
        for ch in "let".chars() {
            execute_command(&mut app, Command::InsertChar(ch));
        }
        let document = &app.active().unwrap().document;
        assert_eq!(document.line(0), "letfn main() {");
        assert!(document.is_dirty());
        assert_eq!(document.cursor().column, CharIdx(3));
    }

    #[test]
    fn the_editing_commands_reach_the_document() {
        let mut app = app();
        execute_command(&mut app, Command::MoveCursor(Motion::End));
        execute_command(&mut app, Command::Backspace);
        assert_eq!(app.active().unwrap().document.line(0), "fn main() ");
        execute_command(&mut app, Command::InsertNewline);
        assert_eq!(app.active().unwrap().document.cursor().line, 1);
        execute_command(&mut app, Command::Delete);
        assert_eq!(
            app.active().unwrap().document.line(1),
            "    println!(\"hello\");"
        );
    }

    #[test]
    fn editing_with_no_open_tab_is_a_no_op_not_a_panic() {
        let mut app = App::new(crate::app::workspace::Workspace::from_arg(None).unwrap());
        for command in [
            Command::InsertChar('x'),
            Command::InsertNewline,
            Command::Backspace,
            Command::Delete,
            Command::MoveCursor(Motion::Down),
            Command::ScrollEditor(3),
            Command::PlaceCursor { line: 4, col: 4 },
        ] {
            execute_command(&mut app, command);
        }
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn moving_the_cursor_down_scrolls_the_viewport_to_follow_it() {
        let mut app = app();
        // A pane two rows tall: the third line cannot be reached without one
        // line of scrolling.
        app.editor_view = crate::app::EditorView {
            width: 40,
            height: 2,
        };
        execute_command(&mut app, Command::MoveCursor(Motion::Down));
        assert_eq!(app.active().unwrap().viewport.top_line, 0);
        execute_command(&mut app, Command::MoveCursor(Motion::Down));
        assert_eq!(app.active().unwrap().document.cursor().line, 2);
        assert_eq!(app.active().unwrap().viewport.top_line, 1);
    }

    #[test]
    fn clicking_in_the_editor_places_the_cursor_and_focuses_it() {
        let mut app = app();
        execute_command(&mut app, Command::FocusPane(FocusTarget::Explorer));
        execute_command(&mut app, Command::PlaceCursor { line: 1, col: 4 });
        assert_eq!(app.focus, FocusTarget::Editor);
        let cursor = app.active().unwrap().document.cursor();
        assert_eq!((cursor.line, cursor.column), (1, CharIdx(4)));
    }

    #[test]
    fn shift_navigation_selects_and_plain_navigation_clears_it() {
        let mut app = app();
        execute_command(&mut app, Command::ExtendSelection(Motion::End));
        let document = &app.active().unwrap().document;
        assert_eq!(document.selected_text().as_deref(), Some("fn main() {"));
        execute_command(&mut app, Command::MoveCursor(Motion::Home));
        assert!(app.active().unwrap().document.selection().is_none());
    }

    #[test]
    fn select_all_selects_the_whole_document() {
        let mut app = app();
        execute_command(&mut app, Command::SelectAll);
        assert_eq!(
            app.active().unwrap().document.selected_len(),
            "fn main() {\n    println!(\"hello\");\n}".chars().count()
        );
    }

    #[test]
    fn copy_leaves_the_document_alone_and_cut_removes_the_selection() {
        let mut app = app();
        execute_command(&mut app, Command::ExtendSelection(Motion::End));
        execute_command(&mut app, Command::Copy);
        assert_eq!(app.active().unwrap().document.line(0), "fn main() {");
        assert_eq!(app.clipboard.get().unwrap(), "fn main() {");
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Copied 11 characters"
        );

        execute_command(&mut app, Command::ExtendSelection(Motion::End));
        execute_command(&mut app, Command::Cut);
        assert_eq!(app.active().unwrap().document.line(0), "");
        assert!(app.active().unwrap().document.is_dirty());
        assert_eq!(app.clipboard.get().unwrap(), "fn main() {");
    }

    #[test]
    fn copy_and_cut_round_trip_through_paste() {
        let mut app = app();
        execute_command(&mut app, Command::ExtendSelection(Motion::End));
        execute_command(&mut app, Command::Cut);
        execute_command(&mut app, Command::Paste);
        assert_eq!(
            app.active().unwrap().document.line(0),
            "fn main() {",
            "what was cut comes back where the caret is"
        );
    }

    #[test]
    fn a_multi_line_selection_round_trips() {
        let mut app = app();
        execute_command(&mut app, Command::SelectAll);
        execute_command(&mut app, Command::Copy);
        execute_command(&mut app, Command::MoveCursor(Motion::DocumentEnd));
        execute_command(&mut app, Command::Paste);
        let document = &app.active().unwrap().document;
        assert_eq!(document.line_count(), 5);
        assert_eq!(document.line(3), "    println!(\"hello\");");
    }

    #[test]
    fn copying_with_nothing_selected_says_so_instead_of_copying_a_stale_clipboard() {
        let mut app = app();
        execute_command(&mut app, Command::Copy);
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Nothing selected"
        );
        assert!(app.clipboard.get().is_err(), "and nothing was copied");
    }

    #[test]
    fn pasting_an_empty_clipboard_notifies_instead_of_editing() {
        let mut app = app();
        execute_command(&mut app, Command::Paste);
        assert!(!app.active().unwrap().document.is_dirty());
        assert_eq!(
            app.notifications.current().unwrap().message,
            "The clipboard is empty"
        );
    }

    #[test]
    fn pasting_replaces_the_selection() {
        let mut app = app();
        execute_command(&mut app, Command::InsertText("x".into()));
        execute_command(&mut app, Command::MoveCursor(Motion::Home));
        execute_command(&mut app, Command::ExtendSelection(Motion::End));
        execute_command(&mut app, Command::InsertText("replaced".into()));
        assert_eq!(app.active().unwrap().document.line(0), "replaced");
    }

    #[test]
    fn a_drag_extends_from_the_click_that_started_it() {
        let mut app = app();
        execute_command(&mut app, Command::PlaceCursor { line: 0, col: 3 });
        execute_command(&mut app, Command::ExtendCursorTo { line: 1, col: 4 });
        assert_eq!(
            app.active().unwrap().document.selected_text().as_deref(),
            Some("main() {\n    ")
        );
    }

    #[test]
    fn double_clicking_selects_a_word_and_focuses_the_editor() {
        let mut app = app();
        execute_command(&mut app, Command::FocusPane(FocusTarget::Explorer));
        execute_command(&mut app, Command::SelectWordAt { line: 0, col: 4 });
        assert_eq!(app.focus, FocusTarget::Editor);
        assert_eq!(
            app.active().unwrap().document.selected_text().as_deref(),
            Some("main")
        );
    }

    #[test]
    fn a_click_is_remembered_so_the_next_one_can_pair_with_it() {
        let mut app = app();
        assert!(app.last_click.is_none());
        execute_command(&mut app, Command::PlaceCursor { line: 1, col: 6 });
        let last = app.last_click.expect("the click was recorded");
        assert_eq!((last.line, last.col), (1, 6));
    }

    #[test]
    fn the_selection_commands_are_no_ops_without_an_open_tab() {
        let mut app = App::new(crate::app::workspace::Workspace::from_arg(None).unwrap());
        app.clipboard = crate::editor::clipboard::Clipboard::detached();
        for command in [
            Command::ExtendSelection(Motion::Down),
            Command::ExtendCursorTo { line: 2, col: 2 },
            Command::SelectWordAt { line: 2, col: 2 },
            Command::SelectAll,
            Command::Copy,
            Command::Cut,
            Command::Paste,
            Command::InsertText("text".into()),
        ] {
            execute_command(&mut app, command);
        }
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn the_edit_menu_cuts_copies_and_pastes() {
        let mut app = app();
        execute_command(&mut app, Command::ExtendSelection(Motion::End));
        let edit = MENUS.iter().position(|m| m.title == "Edit").unwrap();
        let cut = MENUS[edit]
            .items
            .iter()
            .position(|i| i.command == Command::Cut)
            .unwrap();
        execute_command(&mut app, Command::MenuOpen(edit));
        execute_command(&mut app, Command::MenuActivateItem(cut));
        assert_eq!(app.active().unwrap().document.line(0), "");
        assert_eq!(app.clipboard.get().unwrap(), "fn main() {");
    }

    #[test]
    fn ten_files_open_and_switch_by_index_and_by_step() {
        let mut app = App::fixture_with_tabs(12);
        assert_eq!(app.tabs.len(), 12);
        execute_command(&mut app, Command::SelectTab(9));
        assert_eq!(app.active().unwrap().document.title(), "file09.rs");
        execute_command(&mut app, Command::NextTab);
        assert_eq!(app.active().unwrap().document.title(), "file10.rs");
        execute_command(&mut app, Command::PrevTab);
        execute_command(&mut app, Command::PrevTab);
        assert_eq!(app.active().unwrap().document.title(), "file08.rs");
    }

    #[test]
    fn closing_a_clean_tab_removes_it_without_asking() {
        let mut app = App::fixture_with_tabs(3);
        execute_command(&mut app, Command::SelectTab(1));
        execute_command(&mut app, Command::CloseTab);
        assert!(app.dialog.is_none());
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(
            app.active().unwrap().document.title(),
            "file02.rs",
            "the tab that slid into its place is the one now in front"
        );
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Closed file01.rs"
        );
    }

    #[test]
    fn closing_a_tab_before_the_active_one_keeps_the_same_file_in_front() {
        let mut app = App::fixture_with_tabs(3);
        execute_command(&mut app, Command::SelectTab(2));
        execute_command(&mut app, Command::CloseTabAt(0));
        assert_eq!(app.active_tab, Some(1));
        assert_eq!(app.active().unwrap().document.title(), "file02.rs");
    }

    #[test]
    fn closing_the_only_tab_leaves_an_empty_editor_rather_than_a_dangling_index() {
        let mut app = App::fixture_with_tabs(1);
        execute_command(&mut app, Command::CloseTab);
        assert!(app.tabs.is_empty());
        assert_eq!(app.active_tab, None);
        assert!(app.active().is_none());
    }

    #[test]
    fn closing_with_no_tabs_open_says_so_instead_of_doing_nothing() {
        let mut app = App::new(crate::app::workspace::Workspace::from_arg(None).unwrap());
        execute_command(&mut app, Command::CloseTab);
        assert_eq!(
            app.notifications.current().unwrap().message,
            "No tab to close"
        );
    }

    #[test]
    fn closing_a_modified_tab_asks_before_it_does() {
        let mut app = app();
        execute_command(&mut app, Command::SelectTab(1));
        execute_command(&mut app, Command::CloseTab);

        let dialog = app.dialog.as_ref().expect("a prompt");
        assert_eq!(dialog.prompt(), "editor.rs has unsaved changes.");
        assert_eq!(app.focus, FocusTarget::Dialog);
        assert_eq!(app.tabs.len(), 3, "and nothing has been closed yet");
    }

    #[test]
    fn cancelling_the_close_prompt_leaves_the_tab_and_the_focus_alone() {
        let mut app = app();
        execute_command(&mut app, Command::SelectTab(1));
        execute_command(&mut app, Command::CloseTab);
        execute_command(&mut app, Command::DialogCancel);

        assert!(app.dialog.is_none());
        assert_eq!(app.tabs.len(), 3);
        assert_eq!(app.focus, FocusTarget::Editor);
    }

    #[test]
    fn the_prompts_dont_save_button_closes_the_tab_and_drops_the_edit() {
        let mut app = app();
        execute_command(&mut app, Command::SelectTab(1));
        execute_command(&mut app, Command::CloseTab);
        // Save, Don't Save, Cancel — the second one.
        execute_command(&mut app, Command::DialogActivateButton(1));

        assert!(app.dialog.is_none());
        assert_eq!(app.tabs.len(), 2);
        assert!(app.tabs.iter().all(|t| t.document.title() != "editor.rs"));
    }

    #[test]
    fn the_prompts_save_button_writes_the_file_and_then_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, "hello\n").unwrap();

        let mut app =
            App::new(crate::app::workspace::Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, None).unwrap();
        execute_command(&mut app, Command::InsertChar('!'));
        execute_command(&mut app, Command::CloseTab);
        execute_command(&mut app, Command::DialogActivate);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "!hello\n");
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn a_tab_whose_save_fails_stays_open_rather_than_losing_the_edit() {
        let mut app = app();
        execute_command(&mut app, Command::SelectTab(1));
        execute_command(&mut app, Command::CloseTab);
        // The fixture documents live under a path nothing can be written to.
        execute_command(&mut app, Command::DialogActivate);

        assert_eq!(app.tabs.len(), 3, "the tab is still open");
        assert!(app.tabs[1].document.is_dirty());
        let notification = app.notifications.current().unwrap();
        assert_eq!(
            notification.kind,
            crate::app::notifications::NotificationKind::Error
        );
        assert!(notification.message.starts_with("Failed to save:"));
    }

    #[test]
    fn a_tab_undone_back_to_what_is_on_disk_closes_without_a_prompt() {
        let mut app = app();
        execute_command(&mut app, Command::SelectTab(1));
        // The fixture's second tab is dirty from an edit and its own undo.
        execute_command(&mut app, Command::Undo);
        execute_command(&mut app, Command::Undo);
        assert!(!app.tabs[1].document.is_dirty(), "ADR-014");
        execute_command(&mut app, Command::CloseTab);
        assert!(app.dialog.is_none());
        assert_eq!(app.tabs.len(), 2);
    }

    #[test]
    fn the_file_close_tab_menu_item_closes_the_active_tab() {
        let mut app = App::fixture_with_tabs(3);
        let close = MENUS[0]
            .items
            .iter()
            .position(|i| i.command == Command::CloseTab)
            .expect("a Close Tab item");
        execute_command(&mut app, Command::MenuOpen(0));
        execute_command(&mut app, Command::MenuActivateItem(close));
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.focus, FocusTarget::Editor);
    }

    #[test]
    fn a_prompt_opened_from_the_menu_returns_focus_to_the_pane_underneath_it() {
        let mut app = app();
        execute_command(&mut app, Command::SelectTab(1));
        let close = MENUS[0]
            .items
            .iter()
            .position(|i| i.command == Command::CloseTab)
            .unwrap();
        execute_command(&mut app, Command::MenuOpen(0));
        execute_command(&mut app, Command::MenuActivateItem(close));
        assert_eq!(app.focus, FocusTarget::Dialog);
        assert_eq!(app.menu.open, None, "the menu is not left open behind it");
        execute_command(&mut app, Command::DialogCancel);
        assert_eq!(app.focus, FocusTarget::Editor);
    }

    #[test]
    fn closing_a_tab_scrolls_whichever_one_comes_forward_to_its_cursor() {
        let mut app = App::fixture_with_tabs(2);
        app.editor_view = EditorView {
            width: 40,
            height: 3,
        };
        app.tabs[1] = crate::app::Tab::scratch("long.txt", &"line\n".repeat(40));
        app.tabs[1].document.goto_line(30);
        execute_command(&mut app, Command::CloseTabAt(0));
        assert_eq!(app.active_tab, Some(0));
        assert!(
            app.active().unwrap().viewport.top_line > 0,
            "the cursor of the tab that came forward is on screen"
        );
    }

    #[test]
    fn saving_a_document_with_no_file_behind_it_reports_the_failure() {
        let mut app = app();
        execute_command(&mut app, Command::Save);
        let notification = app.notifications.current().unwrap();
        assert_eq!(
            notification.kind,
            crate::app::notifications::NotificationKind::Error
        );
        assert!(notification.message.starts_with("Failed to save:"));
    }

    #[test]
    fn saving_writes_the_file_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        std::fs::write(&path, "hello\n").unwrap();

        let mut app =
            App::new(crate::app::workspace::Workspace::from_arg(Some(dir.path())).unwrap());
        app.open_path(&path, None).unwrap();
        execute_command(&mut app, Command::InsertChar('!'));
        execute_command(&mut app, Command::Save);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "!hello\n");
        assert!(!app.active().unwrap().document.is_dirty());
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Saved hello.txt"
        );
    }

    // --- Phase 6: the explorer and the file operations ---------------------

    /// The workspace root as `App` sees it: `Workspace` canonicalises, and on
    /// macOS a temporary directory is a symlink into `/private`.
    fn root_of(dir: &tempfile::TempDir) -> PathBuf {
        std::fs::canonicalize(dir.path()).unwrap()
    }

    /// A workspace with something in every shape the explorer has to handle.
    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "# project\n").unwrap();
        dir
    }

    /// The rows as `name` strings, which is what the assertions below read.
    fn rows(app: &App) -> Vec<String> {
        app.sidebar
            .rows()
            .iter()
            .map(|row| row.name.clone())
            .collect()
    }

    fn select(app: &mut App, name: &str) {
        let index = rows(app)
            .iter()
            .position(|row| row == name)
            .unwrap_or_else(|| panic!("{name} is not in the tree: {:?}", rows(app)));
        execute_command(app, Command::SelectSidebarRow(index));
    }

    #[test]
    fn activating_a_directory_opens_and_closes_it_and_a_file_opens_a_tab() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        assert_eq!(rows(&app), vec!["src", "README.md"]);

        select(&mut app, "src");
        execute_command(&mut app, Command::ExplorerActivate);
        assert_eq!(rows(&app), vec!["src", "main.rs", "README.md"]);
        assert!(app.tabs.is_empty(), "a directory is not a document");

        execute_command(&mut app, Command::ExplorerActivate);
        assert_eq!(rows(&app), vec!["src", "README.md"], "and folds again");

        select(&mut app, "README.md");
        execute_command(&mut app, Command::ExplorerActivate);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active().unwrap().document.title(), "README.md");
        assert_eq!(app.focus, FocusTarget::Editor, "the file is now the work");
    }

    #[test]
    fn a_click_selects_and_acts_in_one_go() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        app.focus = FocusTarget::Editor;

        execute_command(&mut app, Command::ExplorerActivateRow(1));
        assert_eq!(app.sidebar.selected, 1);
        assert_eq!(app.active().unwrap().document.title(), "README.md");

        // Below the last row there is nothing to act on, but the pane still
        // takes focus.
        execute_command(&mut app, Command::ExplorerActivateRow(99));
        assert_eq!(app.focus, FocusTarget::Explorer);
        assert_eq!(app.sidebar.selected, 1);
    }

    #[test]
    fn right_opens_a_directory_and_then_steps_into_it_and_left_comes_back_out() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "src");

        execute_command(&mut app, Command::ExplorerExpand);
        assert!(app.sidebar.selected_row().unwrap().expanded);
        assert_eq!(
            app.sidebar.selected, 0,
            "opening does not move the selection"
        );

        execute_command(&mut app, Command::ExplorerExpand);
        assert_eq!(app.sidebar.selected_row().unwrap().name, "main.rs");

        // Left on a file steps out to the directory holding it.
        execute_command(&mut app, Command::ExplorerCollapse);
        assert_eq!(app.sidebar.selected_row().unwrap().name, "src");
        // And again on an open directory closes it.
        execute_command(&mut app, Command::ExplorerCollapse);
        assert_eq!(rows(&app), vec!["src", "README.md"]);
    }

    #[test]
    fn the_view_scrolls_to_keep_the_selection_on_screen() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            std::fs::write(dir.path().join(format!("f{i:02}.txt")), "").unwrap();
        }
        let mut app = App::fixture_in(dir.path());
        app.explorer_rows = 5;

        execute_command(&mut app, Command::MoveSidebarSelection(7));
        assert_eq!(app.sidebar.selected, 7);
        assert_eq!(
            app.sidebar.scroll, 3,
            "the selected row is the last visible"
        );

        execute_command(&mut app, Command::MoveSidebarSelection(-7));
        assert_eq!(app.sidebar.scroll, 0, "and it comes back with it");
    }

    #[test]
    fn a_new_file_is_created_selected_and_opened() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "src");
        execute_command(&mut app, Command::ExplorerExpand);

        execute_command(&mut app, Command::NewFilePrompt);
        assert!(app.dialog_wants_text(), "the prompt asks for a name");
        for ch in "notes.md".chars() {
            execute_command(&mut app, Command::DialogInputChar(ch));
        }
        execute_command(&mut app, Command::DialogActivate);

        let created = dir.path().join("src/notes.md");
        assert!(created.exists(), "and it is on disk, empty");
        assert_eq!(rows(&app), vec!["src", "main.rs", "notes.md", "README.md"]);
        assert_eq!(app.sidebar.selected_row().unwrap().name, "notes.md");
        assert_eq!(app.active().unwrap().document.title(), "notes.md");
        assert!(app.dialog.is_none());
    }

    #[test]
    fn a_new_file_with_nothing_selected_lands_in_the_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::fixture_in(dir.path());
        assert!(app.sidebar.selected_row().is_none(), "an empty tree");

        execute_command(&mut app, Command::NewFilePrompt);
        execute_command(&mut app, Command::DialogInputText("first.txt".into()));
        execute_command(&mut app, Command::DialogActivate);
        assert!(dir.path().join("first.txt").exists());
    }

    #[test]
    fn a_new_entry_goes_inside_the_selected_directory() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "src");

        execute_command(&mut app, Command::NewFilePrompt);
        assert_eq!(app.dialog.as_ref().unwrap().prompt(), "Create in src");
        execute_command(&mut app, Command::DialogInputText("inside.rs".into()));
        execute_command(&mut app, Command::DialogActivate);
        assert!(dir.path().join("src/inside.rs").exists());
    }

    #[test]
    fn a_new_folder_is_created_and_opens_no_tab() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "README.md");

        execute_command(&mut app, Command::NewDirectoryPrompt);
        execute_command(&mut app, Command::DialogInputText("docs".into()));
        execute_command(&mut app, Command::DialogActivate);

        assert!(dir.path().join("docs").is_dir());
        assert!(app.tabs.is_empty());
        assert_eq!(rows(&app), vec!["docs", "src", "README.md"]);
    }

    #[test]
    fn a_name_that_cannot_be_used_is_reported_and_nothing_happens() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "README.md");

        execute_command(&mut app, Command::NewFilePrompt);
        execute_command(&mut app, Command::DialogInputText("README.md".into()));
        execute_command(&mut app, Command::DialogActivate);
        assert_eq!(
            app.notifications.current().map(|n| n.kind),
            Some(crate::app::notifications::NotificationKind::Error)
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "# project\n",
            "the file that was there is untouched"
        );

        // A name is one component, never a path out of the directory.
        execute_command(&mut app, Command::NewFilePrompt);
        execute_command(&mut app, Command::DialogInputText("../escape.txt".into()));
        execute_command(&mut app, Command::DialogActivate);
        assert!(!dir.path().parent().unwrap().join("escape.txt").exists());
    }

    #[test]
    fn renaming_moves_the_file_and_the_tab_that_is_showing_it() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "README.md");
        execute_command(&mut app, Command::ExplorerActivate);
        execute_command(&mut app, Command::FocusPane(FocusTarget::Explorer));

        execute_command(&mut app, Command::RenamePrompt);
        let field = app.dialog.as_ref().unwrap().field().unwrap();
        assert_eq!(field.value, "README.md", "pre-filled with the current name");
        execute_command(&mut app, Command::DialogInputBackspace);
        execute_command(&mut app, Command::DialogInputBackspace);
        execute_command(&mut app, Command::DialogInputText("st".into()));
        execute_command(&mut app, Command::DialogActivate);

        assert!(dir.path().join("README.st").exists());
        assert!(!dir.path().join("README.md").exists());
        assert_eq!(
            app.tabs[0].document.path().unwrap(),
            root_of(&dir).join("README.st"),
            "the open tab points at the file, not at the name it used to have"
        );
    }

    #[test]
    fn renaming_a_directory_follows_the_files_open_from_inside_it() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        app.open_path(&dir.path().join("src/main.rs"), None)
            .unwrap();

        execute_command(&mut app, Command::FocusPane(FocusTarget::Explorer));
        select(&mut app, "src");
        execute_command(&mut app, Command::RenamePrompt);
        // The field is pre-filled with `src`, so the old name is cleared first.
        for _ in 0..3 {
            execute_command(&mut app, Command::DialogInputBackspace);
        }
        execute_command(&mut app, Command::DialogInputText("lib".into()));
        execute_command(&mut app, Command::DialogActivate);

        assert_eq!(
            app.tabs[0].document.path().unwrap(),
            root_of(&dir).join("lib/main.rs")
        );
    }

    #[test]
    fn deleting_asks_first_and_the_answer_decides() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "README.md");

        execute_command(&mut app, Command::DeletePrompt);
        let dialog = app.dialog.as_ref().expect("a prompt");
        assert_eq!(dialog.prompt(), "Delete README.md?");
        assert_eq!(dialog.buttons[dialog.selected].label, "Cancel");

        // Enter takes the default, which is to do nothing.
        execute_command(&mut app, Command::DialogActivate);
        assert!(dir.path().join("README.md").exists());

        execute_command(&mut app, Command::DeletePrompt);
        execute_command(&mut app, Command::DialogMove(1));
        execute_command(&mut app, Command::DialogActivate);
        assert!(!dir.path().join("README.md").exists());
        assert_eq!(rows(&app), vec!["src"]);
    }

    #[test]
    fn deleting_a_directory_says_that_it_takes_the_contents_with_it() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "src");

        execute_command(&mut app, Command::DeletePrompt);
        assert_eq!(
            app.dialog.as_ref().unwrap().prompt(),
            "Delete src and everything in it?"
        );
        execute_command(&mut app, Command::DialogMove(1));
        execute_command(&mut app, Command::DialogActivate);
        assert!(!dir.path().join("src").exists());
        assert_eq!(rows(&app), vec!["README.md"]);
    }

    #[test]
    fn rename_and_delete_need_something_to_act_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::fixture_in(dir.path());
        for command in [Command::RenamePrompt, Command::DeletePrompt] {
            execute_command(&mut app, command);
            assert!(app.dialog.is_none(), "nothing is selected");
            assert_eq!(
                app.notifications.current().map(|n| n.kind),
                Some(crate::app::notifications::NotificationKind::Warning)
            );
        }
    }

    #[test]
    fn a_cancelled_prompt_creates_nothing_and_gives_focus_back() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::NewFilePrompt);
        execute_command(&mut app, Command::DialogInputText("ghost.txt".into()));
        execute_command(&mut app, Command::DialogCancel);

        assert!(!dir.path().join("ghost.txt").exists());
        assert!(app.dialog.is_none());
        assert_eq!(app.focus, FocusTarget::Explorer);
    }

    #[test]
    fn refreshing_picks_up_what_changed_outside_the_editor() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        std::fs::write(dir.path().join("added.txt"), "").unwrap();
        assert_eq!(rows(&app), vec!["src", "README.md"]);

        execute_command(&mut app, Command::ExplorerRefresh);
        assert_eq!(rows(&app), vec!["src", "added.txt", "README.md"]);
    }

    #[test]
    fn hidden_and_ignored_files_can_be_shown_and_hidden_again() {
        let dir = project();
        std::fs::write(dir.path().join(".gitignore"), "/build\n").unwrap();
        std::fs::create_dir(dir.path().join("build")).unwrap();
        let mut app = App::fixture_in(dir.path());
        assert_eq!(rows(&app), vec!["src", "README.md"]);

        execute_command(&mut app, Command::ToggleHiddenFiles);
        assert_eq!(
            rows(&app),
            vec!["build", "src", ".gitignore", "README.md"],
            "the ignored directory and the dotfile are both back"
        );

        execute_command(&mut app, Command::ToggleHiddenFiles);
        assert_eq!(rows(&app), vec!["src", "README.md"]);
    }
    // --- search ------------------------------------------------------------

    /// A one-tab app over `text`, which is what every search test wants: the
    /// fixture's three tabs would only make the assertions about tab indices.
    fn searchable(text: &str) -> App {
        let mut app = App::fixture();
        app.tabs = vec![crate::app::Tab::scratch("notes.txt", text)];
        app.active_tab = Some(0);
        app
    }

    /// Types a query into the open bar, one command per character, exactly as
    /// the keyboard would.
    fn type_query(app: &mut App, query: &str) {
        for ch in query.chars() {
            execute_command(app, Command::SearchInputChar(ch));
        }
        app.sync_search();
    }

    #[test]
    fn ctrl_f_opens_the_bar_and_takes_focus() {
        let mut app = searchable("one two three\n");
        execute_command(&mut app, Command::SearchOpen);
        assert!(app.search.open);
        assert!(!app.search.replacing, "Ctrl+F is find, not replace");
        assert_eq!(app.focus, FocusTarget::Search);
        assert_eq!(app.search.height(), 1);
    }

    #[test]
    fn opening_the_bar_seeds_the_query_with_the_selection() {
        let mut app = searchable("alpha beta\n");
        execute_command(&mut app, Command::SelectWordAt { line: 0, col: 0 });
        execute_command(&mut app, Command::SearchOpen);
        assert_eq!(app.search.query.value, "alpha");
        app.sync_search();
        assert_eq!(app.search.matches.len(), 1);
    }

    #[test]
    fn typing_a_query_finds_every_hit() {
        let mut app = searchable("foo\nbar foo\nFOO\n");
        execute_command(&mut app, Command::SearchOpen);
        type_query(&mut app, "foo");
        assert_eq!(app.search.matches.len(), 3, "case-insensitive by default");
        assert_eq!(app.search.count_label(), "1/3");

        execute_command(&mut app, Command::SearchToggleCase);
        app.sync_search();
        assert_eq!(app.search.matches.len(), 2);
    }

    #[test]
    fn find_next_selects_the_hits_in_turn_and_wraps() {
        let mut app = searchable("foo\nbar\nfoo\n");
        execute_command(&mut app, Command::SearchOpen);
        type_query(&mut app, "foo");

        execute_command(&mut app, Command::FindNext);
        assert_eq!(app.active().unwrap().document.cursor().line, 0);
        assert_eq!(
            app.active().unwrap().document.selected_text().as_deref(),
            Some("foo"),
            "the current hit is the selection"
        );

        execute_command(&mut app, Command::FindNext);
        assert_eq!(app.active().unwrap().document.cursor().line, 2);
        execute_command(&mut app, Command::FindNext);
        assert_eq!(app.active().unwrap().document.cursor().line, 0, "wrapped");
        execute_command(&mut app, Command::FindPrev);
        assert_eq!(app.active().unwrap().document.cursor().line, 2);
    }

    #[test]
    fn find_next_says_so_when_there_is_nothing_to_find() {
        let mut app = searchable("nothing here\n");
        execute_command(&mut app, Command::SearchOpen);
        type_query(&mut app, "zzz");
        execute_command(&mut app, Command::FindNext);
        assert!(app
            .notifications
            .current()
            .is_some_and(|n| n.message.contains("No matches")));
        assert!(app.active().unwrap().document.selection().is_none());
    }

    #[test]
    fn find_next_with_no_query_opens_the_bar_instead_of_doing_nothing() {
        let mut app = searchable("anything\n");
        execute_command(&mut app, Command::FindNext);
        assert!(app.search.open);
        assert_eq!(app.focus, FocusTarget::Search);
    }

    #[test]
    fn closing_the_bar_returns_focus_and_clears_the_highlighting() {
        let mut app = searchable("foo foo\n");
        execute_command(&mut app, Command::SearchOpen);
        type_query(&mut app, "foo");
        assert_eq!(app.search.matches.len(), 2);

        execute_command(&mut app, Command::SearchClose);
        assert!(!app.search.open);
        assert!(app.search.matches.is_empty());
        assert_eq!(app.focus, FocusTarget::Editor);
        assert_eq!(app.search.height(), 0);
    }

    #[test]
    fn an_edit_refinds_the_hits() {
        let mut app = searchable("foo\n");
        execute_command(&mut app, Command::SearchOpen);
        type_query(&mut app, "foo");
        assert_eq!(app.search.matches.len(), 1);

        // Type another `foo` into the document itself.
        execute_command(&mut app, Command::SearchClose);
        execute_command(&mut app, Command::MoveCursor(Motion::DocumentEnd));
        execute_command(&mut app, Command::InsertText("foo".into()));
        execute_command(&mut app, Command::SearchOpen);
        app.sync_search();
        assert_eq!(app.search.matches.len(), 2, "the new one is found too");
    }

    #[test]
    fn ctrl_h_opens_the_replacement_row() {
        let mut app = searchable("foo\n");
        execute_command(&mut app, Command::ReplaceOpen);
        assert!(app.search.replacing);
        assert_eq!(app.search.height(), 2);
        assert_eq!(app.search.field, SearchField::Query);
        execute_command(&mut app, Command::SearchToggleField);
        assert_eq!(app.search.field, SearchField::Replacement);
    }

    #[test]
    fn replace_rewrites_one_hit_and_moves_to_the_next() {
        let mut app = searchable("foo foo\n");
        execute_command(&mut app, Command::ReplaceOpen);
        type_query(&mut app, "foo");
        execute_command(&mut app, Command::SearchToggleField);
        type_query(&mut app, "bar");

        execute_command(&mut app, Command::ReplaceCurrent);
        assert_eq!(document_text(&app), "bar foo\n");
        assert_eq!(app.search.matches.len(), 1, "one hit left");

        execute_command(&mut app, Command::ReplaceCurrent);
        assert_eq!(document_text(&app), "bar bar\n");
        assert!(app.search.matches.is_empty());
    }

    #[test]
    fn replace_all_rewrites_everything_and_undoes_in_one_step() {
        let mut app = searchable("foo\nfoo bar foo\n");
        execute_command(&mut app, Command::ReplaceOpen);
        type_query(&mut app, "foo");
        execute_command(&mut app, Command::SearchToggleField);
        type_query(&mut app, "X");

        execute_command(&mut app, Command::ReplaceAll);
        assert_eq!(document_text(&app), "X\nX bar X\n");
        assert!(app
            .notifications
            .current()
            .is_some_and(|n| n.message == "Replaced 3 matches"));

        execute_command(&mut app, Command::Undo);
        assert_eq!(document_text(&app), "foo\nfoo bar foo\n", "one step");
    }

    #[test]
    fn replace_all_with_no_hits_says_so_and_changes_nothing() {
        let mut app = searchable("foo\n");
        execute_command(&mut app, Command::ReplaceOpen);
        type_query(&mut app, "zzz");
        execute_command(&mut app, Command::ReplaceAll);
        assert_eq!(document_text(&app), "foo\n");
        assert!(app
            .notifications
            .current()
            .is_some_and(|n| n.message.contains("No matches")));
    }

    #[test]
    fn replacing_from_the_menu_with_only_the_find_bar_open_asks_for_the_text_first() {
        let mut app = searchable("foo\n");
        execute_command(&mut app, Command::SearchOpen);
        type_query(&mut app, "foo");
        execute_command(&mut app, Command::ReplaceAll);
        assert_eq!(document_text(&app), "foo\n", "nothing was replaced yet");
        assert!(app.search.replacing, "the replacement row is now open");
        assert_eq!(app.search.field, SearchField::Replacement);
    }

    #[test]
    fn a_query_typed_into_a_field_never_contains_a_newline() {
        let mut app = searchable("a\n");
        execute_command(&mut app, Command::SearchOpen);
        execute_command(&mut app, Command::SearchInputText("two\nlines".into()));
        assert_eq!(app.search.query.value, "two lines");
    }

    /// The whole buffer of the active tab, for the replace assertions.
    fn document_text(app: &App) -> String {
        let document = &app.active().expect("a tab").document;
        (0..document.line_count())
            .map(|line| format!("{}\n", document.line(line)))
            .collect::<String>()
            .strip_suffix("\n")
            .unwrap_or_default()
            .to_string()
    }

    // --- Phase 9: Open, Save As, About ------------------------------------

    fn type_into_dialog(app: &mut App, text: &str) {
        // The field may be pre-filled, and typing appends at the caret, so the
        // tests that mean "instead of this" clear it first.
        execute_command(app, Command::DialogInputEnd);
        while app
            .dialog
            .as_ref()
            .and_then(|d| d.field())
            .is_some_and(|field| !field.value.is_empty())
        {
            execute_command(app, Command::DialogInputBackspace);
        }
        execute_command(app, Command::DialogInputText(text.into()));
        execute_command(app, Command::DialogActivate);
    }

    #[test]
    fn a_typed_path_opens_relative_to_the_explorer() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        select(&mut app, "src");
        execute_command(&mut app, Command::ExplorerExpand);

        execute_command(&mut app, Command::OpenPrompt);
        assert!(app.dialog_wants_text(), "the prompt asks for a path");
        assert_eq!(app.dialog.as_ref().unwrap().prompt(), "Open in src");
        type_into_dialog(&mut app, "main.rs");

        assert_eq!(app.active().unwrap().document.title(), "main.rs");
        assert_eq!(app.focus, FocusTarget::Editor);
        assert!(app.dialog.is_none());
    }

    #[test]
    fn an_absolute_path_ignores_the_directory_the_prompt_named() {
        let dir = project();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("elsewhere.txt");
        std::fs::write(&path, "out of tree\n").unwrap();

        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        type_into_dialog(&mut app, path.to_str().unwrap());

        assert_eq!(app.active().unwrap().document.title(), "elsewhere.txt");
    }

    #[test]
    fn opening_a_path_that_is_not_there_yet_starts_a_buffer_for_it() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        type_into_dialog(&mut app, "notes.md");

        assert_eq!(app.active().unwrap().document.title(), "notes.md");
        assert!(
            !dir.path().join("notes.md").exists(),
            "the file appears when it is saved, as it does from the command line"
        );
    }

    #[test]
    fn an_empty_open_answer_opens_nothing() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        execute_command(&mut app, Command::DialogActivate);

        assert!(app.tabs.is_empty(), "Enter on an empty field dismisses");
        assert_eq!(
            app.notifications.current().unwrap().message,
            "No path given"
        );
    }

    #[test]
    fn opening_a_directory_says_so_rather_than_opening_a_tab() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        // A file is selected, so the prompt is rooted at the workspace and
        // `src` below is the directory next to it.
        select(&mut app, "README.md");
        execute_command(&mut app, Command::OpenPrompt);
        type_into_dialog(&mut app, "src");

        assert!(app.tabs.is_empty());
        assert_eq!(
            app.notifications.current().unwrap().message,
            "src is a directory"
        );
    }

    #[test]
    fn save_as_writes_the_new_path_and_goes_on_editing_it_there() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        app.open_path(&dir.path().join("src/main.rs"), None)
            .unwrap();
        execute_command(&mut app, Command::InsertChar('x'));

        execute_command(&mut app, Command::SaveAsPrompt);
        let dialog = app.dialog.as_ref().expect("a prompt");
        assert_eq!(dialog.prompt(), "Save in src");
        assert_eq!(
            dialog.field().unwrap().value,
            "main.rs",
            "pre-filled, so confirming it unchanged is a plain save"
        );

        type_into_dialog(&mut app, "copy.rs");

        let written = dir.path().join("src/copy.rs");
        assert!(written.exists(), "the new path was written");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/main.rs")).unwrap(),
            "fn main() {}\n",
            "and the old one was left as it was"
        );
        assert_eq!(app.tabs.len(), 1, "the same tab, under another name");
        assert_eq!(app.active().unwrap().document.title(), "copy.rs");
        assert!(!app.active().unwrap().document.is_dirty());
        assert!(
            rows(&app).contains(&"copy.rs".to_string()),
            "and the tree shows it: {:?}",
            rows(&app)
        );
    }

    #[test]
    fn the_history_survives_save_as() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        app.open_path(&dir.path().join("src/main.rs"), None)
            .unwrap();
        execute_command(&mut app, Command::InsertChar('/'));

        execute_command(&mut app, Command::SaveAsPrompt);
        type_into_dialog(&mut app, "copy.rs");
        app.focus = FocusTarget::Editor;
        execute_command(&mut app, Command::Undo);

        assert_eq!(app.active().unwrap().document.line(0), "fn main() {}");
        assert!(
            app.active().unwrap().document.is_dirty(),
            "undone back past the save point, under the new name"
        );
    }

    #[test]
    fn save_as_onto_a_path_another_tab_holds_is_refused() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        app.open_path(&dir.path().join("README.md"), None).unwrap();
        app.open_path(&dir.path().join("src/main.rs"), None)
            .unwrap();

        execute_command(&mut app, Command::SaveAsPrompt);
        type_into_dialog(&mut app, "../README.md");

        assert_eq!(
            app.notifications.current().unwrap().message,
            "README.md is already open in another tab"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "# project\n",
            "and nothing was written over it"
        );
        assert_eq!(app.active().unwrap().document.title(), "main.rs");
    }

    #[test]
    fn save_as_with_no_tab_open_says_so() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::SaveAsPrompt);

        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().unwrap().message,
            "No file to save"
        );
    }

    #[test]
    fn about_is_a_dialog_that_only_dismisses() {
        let mut app = app();
        execute_command(&mut app, Command::ShowAbout);
        let dialog = app.dialog.as_ref().expect("a dialog");
        assert!(dialog.prompt().starts_with("FerroEdit "));
        assert_eq!(dialog.buttons.len(), 1);
        assert_eq!(dialog.command_at(0), None);

        execute_command(&mut app, Command::DialogActivate);
        assert!(app.dialog.is_none());
        assert_eq!(app.focus, FocusTarget::Editor);
    }

    #[test]
    fn every_menu_item_runs_without_panicking_from_every_focus() {
        // Phase 9's acceptance, as a test: the menu is wired to commands, and
        // no entry is a hole. The Git four and the help screen report
        // themselves; nothing else may.
        for (menu_index, menu) in MENUS.iter().enumerate() {
            for (item_index, item) in menu.items.iter().enumerate() {
                let dir = project();
                let mut app = App::fixture_in(dir.path());
                app.open_path(&dir.path().join("src/main.rs"), None)
                    .unwrap();
                execute_command(&mut app, Command::MenuOpen(menu_index));
                execute_command(&mut app, Command::MenuActivateItem(item_index));
                assert!(
                    app.menu.open.is_none(),
                    "{} > {} left the menu open",
                    menu.title,
                    item.label
                );
            }
        }
    }
}
