//! `execute_command` — the only place `App` is mutated.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::app::dialog::{DialogState, ListItem, MAX_LIST_ROWS};
use crate::app::diff::{DiffState, HORIZONTAL_STEP};
use crate::app::focus::FocusTarget;
use crate::app::git::NotStarted;
use crate::app::help::HelpState;
use crate::app::input_field::InputField;
use crate::app::search::SearchField;
use crate::app::tabs::{active_after_close, Stale, TabItem};
use crate::app::{App, LastClick, SidebarMode};
use crate::commands::{Command, FileOp, MenuEntry, MENUS};
use crate::config::ThemeKind;
use crate::editor::charset::Charset;
use crate::editor::coords::VisualCol;
use crate::editor::document::{DiskState, Document, LineEnding};
use crate::editor::wrap;
use crate::filesystem;
use crate::filesystem::watcher::FsChange;
use crate::git::diff::DiffSide;
use crate::git::models::{Change, Operation};
use crate::git::{GitJob, JobFailure, JobOutcome};
use crate::syntax::highlighter;

pub fn execute_command(app: &mut App, command: Command) {
    log::debug!("command {command:?} (focus {:?})", app.focus);
    match command {
        Command::Quit => quit(app),
        // Each answer closes its tab and asks `quit` again, which either finds
        // the next dirty tab or ends the run (ADR-047). A save that failed
        // stops there: the file is still only in the buffer, and quitting past
        // it is the one outcome that cannot be taken back.
        Command::SaveAndQuit(index) => {
            if save_tab(app, index) {
                remove_tab(app, index);
                quit(app);
            }
        }
        Command::DiscardAndQuit(index) => {
            remove_tab(app, index);
            quit(app);
        }

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
        Command::ExternalChange(change) => external_change(app, change),
        Command::GitOpenSelected => git_open_selected(app),
        Command::GitDiffRow(row) => git_diff_row(app, row),
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
        Command::GitCancel => cancel_git_jobs(app),
        Command::GitPush => start_git_job(app, GitJob::Push),
        Command::GitBranchPrompt => prompt_branch(app, false),
        Command::GitMergePrompt => prompt_branch(app, true),
        Command::GitSwitchBranch(branch) => start_git_job(app, GitJob::Switch(branch)),
        Command::GitMerge(branch) => start_git_job(app, GitJob::Merge(branch)),
        Command::GitNewBranchPrompt => prompt_new_branch(app),
        // Paired with the typed name by the dialog, exactly as `SubmitInput`
        // and `SubmitCommit` are; see `activate_dialog_button`.
        Command::SubmitBranch => log::warn!("a branch was submitted with no dialog open"),
        Command::GitCreateBranch(name) => git_create_branch(app, &name),
        Command::GitStageResolved => git_stage_conflicted(app),
        Command::GitDiff => open_diff(app),
        Command::GitDiffToggleSide => toggle_diff_side(app),
        Command::DiffRefresh => reread_diff(app, false),
        Command::DiffClose => close_diff(app),
        Command::DiffScroll(delta) => scroll_diff(app, delta as isize),
        Command::DiffScrollPage(delta) => {
            let page = (app.diff_rows as isize).max(1);
            scroll_diff(app, delta as isize * page);
        }
        Command::DiffScrollHorizontal(delta) => {
            let step = delta as isize * HORIZONTAL_STEP as isize;
            if let Some(viewer) = app.diff_mut() {
                viewer.scroll_h(step);
            }
        }
        Command::DiffHome => {
            if let Some(viewer) = app.diff_mut() {
                viewer.home();
            }
        }
        Command::DiffEnd => {
            let height = app.diff_rows as usize;
            if let Some(viewer) = app.diff_mut() {
                viewer.end(height);
            }
        }
        Command::GitJobFinished(outcome) => finish_git_job(app, &outcome),
        Command::ToggleHiddenFiles => toggle_hidden_files(app),

        Command::NewFilePrompt => prompt_new(app, false),
        Command::NewDirectoryPrompt => prompt_new(app, true),
        Command::RenamePrompt => prompt_rename(app),
        Command::DeletePrompt => prompt_delete(app),
        Command::OpenPrompt => prompt_open(app),
        // Both reach the dispatcher only through `activate_dialog_button`,
        // which runs them while the browser is still open — they read it.
        Command::BrowserOpen => browser_open(app),
        Command::BrowserOpenFolder => browser_open_folder(app),
        Command::OpenWorkspace(path) => open_workspace(app, &path),
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
            None => app.notifications.warning("No tab to close"),
        },
        Command::CloseTabAt(index) => close_tab(app, index),
        Command::CloseTabDiscarding(index) => remove_tab(app, index),
        Command::SaveAndCloseTab(index) => {
            if save_tab(app, index) {
                remove_tab(app, index);
            }
        }

        Command::Reload => reload_active(app),
        Command::ReloadTab(index) => {
            reload_tab(app, index, false);
        }
        Command::KeepBuffer(index) => keep_buffer(app, index),

        Command::ScrollEditor(delta) => scroll_editor(app, delta),
        Command::ToggleWordWrap => toggle_word_wrap(app),
        Command::GotoLinePrompt => prompt_goto_line(app),
        // Paired with the typed number by the dialog, exactly as `SubmitInput`
        // and `SubmitCommit` are; see `activate_dialog_button`.
        Command::SubmitGotoLine => log::warn!("a line was submitted with no dialog open"),
        Command::GotoLine(typed) => goto_line(app, &typed),

        Command::LineEndingPrompt => prompt_line_ending(app),
        Command::ConvertLineEndingPrompt(ending) => confirm_line_ending(app, ending),
        Command::ConvertLineEnding(ending) => convert_line_ending(app, ending),
        Command::EncodingPrompt => prompt_encoding(app),
        Command::EncodingChoice(name) => confirm_encoding(app, &name),
        Command::ReopenWithEncoding(name) => reopen_with_encoding(app, &name),
        Command::ConvertEncoding(name) => convert_encoding(app, &name),
        Command::LanguagePrompt => prompt_language(app),
        Command::SetLanguage(name) => set_language(app, &name),
        Command::ScrollEditorHorizontal(delta) => scroll_editor_columns(app, delta),

        Command::MoveCursor(motion) => {
            laid_out(app, |document: &mut Document, layout| {
                document.move_cursor(motion, layout)
            });
        }
        Command::ExtendSelection(motion) => {
            laid_out(app, |document: &mut Document, layout| {
                document.extend_cursor(motion, layout)
            });
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

        Command::DialogListMove(delta) => {
            let height = list_height(app);
            if let Some(dialog) = app.dialog.as_mut() {
                dialog.step_list(delta, height);
            }
        }
        Command::DialogSelectItem(row) => {
            let height = list_height(app);
            if let Some(dialog) = app.dialog.as_mut() {
                dialog.select_visible_row(row, height);
            }
        }
        Command::SubmitListChoice => log::warn!("a list choice was made with no dialog open"),
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

        Command::ScrollTabs(delta) => {
            app.tab_scroll = shift(app.tab_scroll, delta, app.tabs.len().saturating_sub(1));
        }

        Command::Save => match app.active_tab {
            Some(index) => {
                save_tab(app, index);
            }
            None => app.notifications.warning("No file to save"),
        },

        Command::SetTheme(kind) => set_theme(app, kind),

        Command::ShowAbout => show_about(app),

        Command::ShowHelp => show_help(app),
        Command::HelpClose => close_help(app),
        Command::HelpScroll(delta) => scroll_help(app, delta as isize),
        Command::HelpScrollPage(delta) => {
            let page = (app.help_rows as isize).max(1);
            scroll_help(app, delta as isize * page);
        }
        Command::HelpHome => {
            if let Some(help) = app.help.as_mut() {
                help.home();
            }
        }
        Command::HelpEnd => {
            let (width, height) = help_view(app);
            if let Some(help) = app.help.as_mut() {
                help.end(width, height);
            }
        }

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
    }

    // The help screen is drawn over the whole body, so it cannot outlive its
    // own focus: a command that moved focus to another pane would otherwise
    // leave the user typing into a document they cannot see (ADR-038). The
    // menu and a dialog draw *over* it and are allowed to, so neither closes
    // it. A diff no longer needs the same rule — it is a tab of its own now,
    // and a tab does not stop existing because the sidebar took focus.
    let covered = matches!(
        app.focus,
        FocusTarget::Editor | FocusTarget::Explorer | FocusTarget::GitPanel | FocusTarget::Search
    );
    if covered {
        app.help = None;
    }

    // Which of the two pane focuses is right depends on what the tab strip is
    // showing, and every command that switches tabs would otherwise have to
    // say so itself (SPEC §26).
    app.normalize_focus();

    // A file that moved under a modified buffer has a question attached to it,
    // and the moment to ask it is not the moment the watcher reported it: the
    // tab it belongs to may not be the one on screen, and a dialog may already
    // be open. Asking here, once every command has run, is the same single
    // check the two covering panes are closed by — so the question follows the
    // tab, and arrives when there is a screen free to put it on (ADR-043).
    prompt_stale(app);
}

/// Switches the colour scheme and remembers it (SPEC §43).
///
/// The write happens here rather than at shutdown so the choice survives a
/// crash and a `kill`, and a failure to write is reported rather than
/// swallowed: a theme that silently forgets itself on the next start is worse
/// than one that says it could not be saved.
fn set_theme(app: &mut App, kind: ThemeKind) {
    if app.settings.theme == kind {
        return;
    }
    app.settings.theme = kind;
    app.notifications.info(format!("{} theme", kind.label()));
    save_settings(app, "Theme");
}

/// Writes the settings file, if this run is one that writes it at all.
///
/// `what` names the setting that has just changed, so a failure says which
/// choice will not survive the next start rather than reporting a file path
/// the user did not know existed.
fn save_settings(app: &mut App, what: &str) {
    if !app.persist_settings {
        return;
    }
    if let Err(err) = app.settings.save() {
        log::error!("could not save the settings: {err}");
        app.notifications
            .warning(format!("{what} not saved: {err}"));
    }
}

/// Opens the help screen on the key tables (SPEC §6).
///
/// It covers the whole body, diff tab included, and is closed by `Esc` rather
/// than by whatever takes focus next. Pressing `F1` again from inside is
/// `HelpClose`, so this only ever opens.
fn show_help(app: &mut App) {
    // A menu opened over the screen returns focus *to* the screen, which would
    // leave the second one closing onto nothing.
    let return_focus = match dialog_return_focus(app) {
        FocusTarget::Help => FocusTarget::Editor,
        other => other,
    };
    app.menu.open = None;
    app.help = Some(HelpState::new(return_focus));
    app.focus = FocusTarget::Help;
}

fn close_help(app: &mut App) {
    if let Some(help) = app.help.take() {
        app.focus = help.return_focus;
    }
}

fn scroll_help(app: &mut App, delta: isize) {
    let (width, height) = help_view(app);
    if let Some(help) = app.help.as_mut() {
        help.scroll_by(delta, width, height);
    }
}

/// The help pane's size in the last drawn frame, the way every other scrolling
/// pane reads its own (ADR-010).
fn help_view(app: &App) -> (usize, usize) {
    (app.help_cols as usize, app.help_rows as usize)
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
    let view = app.text_view();
    let Some(tab) = app.active_mut() else { return };
    operation(&mut tab.document);
    tab.follow_cursor(view);
}

/// The same for an operation that has to know the shape of the pane: the
/// motions, whose `Up` is a drawn row and not a line once wrapping is on
/// (ADR-057).
fn laid_out(app: &mut App, operation: impl FnOnce(&mut Document, wrap::Layout)) {
    let view = app.text_view();
    let Some(tab) = app.active_mut() else { return };
    let layout = tab.layout(view);
    operation(&mut tab.document, layout);
    tab.follow_cursor(view);
}

/// Moves the editor's window without moving the cursor — the wheel, and the
/// two keys that shift a long line sideways.
fn scroll_editor(app: &mut App, delta: i16) {
    let view = app.text_view();
    let Some(tab) = app.active_mut() else { return };
    let layout = tab.layout(view);
    tab.viewport.scroll_rows(delta, &tab.document, layout);
}

fn scroll_editor_columns(app: &mut App, delta: i16) {
    let view = app.text_view();
    let Some(tab) = app.active_mut() else { return };
    let layout = tab.layout(view);
    tab.viewport.scroll_columns(delta, &tab.document, layout);
}

/// Turns wrapping on or off, and remembers the answer (SPEC §58).
///
/// Every tab is corrected, not only the one in front: the setting is the
/// pane's and not the file's, so a tab switched to later must not still be
/// scrolled sideways in a pane that no longer scrolls that way.
fn toggle_word_wrap(app: &mut App) {
    app.settings.word_wrap = !app.settings.word_wrap;
    save_settings(app, "Word wrap");
    let view = app.text_view();
    for tab in app.editors_mut() {
        let layout = tab.layout(view);
        tab.viewport.clamp(&tab.document, layout);
    }
    if let Some(tab) = app.active_mut() {
        tab.follow_cursor(view);
    }
    let state = if app.settings.word_wrap { "on" } else { "off" };
    app.notifications.info(format!("Word wrap {state}"));
}

/// Jumps to a one-based line number typed into the Go to Line dialog.
///
/// A number past the end of the file lands on its last line rather than
/// refusing: "go to 9999" in a file of 300 lines means the end of it, and that
/// is where `Ctrl+End` would have gone anyway.
fn goto_line(app: &mut App, typed: &str) {
    let typed = typed.trim();
    if typed.is_empty() {
        return;
    }
    let Ok(line) = typed.parse::<usize>() else {
        app.notifications
            .warning(format!("Not a line number: {typed}"));
        return;
    };
    let view = app.text_view();
    let Some(tab) = app.active_mut() else {
        app.notifications.warning("No file open");
        return;
    };
    tab.document.goto_line(line);
    tab.follow_cursor(view);
}

/// Asks which line to go to, with the one the cursor is on already in the
/// field: the dialog opens on an answer that changes nothing, and typing over
/// it is one gesture.
fn prompt_goto_line(app: &mut App) {
    let Some(tab) = app.active() else {
        app.notifications.warning("No file open");
        return;
    };
    let current = tab.document.cursor().line + 1;
    let count = tab.document.line_count();
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::goto_line(current, count, return_focus));
}

// --- what the status bar says about a file (ADR-058) ------------------------

/// Asks what the active file's lines should be separated by.
fn prompt_line_ending(app: &mut App) {
    let Some(tab) = app.active() else {
        app.notifications.warning("No file open");
        return;
    };
    let current = tab.document.line_ending();
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::line_ending(current, return_focus));
}

/// Asks before the file is rewritten.
///
/// Choosing the ending the file already has is not a question — there is
/// nothing to rewrite — so it says so and stops rather than offering to write
/// the file for no reason.
fn confirm_line_ending(app: &mut App, ending: LineEnding) {
    let Some(tab) = app.active() else {
        app.notifications.warning("No file open");
        return;
    };
    if tab.document.line_ending() == ending {
        app.notifications
            .info(format!("Already {} line endings", ending.label()));
        return;
    }
    let title = tab.document.title().to_string();
    let dirty = tab.document.is_dirty();
    let return_focus = dialog_return_focus(app);
    open_dialog(
        app,
        DialogState::convert_line_ending(&title, ending, dirty, return_focus),
    );
}

/// Sets the ending and writes the file, which is what the user just approved.
///
/// A buffer with no file behind it keeps the choice without a save: there is
/// nothing to rewrite yet, and the first Save As writes it with the ending that
/// was chosen here.
fn convert_line_ending(app: &mut App, ending: LineEnding) {
    let Some(index) = app.active_tab else {
        app.notifications.warning("No file open");
        return;
    };
    let Some(tab) = app.editor_at_mut(index) else {
        app.notifications.warning("No file open");
        return;
    };
    tab.document.set_line_ending(ending);
    let named = tab.document.path().is_some();
    if !named {
        app.notifications
            .info(format!("Line endings set to {}", ending.label()));
        return;
    }
    if save_tab(app, index) {
        // After `save_tab`'s own "Saved …", because this is the question the
        // user asked and the file name is not news.
        app.notifications
            .info(format!("Converted to {} line endings", ending.label()));
    }
}

/// Opens the encoding picker (SPEC §17, ADR-059).
fn prompt_encoding(app: &mut App) {
    let Some(tab) = app.active() else {
        app.notifications.warning("No file open");
        return;
    };
    let current = tab.document.charset();
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::encoding(current, return_focus));
}

/// Asks what the chosen encoding should mean — reopen, or convert.
///
/// Choosing the charset the file already has is not a question, exactly as it
/// is not one for the line endings: there is nothing to re-read and nothing to
/// rewrite.
fn confirm_encoding(app: &mut App, name: &str) {
    let Some(charset) = Charset::by_label(name) else {
        app.notifications
            .warning(format!("No encoding named {name}"));
        return;
    };
    let Some(tab) = app.active() else {
        app.notifications.warning("No file open");
        return;
    };
    if tab.document.charset() == charset {
        app.notifications.info(format!("Already {}", charset.label));
        return;
    }
    let title = tab.document.title().to_string();
    let named = tab.document.path().is_some();
    let dirty = tab.document.is_dirty();
    let return_focus = dialog_return_focus(app);
    open_dialog(
        app,
        DialogState::choose_encoding(&title, charset, named, dirty, return_focus),
    );
}

/// Re-reads the file with the chosen encoding — the answer for a file that came
/// out as mojibake.
///
/// The step is undoable, so a second wrong guess costs `Ctrl+Z` and not a
/// re-open; the viewport and the highlighting are brought back into step the
/// same way a reload brings them.
fn reopen_with_encoding(app: &mut App, name: &str) {
    let Some(charset) = Charset::by_label(name) else {
        app.notifications
            .warning(format!("No encoding named {name}"));
        return;
    };
    let view = app.text_view();
    let Some(tab) = app.active_mut() else {
        app.notifications.warning("No file open");
        return;
    };
    match tab.document.reopen_as(charset) {
        Ok(()) => {
            tab.mark_stale(None);
            tab.follow_cursor(view);
            app.notifications
                .info(format!("Reopened as {}", charset.label));
        }
        Err(err) => {
            log::error!("reopen failed: {err}");
            app.notifications.error(format!("Failed to reopen: {err}"));
        }
    }
}

/// Writes the buffer out in the chosen encoding, keeping the text.
///
/// A character the charset cannot hold makes the save fail with the file on
/// disk untouched (ADR-059), so the buffer is put back to the charset it had:
/// a tab left claiming an encoding it cannot be written in would fail again on
/// the next `Ctrl+S`, for a reason the user has since forgotten.
fn convert_encoding(app: &mut App, name: &str) {
    let Some(charset) = Charset::by_label(name) else {
        app.notifications
            .warning(format!("No encoding named {name}"));
        return;
    };
    let Some(index) = app.active_tab else {
        app.notifications.warning("No file open");
        return;
    };
    let Some(tab) = app.editor_at_mut(index) else {
        app.notifications.warning("No file open");
        return;
    };
    let previous = tab.document.charset();
    tab.document.set_charset(charset);
    if tab.document.path().is_none() {
        app.notifications
            .info(format!("Encoding set to {}", charset.label));
        return;
    }
    if save_tab(app, index) {
        app.notifications
            .info(format!("Converted to {}", charset.label));
    } else if let Some(tab) = app.editor_at_mut(index) {
        tab.document.set_charset(previous);
    }
}

/// Opens the syntax picker with every grammar in the set, the one in use
/// highlighted (SPEC §21).
fn prompt_language(app: &mut App) {
    let Some(tab) = app.active() else {
        app.notifications.warning("No file open");
        return;
    };
    let current = tab.highlights.language();
    let items: Vec<ListItem> = highlighter::names()
        .iter()
        .map(|name| ListItem {
            label: (*name).to_string(),
            command: Command::SetLanguage((*name).to_string()),
            current: *name == current,
        })
        .collect();
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::syntax_mode(items, return_focus));
}

/// Highlights the active tab with a grammar chosen by name.
///
/// Nothing on disk changes: this is what the file *looks* like, not what it is,
/// which is why it needs no confirmation where the line-ending conversion does.
fn set_language(app: &mut App, name: &str) {
    let Some(syntax) = highlighter::by_name(name) else {
        app.notifications
            .warning(format!("No grammar named {name}"));
        return;
    };
    let Some(tab) = app.active_mut() else {
        app.notifications.warning("No file open");
        return;
    };
    tab.highlights.set_syntax(syntax, &tab.document);
    app.notifications
        .info(format!("Highlighting as {}", syntax.name));
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
    let view = app.text_view();
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
    let view = app.text_view();
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
    let view = app.text_view();
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
    let Some(tab) = app.editor_at_mut(index) else {
        app.notifications.warning("No file to save");
        return false;
    };
    let outcome = tab.document.save();
    let title = tab.document.title().to_string();
    match outcome {
        Ok(()) => {
            // Whatever the file was before, it is the buffer now: a save
            // answers the "changed on disk" question by overwriting it.
            if let Some(tab) = app.editor_at_mut(index) {
                tab.mark_stale(None);
            }
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

// --- files that changed underneath (ADR-043) --------------------------------

/// Re-reads one tab from disk, reporting either outcome.
///
/// `quiet` is what the silent path uses: a clean buffer that is re-read because
/// its file changed has nothing to announce beyond the text itself, and the
/// status bar is where the answers to the user's *own* commands go. A reload
/// the user asked for says so.
///
/// Returns whether the buffer is now what is on disk.
fn reload_tab(app: &mut App, index: usize, quiet: bool) -> bool {
    let Some(tab) = app.editor_at_mut(index) else {
        app.notifications.warning("No file to reload");
        return false;
    };
    let outcome = tab.document.reload();
    let title = tab.document.title().to_string();
    match outcome {
        Ok(()) => {
            tab.mark_stale(None);
            // The pane was scrolled for a document that may have been longer,
            // and the caret has just been clamped into a different one.
            let view = app.text_view();
            if let Some(tab) = app.editor_at_mut(index) {
                tab.follow_cursor(view);
            }
            if !quiet {
                app.notifications.info(format!("Reloaded {title}"));
            }
            true
        }
        Err(err) => {
            log::error!("reload failed: {err}");
            // The buffer is untouched, so the tab is exactly as stale as it
            // was — but the user asked and deserves the reason.
            app.notifications.error(format!("Failed to reload: {err}"));
            false
        }
    }
}

/// The File menu's Reload: re-reads the active tab, asking first when there is
/// something in it that is not on disk.
fn reload_active(app: &mut App) {
    let Some(index) = app.active_tab else {
        app.notifications.warning("No file to reload");
        return;
    };
    let Some(tab) = app.editor_at(index) else {
        return;
    };
    if !tab.document.is_dirty() {
        reload_tab(app, index, false);
        return;
    }
    let gone = tab.document.disk_state() == DiskState::Gone;
    let title = tab.document.title().to_string();
    let return_focus = dialog_return_focus(app);
    open_dialog(
        app,
        DialogState::file_changed(index, &title, gone, return_focus),
    );
}

/// The "Keep Mine" answer: the buffer stands, and the question is not asked
/// again until the file moves once more.
fn keep_buffer(app: &mut App, index: usize) {
    if let Some(tab) = app.editor_at_mut(index) {
        tab.mark_stale(None);
    }
}

/// Looks at every open tab after something changed in the workspace (ADR-043).
///
/// A clean buffer is re-read where it stands: everything in it is also on disk,
/// so there is nothing a prompt could protect. A modified one is marked and
/// left alone — the buffer is the only copy of the user's work, and no
/// filesystem event is allowed to spend it.
fn check_open_files(app: &mut App) {
    let view = app.text_view();
    // Diff tabs are skipped: there is no buffer in one to lose, and the diff
    // itself is re-read by `reread_diff` on the same event.
    let indices: Vec<usize> = app.editors().map(|(index, _)| index).collect();
    for index in indices {
        let Some(tab) = app.editor_at(index) else {
            continue;
        };
        let state = tab.document.disk_state();
        let dirty = tab.document.is_dirty();
        if matches!(state, DiskState::Same | DiskState::Untracked) {
            if let Some(tab) = app.editor_at_mut(index) {
                tab.mark_stale(None);
            }
            continue;
        }
        if dirty || state == DiskState::Gone {
            let stale = match state {
                DiskState::Gone => Stale::Gone,
                _ => Stale::Changed,
            };
            let Some(tab) = app.editor_at_mut(index) else {
                continue;
            };
            let news = tab.mark_stale(Some(stale));
            // A file that vanished under a clean buffer has no question to ask
            // — nothing would be lost by keeping it and there is nothing to
            // reload from — so it is said once and marked, not prompted.
            if news && !dirty {
                tab.asked = true;
                let title = tab.document.title().to_string();
                app.notifications
                    .warning(format!("{title} is gone from disk"));
            }
            continue;
        }
        let Some(tab) = app.editor_at_mut(index) else {
            continue;
        };
        if tab.document.reload().is_err() {
            // Unreadable now — a partial write, or a file replaced by a
            // directory. It is not this tab's last word: the next event over
            // the same path tries again.
            tab.mark_stale(Some(Stale::Changed));
            continue;
        }
        tab.mark_stale(None);
        tab.follow_cursor(view);
        log::info!("reloaded {} after an external change", tab.document.title());
    }
}

/// Asks about the active tab's file, once, when there is a screen free for it.
///
/// Only the active tab: a dialog is modal, and one that spoke for a background
/// tab would be a question about a document the user cannot see. The others
/// keep their mark in the tab bar and are asked about when they come forward.
fn prompt_stale(app: &mut App) {
    if app.dialog.is_some() || app.should_quit {
        return;
    }
    let Some(index) = app.active_tab else { return };
    let Some(tab) = app.editor_at(index) else {
        return;
    };
    let (Some(stale), false) = (tab.stale, tab.asked) else {
        return;
    };
    let title = tab.document.title().to_string();
    let return_focus = dialog_return_focus(app);
    if let Some(tab) = app.editor_at_mut(index) {
        tab.asked = true;
    }
    open_dialog(
        app,
        DialogState::file_changed(index, &title, stale == Stale::Gone, return_focus),
    );
}

/// Quits, or asks first when something would be lost by it.
///
/// `is_dirty` is history-aware (ADR-014), so a file edited and then undone back
/// to what is on disk does not hold up the exit.
fn quit(app: &mut App) {
    let dirty = app.tabs.iter().filter(|t| t.is_dirty()).count();
    let Some(index) = app.tabs.iter().position(|t| t.is_dirty()) else {
        app.should_quit = true;
        return;
    };
    let title = app.tabs[index].title();
    let return_focus = dialog_return_focus(app);
    open_dialog(
        app,
        DialogState::unsaved_on_quit(index, &title, dirty, return_focus),
    );
}

/// Closes a tab, asking first when it has unsaved changes (SPEC §11).
fn close_tab(app: &mut App, index: usize) {
    let Some(tab) = app.tabs.get(index) else {
        return;
    };
    if !tab.is_dirty() {
        remove_tab(app, index);
        return;
    }
    let title = tab.title();
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
    app.notifications.info(format!("Closed {}", closed.title()));
    // The tab that came forward was last scrolled for whatever the pane size
    // was then, which need not be what it is now.
    let view = app.text_view();
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
        FocusTarget::Dialog | FocusTarget::Diff | FocusTarget::Help => FocusTarget::Editor,
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
/// Closing first is what keeps this from recursing: the modal state is gone
/// before any of the commands runs, so one that opens a dialog of its own —
/// the branch picker's New… button — replaces this one rather than nesting
/// inside it.
///
/// An input dialog's confirm button is the one command that is completed here
/// rather than carried whole: the operation was decided when the dialog opened
/// and the name only exists now, in a field that is about to be dropped.
fn activate_dialog_button(app: &mut App, index: usize) {
    let Some(dialog) = app.dialog.as_ref() else {
        return;
    };
    // The browser's two buttons are the exception to closing first: one of
    // them walks into a directory, which is the dialog *staying* open, and both
    // read a listing that is about to be dropped (ADR-051).
    if let Some(command @ (Command::BrowserOpen | Command::BrowserOpenFolder)) =
        dialog.command_at(index)
    {
        return execute_command(app, command);
    }
    let typed = || dialog.field().map(|f| f.value.clone()).unwrap_or_default();
    let command = match dialog.command_at(index) {
        Some(Command::SubmitInput(operation)) => Some(Command::ApplyFileOp(operation, typed())),
        Some(Command::SubmitCommit) => Some(Command::GitCommit(typed())),
        Some(Command::SubmitGotoLine) => Some(Command::GotoLine(typed())),
        Some(Command::SubmitBranch) => Some(Command::GitCreateBranch(typed())),
        // A list dialog's confirm button acts on the highlighted row, which is
        // the same pairing as the two above: the button was built before the
        // choice existed.
        Some(Command::SubmitListChoice) => dialog.selected_item().map(|item| item.command.clone()),
        other => other,
    };
    close_dialog(app);
    if let Some(command) = command {
        execute_command(app, command);
    }
}

/// Runs an edit on the open dialog's text field, if it has one.
/// The window the open dialog's list scrolls within: what the last frame drew,
/// or the body's own maximum before there has been one.
fn list_height(app: &App) -> usize {
    app.dialog
        .as_ref()
        .map_or(MAX_LIST_ROWS, |dialog| dialog.list_height(app.dialog_rows))
}

fn edit_field(app: &mut App, operation: impl FnOnce(&mut InputField)) {
    let height = list_height(app);
    let Some(dialog) = app.dialog.as_mut() else {
        return;
    };
    if let Some(field) = dialog.field_mut() {
        operation(field);
    }
    // In the browser — and in the syntax picker (ADR-058) — that field is a
    // filter, so editing it changes which rows exist and where the selection is
    // among them.
    dialog.refilter(height);
}

/// Undoes or redoes one step on the active tab.
///
/// An empty stack is not a failure — it is the beginning (or the end) of the
/// document's history, and saying so is friendlier than a key that appears to
/// have missed.
fn undo_redo(app: &mut App, undo: bool) {
    let view = app.text_view();
    let Some(tab) = app.active_mut() else { return };
    let moved = if undo {
        tab.document.undo()
    } else {
        tab.document.redo()
    };
    tab.follow_cursor(view);
    if !moved {
        let what = if undo { "undo" } else { "redo" };
        app.notifications.warning(format!("Nothing to {what}"));
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
        app.notifications.warning("Nothing selected");
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
            app.notifications.warning("The clipboard is empty");
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
        app.notifications.warning("The explorer is empty");
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
            app.notifications.error(format!("Failed to open: {err}"));
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

/// Opens the file browser at the workspace root (SPEC §6, ADR-051).
///
/// The root and not the directory the explorer's selection happens to be in:
/// the sidebar's own position is a place in a *tree*, which the browser is not
/// showing, so starting there means the dialog opens somewhere the user did not
/// choose and cannot see the way out of. The root is the one directory they
/// picked on purpose, and `..` is one keystroke from anywhere above it.
fn prompt_open(app: &mut App) {
    let base = app.workspace.root().to_path_buf();
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::browse(&base, return_focus));
}

/// The browser's Open button: walk into a directory, or open a file.
///
/// The filter field doubles as the path field the dialog used to be, and a
/// filter that names something real wins over the selection — a pasted path is
/// an answer, not a search. A filter that matches *nothing* is an answer too:
/// it is how a file that does not exist yet is started, which is what the old
/// dialog did and what the command line still does.
fn browser_open(app: &mut App) {
    let Some(browser) = app.dialog.as_mut().and_then(DialogState::browser_mut) else {
        log::warn!("the browser was used with no browser open");
        return;
    };
    let dir = browser.dir().to_path_buf();
    let typed = resolve_typed_path(&dir, &browser.filter.value);
    let target = match typed {
        Some(path) if path.exists() => path,
        // Nothing in the listing matched what was typed, so what was typed is
        // the answer rather than a way of narrowing it down.
        Some(path) if browser.len() <= 1 => path,
        _ => match browser.selected_entry() {
            Some(entry) => entry.path.clone(),
            None => return,
        },
    };
    if target.is_dir() {
        browser.open_dir(&target);
        return;
    }
    close_dialog(app);
    open_browsed_file(app, &target);
}

/// The browser's Open Folder button: the selected directory, or the one being
/// listed when a file is selected.
fn browser_open_folder(app: &mut App) {
    let Some(browser) = app.dialog.as_ref().and_then(DialogState::browser) else {
        log::warn!("the browser was used with no browser open");
        return;
    };
    let target = browser.folder_target();
    close_dialog(app);
    execute_command(app, Command::OpenWorkspace(target));
}

/// Opens a file chosen in the browser, and leaves the sidebar showing the
/// folder it is in (SPEC §6).
///
/// A file already inside the workspace only has to be revealed: re-rooting the
/// tree to its own directory would throw away the project the user is working
/// in to show them one folder of it. A file outside the workspace has no row to
/// reveal, so the workspace moves to its directory — which is the same rule
/// `ferroedit path/to/file` has followed since Phase 1 (SPEC §10).
fn open_browsed_file(app: &mut App, path: &Path) {
    let path = crate::app::absolute(path);
    if !path.starts_with(app.workspace.root()) {
        if let Some(parent) = path.parent() {
            app.open_workspace(parent);
        }
    }
    open_and_report(app, &path);
    select_path(app, Some(&path));
}

/// Makes a directory the workspace (ADR-051).
///
/// The sidebar switches to the explorer, because a folder that was just opened
/// and is not on screen is a command that appears to have done nothing.
fn open_workspace(app: &mut App, path: &Path) {
    if !path.is_dir() {
        app.notifications
            .warning(format!("{} is not a folder", display_name(path)));
        return;
    }
    app.open_workspace(path);
    app.sidebar.mode = SidebarMode::Explorer;
    // Through `focus_pane` rather than by assignment, so the diff viewer and
    // the help screen close the way they do for every other pane (ADR-037).
    focus_pane(app, FocusTarget::Explorer);
    app.notifications
        .info(format!("Opened {}", app.workspace.name()));
}

/// Asks where to write the active tab (SPEC §6).
///
/// The prompt starts in the file's own directory under its own name, so
/// confirming it unchanged is a plain save rather than a surprise.
fn prompt_save_as(app: &mut App) {
    let Some(index) = app.active_tab else {
        app.notifications.warning("No file to save");
        return;
    };
    let Some(document) = app.editor_at(index).map(|tab| &tab.document) else {
        app.notifications.warning("No file to save");
        return;
    };
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
            .warning("Nothing selected in the explorer");
        return;
    };
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::rename(&path, return_focus));
}

fn prompt_delete(app: &mut App) {
    let Some(row) = app.sidebar.selected_row() else {
        app.notifications
            .warning("Nothing selected in the explorer");
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
    // Save As is answered with a path rather than a name, and it reports
    // through the path that already says "Saved …", so it does not reach the
    // create/rename tail below.
    if let FileOp::SaveAs { index, base } = &operation {
        return save_tab_as(app, *index, base, name);
    }

    let outcome = match &operation {
        FileOp::CreateFile { parent } => filesystem::create_file(parent, name),
        FileOp::CreateDirectory { parent } => filesystem::create_directory(parent, name),
        FileOp::Rename { path } => filesystem::rename(path, name),
        // Handled above; the match stays exhaustive rather than defaulting.
        FileOp::SaveAs { .. } => return,
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
            .warning(format!("{} is a directory", display_name(&path)));
        return;
    }
    // Two tabs at one path would be two histories over one file, and the second
    // save would silently undo the first (SPEC §11).
    if let Some(other) = app.tabs.iter().position(|tab| tab.is_at(&path)) {
        if other != index {
            app.notifications.warning(format!(
                "{} is already open in another tab",
                display_name(&path)
            ));
            return;
        }
    }
    let Some(tab) = app.editor_at_mut(index) else {
        return;
    };
    tab.document.set_path(path.clone());
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
        app.notifications.error(format!("Failed to delete: {err}"));
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
    for tab in app.editors_mut() {
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
    // Every open diff is of a file that status just re-read, so they follow it
    // rather than keeping a diff that is no longer true.
    reread_diffs(app);
}

/// The Git menu's Refresh, and `F5` in the panel: looks for the repository
/// again as well as re-reading its status, so a `git init` in the terminal next
/// door does not need a restart to show up.
/// A change the editor did not make (ADR-040).
///
/// Silent on purpose: the panes are re-read and nothing is said. A refresh
/// nobody asked for that announced itself would put a notification on the
/// status bar every time a build touched a file, and the notification is where
/// the answers to the user's own commands go.
fn external_change(app: &mut App, change: FsChange) {
    if change.worktree {
        refresh_tree(app, None);
        // The panes were the visible half of this gap and the buffers were the
        // other one: an explorer that had caught up while the document it was
        // pointing at had not is what ADR-043 exists to close.
        check_open_files(app);
    }
    if change.repository {
        refresh_git(app);
    }
}

fn git_rescan(app: &mut App) {
    let root = app.workspace.root().to_path_buf();
    app.git.discover(&root);
    follow_git(app);
    app.notifications.info(app.git.summary());
}

/// Opens a branch picker: the one to switch to, or the one to merge (SPEC §33,
/// §35).
///
/// The list is read in the foreground, like the status and for the same reason:
/// `for-each-ref` is a local read that finishes in milliseconds, and a picker
/// that appeared empty and filled in later would be one whose selection moves
/// under the user (ADR-030).
fn prompt_branch(app: &mut App, merge: bool) {
    let Some(repo) = app.git.service() else {
        app.notifications.warning(app.git.summary());
        return;
    };
    let branches = match repo.branches() {
        Ok(branches) => branches,
        Err(err) => {
            log::error!("could not list branches: {err}");
            app.notifications
                .error(format!("Could not list branches: {}", err.reason()));
            return;
        }
    };
    // The merge picker leaves out the branch already checked out, so an empty
    // one means something different in each case.
    let choosable = branches.iter().filter(|b| merge != b.is_head).count();
    if choosable == 0 {
        app.notifications.warning(if merge {
            "No other branch to merge"
        } else {
            "No branches yet — commit something first"
        });
        return;
    }
    let return_focus = dialog_return_focus(app);
    let dialog = if merge {
        DialogState::merge_branch(&branches, return_focus)
    } else {
        DialogState::switch_branch(&branches, return_focus)
    };
    open_dialog(app, dialog);
}

/// Asks for the name of a branch to create at `HEAD`.
fn prompt_new_branch(app: &mut App) {
    if !app.git.is_repository() {
        app.notifications.warning(app.git.summary());
        return;
    }
    let head = app.git.status.head_label().to_string();
    let return_focus = dialog_return_focus(app);
    open_dialog(app, DialogState::new_branch(&head, return_focus));
}

fn git_create_branch(app: &mut App, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        app.notifications.warning("A branch needs a name");
        return;
    }
    start_git_job(app, GitJob::CreateBranch(name.to_string()));
}

/// Stages the selected file with its markers as they are — the confirm
/// dialog's second button.
fn git_stage_conflicted(app: &mut App) {
    let Some(entry) = app.git.selected_entry() else {
        app.notifications
            .warning("Nothing selected in the Git panel");
        return;
    };
    start_git_job(app, GitJob::Stage(vec![entry.path.clone()]));
}

/// How much of a conflicted file is scanned for markers. git writes the first
/// one where the first conflict is, which is not necessarily near the top, but
/// a megabyte is far past any hunk a person is reading.
const MARKER_SCAN_LIMIT: usize = 1 << 20;

/// Whether the file still holds a conflict marker git would have written.
///
/// Only the `<<<<<<< ` opener is looked for, at the start of a line: it is the
/// one of the three that cannot plausibly be the file's own content, and
/// demanding all three would miss a half-finished resolution. The read is
/// capped, because a conflicted file can be a large one and this happens on the
/// UI thread.
fn has_conflict_markers(app: &App, path: &Path) -> bool {
    const MARKER: &[u8] = b"<<<<<<< ";
    let Some(root) = app.git.root() else {
        return false;
    };
    let Ok(text) = std::fs::read(root.join(path)) else {
        // Unreadable, or a delete/modify conflict with nothing on disk. Neither
        // is a reason to block the staging; git will say so if it disagrees.
        return false;
    };
    text[..MARKER_SCAN_LIMIT.min(text.len())]
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(MARKER))
}

/// Opens the file the panel's selection is on.
///
/// The status lists repository-relative paths, so the root is what turns one
/// back into something to open — and the root is the repository's, not the
/// workspace's, which matters when the editor was opened in a subdirectory.
fn git_open_selected(app: &mut App) {
    let Some(entry) = app.git.selected_entry() else {
        app.notifications
            .warning("Nothing selected in the Git panel");
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
            app.notifications.error(format!("Failed to open: {err}"));
        }
    }
}

/// Stages or unstages the selected file (SPEC §31).
fn git_stage_selected(app: &mut App, stage: bool) {
    let Some(entry) = app.git.selected_entry() else {
        app.notifications
            .warning("Nothing selected in the Git panel");
        return;
    };
    // `git add` on a conflicted file is git's way of saying "I resolved this",
    // and now that a merge can be started from the editor it needs saying here
    // too (ADR-036). What it must not do is let `<<<<<<<` reach a commit
    // unremarked, so a file that still has markers in it asks first.
    if stage && entry.is_conflicted() {
        let path = entry.path.clone();
        if has_conflict_markers(app, &path) {
            let return_focus = dialog_return_focus(app);
            open_dialog(
                app,
                DialogState::confirm_conflict_markers(&path, return_focus),
            );
            return;
        }
        start_git_job(app, GitJob::Stage(vec![path]));
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
    // A merge that has been resolved still has to be committed even when the
    // resolution staged nothing new, so an unfinished merge is the second way
    // in. Only a merge: a stopped rebase or cherry-pick is finished by
    // `git <verb> --continue`, which reuses the message git already recorded,
    // and a commit written here would not be that (ADR-041).
    if app.git.status.conflicts() > 0 {
        app.notifications
            .warning("Resolve the conflicts before committing");
        return;
    }
    let operation = app.git.status.operation;
    let staged = app.git.staged_count();
    if staged == 0 && !operation.is_some_and(Operation::finished_by_commit) {
        app.notifications.warning(match operation {
            Some(operation) => format!(
                "Nothing staged — finish the {} with `{}`",
                operation.noun(),
                operation.continue_command()
            ),
            None => "Nothing staged to commit".to_string(),
        });
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
        // No repository is the state the user is in and nothing was attempted;
        // a missing worker is the editor failing at something it promised.
        Err(NotStarted::NoRepository(summary)) => app.notifications.warning(summary),
        Err(NotStarted::NoWorker(why)) => app.notifications.error(why),
    }
}

/// Stops what the worker is doing (ADR-044).
///
/// Says how many were asked about rather than waiting for them: the answers
/// arrive one at a time and each says its own name, but a key press that
/// produced no word at all would look like one that did not register.
fn cancel_git_jobs(app: &mut App) {
    match app.git.cancel() {
        0 => app.notifications.warning("Nothing to cancel"),
        1 => app.notifications.info("Cancelling…"),
        n => app
            .notifications
            .info(format!("Cancelling {n} operations…")),
    }
}

/// Reports a finished job and re-reads the status it changed.
fn finish_git_job(app: &mut App, outcome: &JobOutcome) {
    app.git.finish(outcome);
    let what = outcome.job.label();
    match &outcome.result {
        Ok(said) => app.notifications.info(said.clone()),
        // Not an error: nothing went wrong, the user asked (ADR-044). It is
        // still said, because a job that stops without a word is one the user
        // has to guess about.
        Err(JobFailure::Cancelled) => app.notifications.info(format!("{what} cancelled")),
        Err(JobFailure::Failed(why)) => {
            app.notifications.error(format!("{what} failed: {why}"));
        }
    }
    // Even a failure can have changed the repository — a pull that fetched and
    // then refused to fast-forward has moved the remote-tracking branch — so
    // the status is re-read either way.
    refresh_git(app);
}

// --- diff viewer -----------------------------------------------------------

/// Opens the read-only diff of one file (SPEC §36).
///
/// Which file is the one question worth answering carefully: while the git
/// panel has focus it is the row the selection is on, and anywhere else it is
/// the file being edited — the Git menu's Diff, pressed mid-edit, means "this
/// one" and not "whichever row the panel happens to be parked on".
fn open_diff(app: &mut App) {
    let Some(root) = app.git.root().map(Path::to_path_buf) else {
        app.notifications.warning(app.git.summary());
        return;
    };
    let Some(path) = diff_target(app, &root) else {
        return;
    };
    // An untracked file has no diff at all: git has nothing to compare it
    // with until it is staged, and an empty viewer would look like a bug.
    if app
        .git
        .entries()
        .iter()
        .any(|entry| entry.path == path && entry.worktree == Change::Untracked)
    {
        app.notifications.warning(format!(
            "{} is untracked — stage it to see a diff",
            path.display()
        ));
        return;
    }
    let side = side_for(app, &path);
    let Some(diff) = read_diff(app, &path, side) else {
        return;
    };
    show_diff(app, DiffState::new(&path, side, diff));
}

/// Puts a diff in front of the user, in a tab of its own (SPEC §36).
///
/// A file already open as a diff reuses its tab rather than opening a second
/// one, for the same reason a file already open in the editor does: two tabs
/// of the same thing are two places to keep in step, and the user asked to see
/// the diff, not to collect them. The side is part of what a tab *shows* and
/// not of which tab it is, so re-opening the staged side of a file whose
/// worktree diff is open turns that tab over rather than adding a third.
fn show_diff(app: &mut App, diff: DiffState) {
    let existing = app
        .tabs
        .iter()
        .position(|item| item.diff().is_some_and(|open| open.path == diff.path));
    let index = match existing {
        Some(index) => {
            app.tabs[index] = TabItem::Diff(diff);
            index
        }
        None => {
            app.tabs.push(TabItem::Diff(diff));
            app.tabs.len() - 1
        }
    };
    app.active_tab = Some(index);
    app.focus = FocusTarget::Diff;
}

/// Selects a row of the git panel and opens that file's diff — one click.
///
/// The panel takes focus first, which is both what a click on a pane does and
/// what makes `open_diff` read the row rather than the file being edited.
fn git_diff_row(app: &mut App, row: usize) {
    app.sidebar.mode = SidebarMode::Git;
    app.focus = FocusTarget::GitPanel;
    select_sidebar_row(app, row);
    open_diff(app);
    // Focus stays in the panel, unlike the `d` key's: the point of clicking
    // down a list of changed files is to read them one after another, and a
    // click that moved focus into the diff would cost a click back for every
    // file — and take `Space` away from the row the pointer is on.
    app.focus = FocusTarget::GitPanel;
}

/// The file a diff would be of, or `None` after saying why there is not one.
fn diff_target(app: &mut App, root: &Path) -> Option<PathBuf> {
    if app.focus != FocusTarget::GitPanel {
        if let Some(path) = active_repo_path(app, root) {
            return Some(path);
        }
    }
    match app.git.selected_entry() {
        Some(entry) => Some(entry.path.clone()),
        None => {
            app.notifications
                .warning("Nothing to diff — select a changed file");
            None
        }
    }
}

/// The active tab's path, relative to the repository root, when it has one and
/// it is inside the repository at all.
fn active_repo_path(app: &App, root: &Path) -> Option<PathBuf> {
    let path = app.active()?.document.path()?;
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}

/// Which side is worth showing first: what has not been staged when there is
/// any, and what has been when there is not.
///
/// It is the answer to "what did I just change?" in both cases, which is the
/// question a diff is opened to answer.
fn side_for(app: &App, path: &Path) -> DiffSide {
    match app.git.entries().iter().find(|entry| entry.path == path) {
        Some(entry) if entry.worktree.is_change() => DiffSide::Worktree,
        Some(entry) if entry.index.is_change() => DiffSide::Staged,
        _ => DiffSide::Worktree,
    }
}

/// Runs one `git diff`, reporting a failure and an answer with nothing in it.
///
/// In the foreground, like the status and the branch list and for the same
/// reason: it is a local read that finishes in milliseconds, and a viewer that
/// opened empty and filled in later would be one that scrolls under the
/// reader (ADR-030).
fn read_diff(app: &mut App, path: &Path, side: DiffSide) -> Option<crate::git::Diff> {
    let repo = app.git.service()?.clone();
    match repo.diff(path, side) {
        Ok(diff) if diff.is_empty() => {
            app.notifications
                .warning(format!("{} {}", side.nothing(), path.display()));
            None
        }
        Ok(diff) => Some(diff),
        Err(err) => {
            log::error!("could not diff {}: {err}", path.display());
            app.notifications
                .error(format!("Diff failed: {}", err.reason()));
            None
        }
    }
}

/// Shows the other side of the same file. The side on screen stays put when
/// the other one holds nothing, which is the common case for a file that is
/// only half staged.
fn toggle_diff_side(app: &mut App) {
    let Some(viewer) = app.diff() else {
        return;
    };
    let (path, side) = (viewer.path.clone(), viewer.side.other());
    let Some(diff) = read_diff(app, &path, side) else {
        return;
    };
    show_diff(app, DiffState::new(&path, side, diff));
}

/// Re-runs the diff in the tab in front, and says so when it has emptied.
///
/// `F5` in a diff tab. The quiet form that runs after every git operation is
/// `reread_diffs`, which does the same for every diff tab at once.
fn reread_diff(app: &mut App, quiet: bool) {
    let Some(index) = app.active_tab else { return };
    reread_diff_at(app, index, quiet);
}

/// Follows the repository in every open diff tab (SPEC §36).
///
/// Diffs are tabs now, so several can be open at once and the one in front is
/// not the only one that has gone stale: staging a file changes the answer for
/// its tab whether or not that tab is the one being read. A tab whose diff has
/// emptied — everything in it staged, or undone — closes rather than going on
/// showing a change that is no longer there.
///
/// Backwards, because closing a tab shifts every index after it.
fn reread_diffs(app: &mut App) {
    for index in (0..app.tabs.len()).rev() {
        if app.tabs[index].diff().is_some() {
            reread_diff_at(app, index, true);
        }
    }
}

/// Re-reads the diff in one tab. An empty answer closes that tab.
fn reread_diff_at(app: &mut App, index: usize, quiet: bool) {
    let Some(viewer) = app.tabs.get(index).and_then(TabItem::diff) else {
        return;
    };
    let (path, side) = (viewer.path.clone(), viewer.side);
    let Some(repo) = app.git.service().cloned() else {
        remove_tab(app, index);
        return;
    };
    match repo.diff(&path, side) {
        Ok(diff) if diff.is_empty() => {
            if !quiet {
                app.notifications
                    .warning(format!("{} {}", side.nothing(), path.display()));
            }
            remove_tab(app, index);
        }
        Ok(diff) => {
            let height = app.diff_rows as usize;
            if let Some(viewer) = app.tabs.get_mut(index).and_then(TabItem::diff_mut) {
                viewer.diff = diff;
                viewer.clamp(height);
            }
        }
        Err(err) => {
            // The tab keeps what it has: a diff that failed to re-read is
            // stale, and a tab that vanished would be worse than one that is.
            log::error!("could not re-read the diff of {}: {err}", path.display());
            if !quiet {
                app.notifications
                    .error(format!("Diff failed: {}", err.reason()));
            }
        }
    }
}

fn scroll_diff(app: &mut App, delta: isize) {
    let height = app.diff_rows as usize;
    if let Some(viewer) = app.diff_mut() {
        viewer.scroll_by(delta, height);
    }
}

/// `Esc` in a diff tab closes the tab, which is what `Ctrl+W` would do too.
fn close_diff(app: &mut App) {
    let Some(index) = app.active_tab else { return };
    if app.tabs[index].diff().is_some() {
        remove_tab(app, index);
    }
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

/// Moves the selection by `delta` rows, stepping over the rules.
///
/// A separator is a row of the popup — it takes a line and it is counted by
/// the geometry — but it is not somewhere the selection may rest, so the step
/// keeps going in the direction it was asked for until it lands on an entry.
/// The bound on the walk is the popup's own length: a menu of nothing but
/// rules cannot exist, and if one did this would still terminate.
fn step_menu_item(app: &mut App, delta: i16) {
    let Some(open) = app.menu.open else { return };
    let items = MENUS[open].items;
    let len = items.len() as i16;
    if len == 0 {
        return;
    }
    let step = if delta < 0 { -1 } else { 1 };
    let mut next = app.menu.item as i16;
    for _ in 0..len {
        next = (next + step).rem_euclid(len);
        if !items[next as usize].is_separator() {
            break;
        }
    }
    app.menu.item = next as usize;
}

fn activate_menu_item(app: &mut App) {
    let Some(open) = app.menu.open else { return };
    // A click on a rule resolves to nothing, which is the whole point of one.
    let Some(item) = MENUS[open]
        .items
        .get(app.menu.item)
        .and_then(MenuEntry::item)
    else {
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
    use crate::app::EditorView;
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

    /// A repository whose `other` branch conflicts with `main`.
    fn conflicting_repo() -> crate::git::testing::TestRepo {
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
        repo
    }

    /// ADR-036: staging a conflicted file is how git is told the conflict is
    /// resolved, and the editor now has a merge that needs saying so — but a
    /// file that still has `<<<<<<<` in it asks first.
    #[test]
    fn a_conflicted_file_with_markers_asks_before_it_is_staged() {
        let repo = conflicting_repo();
        let (mut app, rx) = app_over(&repo);
        execute_command(&mut app, Command::GitMerge("other".into()));
        settle(&mut app, &rx);
        assert_eq!(app.git.status.conflicts(), 1, "{:?}", app.git.entries());

        execute_command(&mut app, Command::GitToggleStage);
        assert_eq!(app.git.busy(), None, "nothing was submitted yet");
        let dialog = app.dialog.as_ref().expect("a confirmation");
        assert_eq!(dialog.prompt(), "c.txt still has conflict markers.");
        assert_eq!(dialog.buttons[dialog.selected].label, "Cancel");

        // Stage Anyway is the second button, and it does stage them.
        execute_command(&mut app, Command::DialogMove(1));
        execute_command(&mut app, Command::DialogActivate);
        settle(&mut app, &rx);
        assert_eq!(app.git.status.conflicts(), 0, "{:?}", app.git.entries());
    }

    #[test]
    fn a_conflicted_file_the_user_has_resolved_is_staged_without_a_question() {
        let repo = conflicting_repo();
        let (mut app, rx) = app_over(&repo);
        execute_command(&mut app, Command::GitMerge("other".into()));
        settle(&mut app, &rx);

        repo.write("c.txt", "resolved by hand\n");
        execute_command(&mut app, Command::GitToggleStage);
        assert!(app.dialog.is_none(), "no markers, no question");
        settle(&mut app, &rx);
        assert_eq!(app.git.status.conflicts(), 0);
        assert_eq!(
            app.git.status.operation,
            Some(Operation::Merge),
            "and the merge still wants a commit"
        );
    }

    /// SPEC §35: a merge that stops shows its conflicted files, and the panel
    /// says the merge is unfinished until it is committed.
    #[test]
    fn a_merge_that_conflicts_shows_the_files_and_says_the_merge_is_open() {
        let repo = conflicting_repo();
        let (mut app, rx) = app_over(&repo);

        execute_command(&mut app, Command::GitMerge("other".into()));
        assert_eq!(app.git.busy(), Some("Merging…"));
        settle(&mut app, &rx);

        assert_eq!(app.git.status.operation, Some(Operation::Merge));
        assert_eq!(app.git.entries().len(), 1);
        assert!(app.git.entries()[0].is_conflicted());
        assert_eq!(app.git.summary(), "main (merging) — 1 change, 1 conflict");
        let notification = app.notifications.current().unwrap();
        assert_eq!(
            notification.kind,
            crate::app::notifications::NotificationKind::Error
        );
        assert!(
            notification.message.starts_with("Merge failed: conflicts"),
            "{}",
            notification.message
        );
    }

    #[test]
    fn a_clean_merge_brings_the_other_branchs_files_in() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["checkout", "-q", "-b", "other"]);
        repo.write("b.txt", "b\n");
        repo.run(&["add", "."]);
        repo.commit("other");
        repo.run(&["checkout", "-q", "main"]);

        let (mut app, rx) = app_over(&repo);
        execute_command(&mut app, Command::GitMerge("other".into()));
        settle(&mut app, &rx);

        assert!(repo.path().join("b.txt").exists());
        assert!(app.git.status.is_clean());
        assert_eq!(app.git.status.operation, None);
    }

    /// Committing is refused while anything is still conflicted, and allowed
    /// once nothing is — even though a resolved merge may have nothing new
    /// staged of its own.
    #[test]
    fn a_merge_is_finished_by_committing_it() {
        let repo = conflicting_repo();
        let (mut app, rx) = app_over(&repo);
        execute_command(&mut app, Command::GitMerge("other".into()));
        settle(&mut app, &rx);

        execute_command(&mut app, Command::GitCommitPrompt);
        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Resolve the conflicts before committing")
        );

        repo.write("c.txt", "resolved\n");
        execute_command(&mut app, Command::GitToggleStage);
        settle(&mut app, &rx);

        execute_command(&mut app, Command::GitCommitPrompt);
        assert!(app.dialog.is_some(), "the merge can be committed now");
        for character in "merge other".chars() {
            execute_command(&mut app, Command::DialogInputChar(character));
        }
        execute_command(&mut app, Command::DialogActivate);
        settle(&mut app, &rx);

        assert_eq!(app.git.status.operation, None);
        assert!(app.git.status.is_clean());
        assert_eq!(
            repo.run(&["log", "-1", "--pretty=%s"]).trim(),
            "merge other"
        );
    }

    // --- branches (SPEC §33) ----------------------------------------------

    #[test]
    fn the_branch_picker_lists_the_branches_and_switching_moves_head() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["branch", "topic"]);
        let (mut app, rx) = app_over(&repo);

        execute_command(&mut app, Command::GitBranchPrompt);
        let dialog = app.dialog.as_ref().expect("a picker");
        let labels: Vec<&str> = dialog.rows().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["main", "topic"]);
        assert_eq!(app.focus, FocusTarget::Dialog);

        // Down, then Enter on the default Switch button.
        execute_command(&mut app, Command::DialogListMove(1));
        execute_command(&mut app, Command::DialogActivate);
        assert!(app.dialog.is_none());
        settle(&mut app, &rx);

        assert_eq!(app.git.branch_label(), "topic");
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Switched topic")
        );
        assert_eq!(app.focus, FocusTarget::GitPanel, "focus came back");
    }

    /// Enter without moving is a no-op, because the picker opens on the branch
    /// HEAD is already on.
    #[test]
    fn confirming_the_picker_unchanged_switches_to_the_branch_already_checked_out() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["branch", "topic"]);
        let (mut app, rx) = app_over(&repo);

        execute_command(&mut app, Command::GitBranchPrompt);
        execute_command(&mut app, Command::DialogActivate);
        settle(&mut app, &rx);
        assert_eq!(app.git.branch_label(), "main");
    }

    /// The picker's New… button opens the name prompt in place of the picker
    /// rather than on top of it.
    #[test]
    fn the_pickers_new_button_asks_for_a_name_and_creates_the_branch() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        let (mut app, rx) = app_over(&repo);

        execute_command(&mut app, Command::GitBranchPrompt);
        execute_command(&mut app, Command::DialogActivateButton(1));
        let dialog = app
            .dialog
            .as_ref()
            .expect("the name prompt replaced the picker");
        assert_eq!(dialog.title, "New Branch");
        assert_eq!(dialog.prompt(), "Branch from main");

        for character in "feature/x".chars() {
            execute_command(&mut app, Command::DialogInputChar(character));
        }
        execute_command(&mut app, Command::DialogActivate);
        settle(&mut app, &rx);

        assert_eq!(app.git.branch_label(), "feature/x");
    }

    #[test]
    fn a_branch_with_no_name_is_refused_before_git_sees_it() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        execute_command(&mut app, Command::GitCreateBranch("  ".into()));
        assert_eq!(app.git.busy(), None);
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("A branch needs a name")
        );
    }

    #[test]
    fn a_repository_with_one_branch_has_nothing_to_merge() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        execute_command(&mut app, Command::GitMergePrompt);
        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("No other branch to merge")
        );
    }

    #[test]
    fn a_repository_with_no_commits_has_no_branches_to_pick_from() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        let (mut app, _rx) = app_over(&repo);
        execute_command(&mut app, Command::GitBranchPrompt);
        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("No branches yet — commit something first")
        );
    }

    #[test]
    fn a_branch_action_outside_a_repository_says_so_rather_than_opening_a_picker() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::GitRefresh);

        for command in [Command::GitBranchPrompt, Command::GitNewBranchPrompt] {
            execute_command(&mut app, command);
            assert!(app.dialog.is_none());
            assert_eq!(
                app.notifications.current().map(|n| n.message.as_str()),
                Some("Not a Git repository")
            );
        }
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
        app.tabs.retain(|tab| !tab.is_dirty());
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
        assert_eq!(dialog.prompt(), "editor.rs has unsaved changes.");
        assert_eq!(app.focus, FocusTarget::Dialog);

        // Cancel is the third button now that Save is the first one.
        execute_command(&mut app, Command::DialogMove(-1));
        execute_command(&mut app, Command::DialogActivate);
        assert!(!app.should_quit);
        assert!(app.dialog.is_none());
        assert_eq!(app.focus, FocusTarget::Editor);

        execute_command(&mut app, Command::Quit);
        execute_command(&mut app, Command::DialogMove(1));
        execute_command(&mut app, Command::DialogActivate);
        assert!(app.should_quit, "Don't Save was the last file's answer");
        assert_eq!(app.tabs.len(), 2, "and it closed the tab it discarded");
    }

    /// ADR-047: the quit prompt was one question with one Quit Anyway, so
    /// saving four files on the way out was not something it could express.
    #[test]
    fn quitting_asks_about_every_unsaved_file_in_turn() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::fixture_in(dir.path());
        let paths: Vec<PathBuf> = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, "one\n").unwrap();
                app.open_path(&path, None).unwrap();
                path
            })
            .collect();
        for index in 0..3 {
            app.active_tab = Some(index);
            execute_command(&mut app, Command::InsertChar('!'));
        }

        // Enter three times: Save is the first button, so holding it down saves
        // everything and quits.
        for left in [3, 2, 1] {
            execute_command(&mut app, Command::Quit);
            assert!(!app.should_quit, "{left} still to answer");
            let dialog = app.dialog.as_ref().expect("a prompt");
            assert!(
                dialog.prompt().ends_with("has unsaved changes."),
                "{}",
                dialog.prompt()
            );
            let expected = if left > 1 {
                format!("Unsaved changes ({left} left)")
            } else {
                "Unsaved changes".to_string()
            };
            assert_eq!(dialog.title, expected, "the title counts the walk down");
            execute_command(&mut app, Command::DialogActivate);
        }
        assert!(app.should_quit);
        for path in &paths {
            assert_eq!(std::fs::read_to_string(path).unwrap(), "!one\n");
        }
    }

    /// Cancel stops the quit, and leaves the answers already given standing:
    /// each one was final when it was made, and a save is not something to
    /// take back.
    #[test]
    fn a_cancel_partway_through_the_walk_stops_the_quit() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::fixture_in(dir.path());
        for name in ["a.txt", "b.txt"] {
            let path = dir.path().join(name);
            std::fs::write(&path, "one\n").unwrap();
            app.open_path(&path, None).unwrap();
        }
        for index in 0..2 {
            app.active_tab = Some(index);
            execute_command(&mut app, Command::InsertChar('!'));
        }

        execute_command(&mut app, Command::Quit);
        execute_command(&mut app, Command::DialogActivate);
        execute_command(&mut app, Command::DialogMove(-1));
        execute_command(&mut app, Command::DialogActivate);

        assert!(!app.should_quit, "Cancel ends the walk, not the editor");
        assert!(app.dialog.is_none());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "!one\n",
            "the file already answered for stays saved"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(app.tabs.len(), 1, "and only the answered tab was closed");
    }

    /// A save that fails is the one answer that must not go on: quitting past
    /// it would take the buffer with it.
    #[test]
    fn a_failed_save_stops_the_quit_where_it_is() {
        let mut app = app();
        // A scratch tab has no path, so its save fails.
        execute_command(&mut app, Command::Quit);
        execute_command(&mut app, Command::DialogActivate);

        assert!(!app.should_quit, "the buffer is still the only copy");
        assert!(
            app.dialog.is_none(),
            "and the walk stopped rather than looped"
        );
        assert_eq!(app.tabs.len(), 3, "nothing was closed");
        let notification = app.notifications.current().unwrap();
        assert!(
            notification.message.starts_with("Failed to save:"),
            "{}",
            notification.message
        );
    }

    // --- what the status bar says about a file (ADR-058) ------------------

    /// A tab over a real file with the text and endings given, so the
    /// conversion can be checked where it happens: on disk.
    fn app_over_file(dir: &std::path::Path, text: &str) -> App {
        let path = dir.join("a.txt");
        std::fs::write(&path, text).unwrap();
        let mut app = App::fixture_in(dir);
        app.tabs = vec![TabItem::editing(crate::app::Tab::new(
            Document::open(&path).unwrap(),
        ))];
        app.active_tab = Some(0);
        app
    }

    /// The picker offers both endings and marks the one the file has; choosing
    /// a row asks rather than converting, because the conversion rewrites every
    /// line of the file and the picker is one click from a readout.
    #[test]
    fn the_line_ending_picker_asks_before_it_rewrites_anything() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_over_file(dir.path(), "one\ntwo\n");

        execute_command(&mut app, Command::LineEndingPrompt);
        let dialog = app.dialog.as_ref().expect("a picker");
        let rows: Vec<(&str, bool)> = dialog
            .rows()
            .map(|item| (item.label.as_str(), item.current))
            .collect();
        assert_eq!(
            rows,
            vec![("LF — Unix, macOS", true), ("CRLF — Windows", false)]
        );

        // Down to CRLF, then the default Choose button.
        execute_command(&mut app, Command::DialogListMove(1));
        execute_command(&mut app, Command::DialogActivate);
        let dialog = app.dialog.as_ref().expect("a confirmation");
        assert_eq!(dialog.title, "Convert Line Endings");
        assert_eq!(
            dialog.buttons[0].label, "Cancel",
            "the default is the safe one"
        );
        assert_eq!(
            std::fs::read(dir.path().join("a.txt")).unwrap(),
            b"one\ntwo\n",
            "and nothing has been written yet"
        );
    }

    #[test]
    fn converting_rewrites_the_file_and_leaves_it_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_over_file(dir.path(), "one\ntwo\n");

        execute_command(&mut app, Command::ConvertLineEnding(LineEnding::Crlf));

        assert_eq!(
            std::fs::read(dir.path().join("a.txt")).unwrap(),
            b"one\r\ntwo\r\n"
        );
        let document = &app.active().unwrap().document;
        assert_eq!(document.line_ending(), LineEnding::Crlf);
        assert!(!document.is_dirty(), "the buffer is what is on disk again");
        assert_eq!(
            app.notifications.current().map(|n| n.message.clone()),
            Some("Converted to CRLF line endings".to_string())
        );
    }

    /// The rope holds `\n` alone whatever the file used, so a conversion must
    /// not touch a character of the text.
    #[test]
    fn converting_changes_the_endings_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_over_file(dir.path(), "one\r\ntwo\r\n");
        assert_eq!(
            app.active().unwrap().document.line_ending(),
            LineEnding::Crlf
        );

        execute_command(&mut app, Command::ConvertLineEnding(LineEnding::Lf));

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(app.active().unwrap().document.line(0), "one");
    }

    /// Choosing the ending the file already has is not a question.
    #[test]
    fn choosing_the_ending_the_file_already_has_says_so_and_asks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_over_file(dir.path(), "one\ntwo\n");

        execute_command(&mut app, Command::ConvertLineEndingPrompt(LineEnding::Lf));

        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().map(|n| n.message.clone()),
            Some("Already LF line endings".to_string())
        );
    }

    /// There is nothing to rewrite before the buffer has a file, so the choice
    /// is kept for the first Save As rather than reported as a failed save.
    #[test]
    fn converting_an_unnamed_buffer_keeps_the_choice_without_a_save() {
        let mut app = app();
        app.tabs = vec![TabItem::editing(crate::app::Tab::new(Document::from_text(
            "one\ntwo\n",
            None,
        )))];
        app.active_tab = Some(0);
        execute_command(&mut app, Command::ConvertLineEnding(LineEnding::Crlf));

        let document = &app.active().unwrap().document;
        assert_eq!(document.line_ending(), LineEnding::Crlf);
        assert!(document.is_dirty(), "the write is still owed");
        assert_eq!(
            app.notifications.current().map(|n| n.message.clone()),
            Some("Line endings set to CRLF".to_string())
        );
    }

    /// The picker starts on the charset the file was read with, and choosing a
    /// row asks what to do with it rather than doing either thing (ADR-059).
    #[test]
    fn the_encoding_picker_starts_on_the_file_s_own_charset_and_then_asks() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_over_file(dir.path(), "one\ntwo\n");

        execute_command(&mut app, Command::EncodingPrompt);
        let dialog = app.dialog.as_ref().expect("a picker");
        assert_eq!(
            dialog.selected_item().map(|item| item.label.as_str()),
            Some("UTF-8")
        );
        assert!(dialog.field_filters(), "long enough to need a filter");

        for ch in "1251".chars() {
            execute_command(&mut app, Command::DialogInputChar(ch));
        }
        let rows: Vec<&str> = app
            .dialog
            .as_ref()
            .unwrap()
            .rows()
            .map(|item| item.label.as_str())
            .collect();
        assert_eq!(rows, vec!["Windows-1251"]);

        execute_command(&mut app, Command::DialogActivate);
        let dialog = app.dialog.as_ref().expect("a confirmation");
        let buttons: Vec<&str> = dialog.buttons.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(buttons, vec!["Cancel", "Reopen", "Convert and Save"]);
        assert_eq!(
            std::fs::read(dir.path().join("a.txt")).unwrap(),
            b"one\ntwo\n",
            "and nothing has been written yet"
        );
    }

    /// The two answers a chosen encoding can have: keep the bytes and read them
    /// again, or keep the text and write it out differently.
    #[test]
    fn reopening_reads_the_same_bytes_again_and_converting_rewrites_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let koi8 = Charset::by_label("KOI8-U").unwrap();
        let mut bytes = Vec::new();
        koi8.encode("Привіт\n", &mut bytes);
        std::fs::write(&path, &bytes).unwrap();

        let mut app = app();
        app.tabs = vec![TabItem::editing(crate::app::Tab::new(
            Document::open(&path).unwrap(),
        ))];
        app.active_tab = Some(0);
        assert_ne!(app.active().unwrap().document.line(0), "Привіт");

        execute_command(&mut app, Command::ReopenWithEncoding("KOI8-U".into()));
        assert_eq!(app.active().unwrap().document.line(0), "Привіт");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "reopening reads and does not write"
        );

        execute_command(&mut app, Command::ConvertEncoding("UTF-8".into()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Привіт\n");
        assert_eq!(app.active().unwrap().document.charset(), Charset::UTF8);
    }

    /// A conversion that cannot hold the text writes nothing, and leaves the
    /// tab on the charset it can still be saved in.
    #[test]
    fn converting_to_a_charset_that_cannot_hold_the_text_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_over_file(dir.path(), "Привіт\n");

        execute_command(&mut app, Command::ConvertEncoding("Windows-1252".into()));

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "Привіт\n"
        );
        assert_eq!(app.active().unwrap().document.charset(), Charset::UTF8);
        let message = app.notifications.current().unwrap().message.clone();
        assert!(message.contains("Windows-1252 cannot hold"), "{message}");
    }

    /// Choosing the charset the file already has is not a question.
    #[test]
    fn choosing_the_charset_the_file_already_has_says_so_and_asks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_over_file(dir.path(), "one\n");

        execute_command(&mut app, Command::EncodingChoice("UTF-8".into()));

        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().map(|n| n.message.clone()),
            Some("Already UTF-8".to_string())
        );
    }

    /// The grammar the user names outranks the one detection chose, and the
    /// status bar names it because both read the same cache.
    #[test]
    fn a_chosen_grammar_replaces_the_detected_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_over_file(dir.path(), "one\ntwo\n");
        app.active_mut().unwrap().sync_highlight(10);
        assert_eq!(app.active().unwrap().highlights.language(), "Plain Text");

        execute_command(&mut app, Command::SetLanguage("Rust".into()));
        assert_eq!(app.active().unwrap().highlights.language(), "Rust");

        // And it survives the next frame, which is what would re-detect it.
        app.active_mut().unwrap().sync_highlight(10);
        assert_eq!(app.active().unwrap().highlights.language(), "Rust");
    }

    #[test]
    fn a_grammar_no_one_has_is_reported_rather_than_applied() {
        let mut app = app();
        execute_command(&mut app, Command::SetLanguage("Sindarin".into()));
        assert_eq!(
            app.notifications.current().map(|n| n.message.clone()),
            Some("No grammar named Sindarin".to_string())
        );
    }

    /// Hundreds of grammars are unreachable ten rows at a time, so this picker
    /// is the one with a filter — and the filter is what its typed keys edit.
    #[test]
    fn the_syntax_picker_narrows_as_you_type_and_chooses_what_is_left() {
        let mut app = app();
        execute_command(&mut app, Command::LanguagePrompt);
        let all = app.dialog.as_ref().unwrap().rows().count();
        assert!(all > 20, "the whole syntax set is offered: {all}");

        for ch in "rust".chars() {
            execute_command(&mut app, Command::DialogInputChar(ch));
        }
        let dialog = app.dialog.as_ref().expect("still open");
        let rows: Vec<&str> = dialog.rows().map(|item| item.label.as_str()).collect();
        assert_eq!(rows, vec!["Rust"]);

        execute_command(&mut app, Command::DialogActivate);
        assert!(app.dialog.is_none());
        assert_eq!(app.active().unwrap().highlights.language(), "Rust");
    }

    /// The branch on the status bar is the one readout whose dialog is not
    /// about the file, and the picker is the only thing a branch name could
    /// sensibly open (ADR-058).
    #[test]
    fn the_branch_on_the_status_bar_opens_the_switch_picker() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["branch", "topic"]);
        let mut app = App::fixture_in(repo.path());
        execute_command(&mut app, Command::GitRefresh);

        let rects = crate::ui::layout::compute(ratatui::layout::Rect::new(0, 0, 100, 24), &app);
        let (_, branch) = rects
            .status_zones
            .iter()
            .find(|(zone, _)| *zone == crate::ui::statusbar::StatusZone::Branch)
            .expect("the branch is a zone");
        assert_eq!(
            crate::event::mouse::hit_test(
                &app,
                &rects,
                crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Down(
                        crossterm::event::MouseButton::Left
                    ),
                    column: branch.x,
                    row: branch.y,
                    modifiers: crossterm::event::KeyModifiers::NONE,
                }
            ),
            Some(Command::GitBranchPrompt)
        );

        execute_command(&mut app, Command::GitBranchPrompt);
        let dialog = app.dialog.as_ref().expect("a picker");
        assert_eq!(dialog.title, "Switch Branch");
        let labels: Vec<&str> = dialog.rows().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, vec!["main", "topic"]);
    }

    /// Outside a repository there is nothing to switch to, and the click says
    /// so rather than opening an empty box.
    #[test]
    fn the_branch_readout_outside_a_repository_only_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::GitRefresh);
        execute_command(&mut app, Command::GitBranchPrompt);

        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().map(|n| n.message.clone()),
            Some("Not a Git repository".to_string())
        );
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

    /// A tab over one line far wider than the pane, for the two commands that
    /// are about a line that does not fit.
    fn wide_app() -> App {
        let mut app = app();
        app.tabs[0] = TabItem::editing(crate::app::Tab::scratch(
            "wide.txt",
            &"abcdefghij ".repeat(30),
        ));
        app.active_tab = Some(0);
        app
    }

    #[test]
    fn word_wrap_is_switched_on_and_off_and_says_so() {
        let mut app = app();
        assert!(!app.settings.word_wrap, "off until it is asked for");
        execute_command(&mut app, Command::ToggleWordWrap);
        assert!(app.settings.word_wrap);
        assert_eq!(
            app.notifications.current().map(|n| n.message.clone()),
            Some("Word wrap on".to_string())
        );
        execute_command(&mut app, Command::ToggleWordWrap);
        assert!(!app.settings.word_wrap);
    }

    /// A pane that wraps has no sideways axis, so the window it was scrolled
    /// to has to go — otherwise the text would come back drawn from column 40.
    #[test]
    fn turning_wrapping_on_brings_every_tab_back_to_the_left_edge() {
        let mut app = wide_app();
        execute_command(&mut app, Command::ScrollEditorHorizontal(2));
        assert_eq!(
            app.active().unwrap().viewport.left_col,
            VisualCol(2 * crate::editor::viewport::HORIZONTAL_STEP)
        );
        execute_command(&mut app, Command::ToggleWordWrap);
        assert_eq!(app.active().unwrap().viewport.left_col, VisualCol(0));
    }

    #[test]
    fn the_editor_scrolls_sideways_and_stops_at_the_end_of_the_line() {
        let mut app = wide_app();
        execute_command(&mut app, Command::ScrollEditorHorizontal(1));
        assert_eq!(
            app.active().unwrap().viewport.left_col,
            VisualCol(crate::editor::viewport::HORIZONTAL_STEP)
        );
        execute_command(&mut app, Command::ScrollEditorHorizontal(1000));
        assert_eq!(
            app.active().unwrap().viewport.left_col,
            VisualCol(329),
            "the last column of the widest line on screen"
        );
        execute_command(&mut app, Command::ScrollEditorHorizontal(-1000));
        assert_eq!(app.active().unwrap().viewport.left_col, VisualCol(0));
    }

    /// The whole way in, not just the command: a key press resolves to the
    /// scroll and the scroll reaches the window. `Alt` is the modifier a
    /// terminal is most likely to eat, so the View menu carries the same two
    /// commands and is checked here with them.
    #[test]
    fn the_sideways_keys_and_menu_entries_both_move_the_window() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = wide_app();
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::ALT);
        let command = crate::event::keyboard::resolve(key, app.focus, app.dialog_wants_text())
            .expect("Alt+Right is bound in the editor");
        execute_command(&mut app, command);
        assert_eq!(
            app.active().unwrap().viewport.left_col,
            VisualCol(crate::editor::viewport::HORIZONTAL_STEP)
        );

        let view = MENUS
            .iter()
            .position(|menu| menu.title == "View")
            .expect("a View menu");
        let row = MENUS[view]
            .items
            .iter()
            .position(|entry| {
                entry.item().map(|item| &item.command) == Some(&Command::ScrollEditorHorizontal(-1))
            })
            .expect("a Scroll Left entry");
        execute_command(&mut app, Command::MenuOpen(view));
        execute_command(&mut app, Command::MenuActivateItem(row));
        assert_eq!(app.active().unwrap().viewport.left_col, VisualCol(0));
    }

    #[test]
    fn scrolling_sideways_does_nothing_while_lines_wrap() {
        let mut app = wide_app();
        execute_command(&mut app, Command::ToggleWordWrap);
        execute_command(&mut app, Command::ScrollEditorHorizontal(3));
        assert_eq!(app.active().unwrap().viewport.left_col, VisualCol(0));
    }

    #[test]
    fn going_to_a_line_puts_the_cursor_on_it() {
        let mut app = app();
        app.tabs[0] = TabItem::editing(crate::app::Tab::scratch(
            "lines.txt",
            "one
two
three
four
five",
        ));
        execute_command(&mut app, Command::GotoLine("4".into()));
        assert_eq!(app.active().unwrap().document.cursor().line, 3);
        // Past the end is the end, which is where Ctrl+End would have gone.
        execute_command(&mut app, Command::GotoLine("9999".into()));
        assert_eq!(app.active().unwrap().document.cursor().line, 4);
    }

    #[test]
    fn a_line_number_that_is_not_one_is_reported_and_moves_nothing() {
        let mut app = app();
        let before = app.active().unwrap().document.cursor();
        execute_command(&mut app, Command::GotoLine("banana".into()));
        assert_eq!(app.active().unwrap().document.cursor(), before);
        assert_eq!(
            app.notifications.current().map(|n| n.message.clone()),
            Some("Not a line number: banana".to_string())
        );
    }

    /// The dialog is the way in from the menu and from `Ctrl+G`: it opens on
    /// the line the cursor is on, and its button carries the typed number
    /// through the same pairing as every other input dialog.
    #[test]
    fn the_go_to_line_dialog_jumps_to_what_was_typed() {
        let mut app = app();
        app.tabs[0] = TabItem::editing(crate::app::Tab::scratch(
            "lines.txt",
            "one
two
three
four
five",
        ));
        execute_command(&mut app, Command::GotoLinePrompt);
        let dialog = app.dialog.as_ref().expect("a dialog");
        assert_eq!(dialog.field().unwrap().value, "1", "the cursor's own line");
        assert!(dialog.prompt().contains('5'), "{}", dialog.prompt());
        execute_command(&mut app, Command::DialogInputBackspace);
        execute_command(&mut app, Command::DialogInputChar('3'));
        execute_command(&mut app, Command::DialogActivate);
        assert!(app.dialog.is_none());
        assert_eq!(app.active().unwrap().document.cursor().line, 2);
        assert_eq!(app.focus, FocusTarget::Editor);
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
        app.tabs.retain(|tab| !tab.is_dirty());
        execute_command(&mut app, Command::MenuOpen(0));
        let quit_index = MENUS[0]
            .items
            .iter()
            .position(|i| i.item().map(|i| &i.command) == Some(&Command::Quit))
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
            .position(|i| i.item().map(|i| &i.command) == Some(&Command::Undo))
            .expect("an Undo item");
        execute_command(&mut app, Command::MenuOpen(edit_menu));
        execute_command(&mut app, Command::MenuActivateItem(undo_index));
        assert_eq!(app.active().unwrap().document.line(0), "fn main() {");
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
            .position(|i| i.item().map(|i| &i.command) == Some(&Command::Cut))
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
        assert!(app.tabs.iter().all(|t| t.title() != "editor.rs"));
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
        assert!(app.tab_mut(1).document.is_dirty());
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
        assert!(!app.tab_mut(1).document.is_dirty(), "ADR-014");
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
            .position(|i| i.item().map(|i| &i.command) == Some(&Command::CloseTab))
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
            .position(|i| i.item().map(|i| &i.command) == Some(&Command::CloseTab))
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
        app.tabs[1] = crate::app::TabItem::editing(crate::app::Tab::scratch(
            "long.txt",
            &"line\n".repeat(40),
        ));
        app.tab_mut(1).document.goto_line(30);
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

    /// ADR-046: a command the editor declined is a warning, every time.
    ///
    /// Worth a test rather than a paragraph because the colour is the part of a
    /// status line a reader takes in without reading it, and what this replaced
    /// was fifteen call sites each deciding it on its own — "Nothing to undo"
    /// in the same blue as "Saved main.rs".
    #[test]
    fn a_command_that_changed_nothing_warns() {
        use crate::app::notifications::NotificationKind::Warning;

        fn declined(mut app: App, command: Command) -> String {
            let named = format!("{command:?}");
            execute_command(&mut app, command);
            let notification = app
                .notifications
                .current()
                .expect("a declined command says so");
            assert_eq!(
                notification.kind, Warning,
                "{named}: {}",
                notification.message
            );
            notification.message.clone()
        }

        let no_tabs = || {
            let mut app = App::fixture();
            app.tabs.clear();
            app.active_tab = None;
            app
        };
        let no_changes = || {
            let mut app = App::fixture();
            app.git.status.entries.clear();
            app
        };

        assert_eq!(declined(no_tabs(), Command::CloseTab), "No tab to close");
        assert_eq!(declined(no_tabs(), Command::Save), "No file to save");
        assert_eq!(declined(App::fixture(), Command::Undo), "Nothing to undo");
        assert_eq!(declined(App::fixture(), Command::Redo), "Nothing to redo");
        assert_eq!(declined(App::fixture(), Command::Copy), "Nothing selected");
        assert_eq!(
            declined(App::fixture(), Command::Paste),
            "The clipboard is empty"
        );
        assert_eq!(
            declined(no_changes(), Command::GitStage),
            "Nothing selected in the Git panel"
        );
        assert_eq!(
            declined(App::fixture(), Command::GitCancel),
            "Nothing to cancel"
        );

        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            declined(App::fixture_in(empty.path()), Command::ExplorerActivate),
            "The explorer is empty"
        );
        assert_eq!(
            declined(App::fixture_in(empty.path()), Command::RenamePrompt),
            "Nothing selected in the explorer"
        );
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
        select_sidebar_row(app, index);
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
            app.tab_mut(0).document.path().unwrap(),
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
            app.tab_mut(0).document.path().unwrap(),
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
        assert_eq!(
            rows(&app),
            vec!["build", "src", ".gitignore", "README.md"],
            "the ignored directory and the dotfile are listed to begin with"
        );

        execute_command(&mut app, Command::ToggleHiddenFiles);
        assert_eq!(
            rows(&app),
            vec!["src", "README.md"],
            "and the toggle hides them"
        );

        execute_command(&mut app, Command::ToggleHiddenFiles);
        assert_eq!(rows(&app), vec!["build", "src", ".gitignore", "README.md"]);
    }
    // --- search ------------------------------------------------------------

    /// A one-tab app over `text`, which is what every search test wants: the
    /// fixture's three tabs would only make the assertions about tab indices.
    fn searchable(text: &str) -> App {
        let mut app = App::fixture();
        app.tabs = vec![crate::app::TabItem::editing(crate::app::Tab::scratch(
            "notes.txt",
            text,
        ))];
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

    /// The names the browser is showing, `..` included.
    fn browser_rows(app: &App) -> Vec<String> {
        app.dialog
            .as_ref()
            .and_then(|dialog| dialog.browser())
            .expect("a browser is open")
            .rows()
            .map(|entry| entry.name.clone())
            .collect()
    }

    fn browser_selected(app: &App) -> String {
        app.dialog
            .as_ref()
            .and_then(|dialog| dialog.browser())
            .and_then(|browser| browser.selected_entry())
            .expect("a selected row")
            .name
            .clone()
    }

    /// Types into the browser's filter without pressing Enter afterwards.
    fn filter(app: &mut App, text: &str) {
        execute_command(app, Command::DialogInputText(text.into()));
    }

    #[test]
    fn the_browser_opens_on_the_workspace_root_whatever_the_explorer_is_on() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        // The explorer is sitting inside `src`, which is *not* where the
        // browser opens: the sidebar's place is a spot in a tree the dialog is
        // not showing.
        select(&mut app, "src");
        execute_command(&mut app, Command::ExplorerExpand);
        select(&mut app, "main.rs");

        execute_command(&mut app, Command::OpenPrompt);
        assert!(app.dialog_wants_text(), "the filter takes typing");
        assert_eq!(browser_rows(&app), vec!["..", "src", "README.md"]);
        type_into_dialog(&mut app, "README.md");

        assert_eq!(app.active().unwrap().document.title(), "README.md");
        assert_eq!(app.focus, FocusTarget::Editor);
        assert!(app.dialog.is_none());
    }

    #[test]
    fn the_browser_walks_into_a_directory_and_back_out_of_it() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        assert_eq!(browser_rows(&app), vec!["..", "src", "README.md"]);

        // Down onto `src`, then Enter: the dialog stays open, one level in.
        execute_command(&mut app, Command::DialogListMove(1));
        assert_eq!(browser_selected(&app), "src");
        execute_command(&mut app, Command::DialogActivate);
        assert!(app.dialog.is_some(), "walking in is not answering");
        assert_eq!(browser_rows(&app), vec!["..", "main.rs"]);

        // `..` is selected on arrival, so Enter again goes back.
        assert_eq!(browser_selected(&app), "..");
        execute_command(&mut app, Command::DialogActivate);
        assert_eq!(browser_rows(&app), vec!["..", "src", "README.md"]);
        assert!(app.tabs.is_empty(), "and nothing was opened on the way");
    }

    #[test]
    fn the_filter_narrows_the_rows_and_enter_opens_what_is_left() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        filter(&mut app, "read");
        assert_eq!(browser_rows(&app), vec!["..", "README.md"]);
        assert_eq!(browser_selected(&app), "README.md");

        execute_command(&mut app, Command::DialogActivate);
        assert_eq!(app.active().unwrap().document.title(), "README.md");
    }

    #[test]
    fn open_folder_makes_the_selected_directory_the_workspace() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        execute_command(&mut app, Command::DialogListMove(1));
        assert_eq!(browser_selected(&app), "src");

        // The second button, `Open Folder`.
        execute_command(&mut app, Command::DialogActivateButton(1));
        assert!(app.dialog.is_none());
        assert_eq!(app.workspace.name(), "src");
        assert_eq!(rows(&app), vec!["main.rs"]);
        assert_eq!(app.focus, FocusTarget::Explorer);
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Opened src",
            "a sidebar that moved says so"
        );
    }

    #[test]
    fn open_folder_on_a_file_row_means_the_directory_being_listed() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        execute_command(&mut app, Command::DialogListMove(1));
        execute_command(&mut app, Command::DialogActivate);
        assert_eq!(browser_rows(&app), vec!["..", "main.rs"]);
        execute_command(&mut app, Command::DialogListMove(1));
        assert_eq!(browser_selected(&app), "main.rs");

        execute_command(&mut app, Command::DialogActivateButton(1));
        assert_eq!(app.workspace.name(), "src");
        assert!(app.tabs.is_empty(), "a folder was opened, not the file");
    }

    #[test]
    fn a_file_from_outside_the_workspace_brings_the_sidebar_with_it() {
        let dir = project();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("elsewhere.txt"), "out of tree\n").unwrap();

        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        type_into_dialog(
            &mut app,
            outside.path().join("elsewhere.txt").to_str().unwrap(),
        );

        assert_eq!(app.active().unwrap().document.title(), "elsewhere.txt");
        assert_eq!(
            app.workspace.root(),
            std::fs::canonicalize(outside.path()).unwrap(),
            "the sidebar shows the folder the file is in (SPEC §6)"
        );
        assert_eq!(rows(&app), vec!["elsewhere.txt"]);
    }

    #[test]
    fn a_file_from_inside_the_workspace_leaves_the_workspace_alone() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let root = app.workspace.root().to_path_buf();

        execute_command(&mut app, Command::OpenPrompt);
        execute_command(&mut app, Command::DialogListMove(1));
        execute_command(&mut app, Command::DialogActivate);
        execute_command(&mut app, Command::DialogListMove(1));
        execute_command(&mut app, Command::DialogActivate);

        assert_eq!(app.active().unwrap().document.title(), "main.rs");
        assert_eq!(
            app.workspace.root(),
            root,
            "re-rooting to `src` would throw away the project around it"
        );
        assert_eq!(
            app.sidebar.selected_row().map(|row| row.name.clone()),
            Some("main.rs".to_string()),
            "and the file is revealed where it already lived"
        );
    }

    #[test]
    fn opening_a_path_that_is_not_there_yet_starts_a_buffer_for_it() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        // Nothing in the listing matches, so the filter is read as the answer.
        type_into_dialog(&mut app, "notes.md");

        assert_eq!(app.active().unwrap().document.title(), "notes.md");
        assert!(
            !dir.path().join("notes.md").exists(),
            "the file appears when it is saved, as it does from the command line"
        );
    }

    #[test]
    fn enter_on_an_empty_filter_acts_on_the_row_that_is_selected() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::OpenPrompt);
        // `..` is where the selection starts, and it is a directory.
        execute_command(&mut app, Command::DialogActivate);

        assert!(app.tabs.is_empty(), "nothing was opened");
        assert!(app.dialog.is_some(), "the browser went up instead");
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

    /// A stopped rebase is not finished by writing a commit here, so the
    /// dialog says what does finish it instead of opening (ADR-041).
    #[test]
    fn a_stopped_rebase_is_not_offered_a_commit_dialog() {
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
        let (mut app, _rx) = app_over(&repo);

        assert!(!repo.try_run(&["rebase", "other"]).status.success());
        // Resolve it, so the conflict gate is not what is being tested.
        repo.write("c.txt", "resolved\n");
        repo.run(&["add", "c.txt"]);
        execute_command(&mut app, Command::GitRefresh);
        assert_eq!(app.git.status.operation, Some(Operation::Rebase));

        // Nothing new to stage: the resolution is already in the index, and
        // `staged_count` counts it, so the dialog opens. Unstage it to reach
        // the branch that matters.
        repo.run(&["restore", "--staged", "c.txt"]);
        repo.run(&["checkout", "--", "c.txt"]);
        execute_command(&mut app, Command::GitRefresh);
        execute_command(&mut app, Command::GitCommitPrompt);
        assert!(app.dialog.is_none(), "a rebase is not committed from here");
        let said = app.notifications.current().unwrap().message.clone();
        assert!(said.contains("git rebase --continue"), "{said}");
    }

    // --- buffers whose files moved (ADR-043) -------------------------------

    /// The change the watcher would report, without a watcher in the test.
    fn worktree_changed(app: &mut App) {
        execute_command(
            app,
            Command::ExternalChange(FsChange {
                worktree: true,
                repository: true,
            }),
        );
    }

    /// The gap ADR-040 made visible and did not close: the panes caught up and
    /// the document did not. A clean buffer has nothing to lose, so it follows
    /// its file without asking.
    #[test]
    fn a_clean_tab_follows_its_file() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();

        std::fs::write(&path, "# rewritten elsewhere\n").unwrap();
        worktree_changed(&mut app);

        assert_eq!(
            app.active().unwrap().document.line(0),
            "# rewritten elsewhere"
        );
        assert!(app.dialog.is_none(), "nothing to ask about");
        assert!(app.tab_mut(0).stale.is_none());
    }

    /// The other half of the same rule: a buffer with unsaved work in it is
    /// the only copy of that work, and no filesystem event may spend it.
    #[test]
    fn a_modified_tab_is_asked_about_rather_than_replaced() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();
        execute_command(&mut app, Command::InsertText("mine\n".into()));

        std::fs::write(&path, "# rewritten elsewhere\n").unwrap();
        worktree_changed(&mut app);

        assert_eq!(app.tab_mut(0).stale, Some(Stale::Changed));
        assert!(
            app.active().unwrap().document.line(0).starts_with("mine"),
            "the buffer is untouched"
        );
        let dialog = app.dialog.as_ref().expect("a question");
        assert!(dialog.prompt().contains("changed on disk"), "{dialog:?}");
        assert_eq!(
            dialog.command_at(0),
            Some(Command::KeepBuffer(0)),
            "the answer that does nothing is the default"
        );
        assert_eq!(dialog.command_at(1), Some(Command::ReloadTab(0)));
    }

    /// The watcher reports every burst in the workspace. Without this the same
    /// question would re-open on every build for as long as the tab stayed
    /// unresolved.
    #[test]
    fn the_same_change_is_only_asked_about_once() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();
        execute_command(&mut app, Command::InsertText("mine\n".into()));
        std::fs::write(&path, "# theirs\n").unwrap();

        worktree_changed(&mut app);
        assert!(app.dialog.is_some());
        execute_command(&mut app, Command::DialogCancel);

        // Something else in the workspace moves; this file did not.
        std::fs::write(dir.path().join("other.txt"), "x\n").unwrap();
        worktree_changed(&mut app);
        assert!(app.dialog.is_none(), "the question is not re-opened");
        assert_eq!(
            app.tab_mut(0).stale,
            Some(Stale::Changed),
            "but it is still marked"
        );
    }

    #[test]
    fn keeping_the_buffer_settles_the_tab() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();
        execute_command(&mut app, Command::InsertText("mine\n".into()));
        std::fs::write(&path, "# theirs\n").unwrap();
        worktree_changed(&mut app);

        execute_command(&mut app, Command::DialogActivate);
        assert!(app.dialog.is_none());
        assert!(app.tab_mut(0).stale.is_none(), "answered");
        assert!(app.active().unwrap().document.line(0).starts_with("mine"));
    }

    #[test]
    fn reloading_from_the_question_takes_the_file_and_can_be_undone() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();
        execute_command(&mut app, Command::InsertText("mine\n".into()));
        std::fs::write(&path, "# theirs\n").unwrap();
        worktree_changed(&mut app);

        execute_command(&mut app, Command::DialogMove(1));
        execute_command(&mut app, Command::DialogActivate);
        assert!(app.dialog.is_none());
        assert!(app.tab_mut(0).stale.is_none());
        assert_eq!(app.active().unwrap().document.line(0), "# theirs");

        execute_command(&mut app, Command::Undo);
        assert!(
            app.active().unwrap().document.line(0).starts_with("mine"),
            "a reload the user did not mean is not the one thing that cannot be taken back"
        );
    }

    /// A file that vanished under a clean buffer has no question attached to
    /// it: nothing would be lost by keeping it, and there is nothing to reload
    /// from. It is said once.
    #[test]
    fn a_file_that_vanished_under_a_clean_tab_is_said_once() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();

        std::fs::remove_file(&path).unwrap();
        worktree_changed(&mut app);

        assert!(app.dialog.is_none());
        assert_eq!(app.tab_mut(0).stale, Some(Stale::Gone));
        assert_eq!(
            app.notifications.current().unwrap().message,
            "README.md is gone from disk"
        );
        assert_eq!(
            app.active().unwrap().document.line(0),
            "# project",
            "the buffer is the last copy and is kept"
        );
    }

    /// A modified tab whose file is gone *is* asked about — but with one
    /// answer, because there is nothing to reload from.
    #[test]
    fn a_modified_tab_whose_file_is_gone_is_offered_only_its_buffer() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();
        execute_command(&mut app, Command::InsertText("mine\n".into()));

        std::fs::remove_file(&path).unwrap();
        worktree_changed(&mut app);

        let dialog = app.dialog.as_ref().expect("a question");
        assert!(dialog.prompt().contains("gone from disk"), "{dialog:?}");
        assert_eq!(dialog.buttons.len(), 1, "nothing to reload from");
    }

    /// A save is an answer to the question too: whatever the file was, it is
    /// the buffer now.
    #[test]
    fn saving_over_the_change_settles_the_tab() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();
        execute_command(&mut app, Command::InsertText("mine\n".into()));
        std::fs::write(&path, "# theirs\n").unwrap();
        worktree_changed(&mut app);
        execute_command(&mut app, Command::DialogCancel);

        execute_command(&mut app, Command::Save);
        assert!(app.tab_mut(0).stale.is_none());
        assert!(
            std::fs::read_to_string(&path).unwrap().starts_with("mine"),
            "and the file is what the buffer was"
        );
    }

    /// Only the active tab is asked about: a modal question over a document
    /// the user cannot see is a question about nothing.
    #[test]
    fn a_background_tab_is_marked_and_asked_about_when_it_comes_forward() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let readme = dir.path().join("README.md");
        app.open_path(&readme, None).unwrap();
        execute_command(&mut app, Command::InsertText("mine\n".into()));
        app.open_path(&dir.path().join("src/main.rs"), None)
            .unwrap();
        assert_eq!(app.active_tab, Some(1));

        std::fs::write(&readme, "# theirs\n").unwrap();
        worktree_changed(&mut app);
        assert!(app.dialog.is_none(), "the tab on screen did not move");
        assert_eq!(app.tab_mut(0).stale, Some(Stale::Changed));

        execute_command(&mut app, Command::PrevTab);
        let dialog = app.dialog.as_ref().expect("asked once it is in front");
        assert_eq!(dialog.command_at(0), Some(Command::KeepBuffer(0)));
    }

    /// `F5` in the editor is the same "show me what is really there" the two
    /// panels bind it to, and it asks before it spends anything.
    #[test]
    fn reload_asks_only_when_there_is_something_to_lose() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        let path = dir.path().join("README.md");
        app.open_path(&path, None).unwrap();
        std::fs::write(&path, "# theirs\n").unwrap();

        execute_command(&mut app, Command::Reload);
        assert!(app.dialog.is_none(), "a clean buffer has nothing to lose");
        assert_eq!(app.active().unwrap().document.line(0), "# theirs");
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Reloaded README.md",
            "a reload the user asked for says so"
        );

        execute_command(&mut app, Command::InsertText("mine\n".into()));
        execute_command(&mut app, Command::Reload);
        assert!(app.dialog.is_some(), "and a modified one is asked about");
    }

    #[test]
    fn reload_with_no_tab_open_says_so() {
        let dir = project();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::Reload);
        assert!(app.dialog.is_none());
        assert_eq!(
            app.notifications.current().unwrap().message,
            "No file to reload"
        );
    }

    // --- cancelling a job (ADR-044) ----------------------------------------

    #[test]
    fn cancelling_with_nothing_running_says_so() {
        let mut app = app();
        execute_command(&mut app, Command::GitCancel);
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Nothing to cancel"
        );
    }

    #[test]
    fn cancelling_names_how_many_it_is_stopping() {
        let mut app = app();
        app.git.pretend_running(GitJob::Push);
        execute_command(&mut app, Command::GitCancel);
        assert_eq!(app.notifications.current().unwrap().message, "Cancelling…");

        app.git.pretend_running(GitJob::Pull);
        execute_command(&mut app, Command::GitCancel);
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Cancelling 2 operations…"
        );
    }

    /// A cancelled job is not a failed one: nothing went wrong and the editor
    /// must not say `Push failed:` about a stop the user asked for.
    #[test]
    fn a_cancelled_job_is_reported_as_stopped_and_not_as_broken() {
        let mut app = app();
        app.git.pretend_running(GitJob::Push);
        execute_command(
            &mut app,
            Command::GitJobFinished(JobOutcome {
                id: crate::git::JobId(0),
                job: GitJob::Push,
                result: Err(JobFailure::Cancelled),
            }),
        );

        let said = app.notifications.current().unwrap();
        assert_eq!(said.message, "Push cancelled");
        assert_eq!(
            said.kind,
            crate::app::notifications::NotificationKind::Info,
            "not the error colour"
        );
        assert!(app.git.busy().is_none(), "and the queue drained");
    }

    /// The watcher's whole point: a change made outside the editor is on
    /// screen without anybody pressing `F5` (ADR-040).
    #[test]
    fn a_change_made_outside_the_editor_reaches_the_panel_and_the_tree() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        let (mut app, _rx) = app_over(&repo);
        assert!(app.git.entries().is_empty(), "a clean tree to start from");
        let before = app.sidebar.tree.len();

        // Somebody else's terminal, or a build, or a `git checkout`.
        repo.write("a.txt", "changed\n");
        repo.write("b.txt", "new\n");
        execute_command(
            &mut app,
            Command::ExternalChange(FsChange {
                worktree: true,
                repository: true,
            }),
        );

        assert_eq!(app.git.entries().len(), 2, "{:?}", app.git.entries());
        assert!(app.sidebar.tree.len() > before, "the tree grew a row");
    }

    /// Silent by design: a refresh nobody asked for must not take the status
    /// bar away from the answers to the user's own commands.
    #[test]
    fn an_outside_change_says_nothing_on_the_status_bar() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        app.notifications.info("Saved a.txt");
        execute_command(
            &mut app,
            Command::ExternalChange(FsChange {
                worktree: true,
                repository: true,
            }),
        );
        assert_eq!(
            app.notifications.current().unwrap().message,
            "Saved a.txt",
            "the refresh spoke over the last answer"
        );
    }

    /// Staging in another terminal is a repository change and not a worktree
    /// one, so the explorer is left alone.
    #[test]
    fn an_index_only_change_does_not_rebuild_the_tree() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        app.sidebar.tree.set_show_hidden(true);
        let rows: Vec<_> = app.sidebar.rows().iter().map(|r| r.path.clone()).collect();

        repo.run(&["add", "a.txt"]);
        execute_command(
            &mut app,
            Command::ExternalChange(FsChange {
                worktree: false,
                repository: true,
            }),
        );

        let after: Vec<_> = app.sidebar.rows().iter().map(|r| r.path.clone()).collect();
        assert_eq!(rows, after);
        assert!(
            app.git
                .entries()
                .iter()
                .any(|e| e.path == Path::new("a.txt") && e.index.is_change()),
            "the status was re-read: {:?}",
            app.git.entries()
        );
    }

    #[test]
    fn the_help_screen_opens_on_the_keymap_and_gives_focus_back() {
        let mut app = app();
        app.help_rows = 10;
        app.help_cols = 80;
        execute_command(&mut app, Command::FocusPane(FocusTarget::Explorer));

        execute_command(&mut app, Command::ShowHelp);
        assert_eq!(app.focus, FocusTarget::Help);
        let help = app.help.as_ref().expect("a help screen");
        assert!(!help.sections.is_empty());
        assert_eq!(help.return_focus, FocusTarget::Explorer);

        execute_command(&mut app, Command::HelpClose);
        assert!(app.help.is_none());
        assert_eq!(app.focus, FocusTarget::Explorer, "back where it was opened");
    }

    #[test]
    fn the_help_menu_entry_is_the_help_screen() {
        let mut app = app();
        let menu = MENUS.iter().position(|m| m.title == "Help").unwrap();
        let item = MENUS[menu]
            .items
            .iter()
            .position(|i| i.item().is_some_and(|i| i.label == "Shortcuts"))
            .unwrap();
        execute_command(&mut app, Command::MenuOpen(menu));
        execute_command(&mut app, Command::MenuActivateItem(item));
        assert!(app.help.is_some());
        assert_eq!(app.focus, FocusTarget::Help);
        assert!(app.menu.open.is_none(), "the menu closed under it");
    }

    #[test]
    fn the_help_screen_pages_and_stops_at_both_ends() {
        let mut app = app();
        app.help_rows = 10;
        app.help_cols = 80;
        execute_command(&mut app, Command::ShowHelp);

        execute_command(&mut app, Command::HelpScroll(-1));
        assert_eq!(app.help.as_ref().unwrap().scroll, 0);
        execute_command(&mut app, Command::HelpScrollPage(1));
        assert_eq!(app.help.as_ref().unwrap().scroll, 10);
        execute_command(&mut app, Command::HelpHome);
        assert_eq!(app.help.as_ref().unwrap().scroll, 0);
        execute_command(&mut app, Command::HelpEnd);
        let help = app.help.as_ref().unwrap();
        assert_eq!(help.scroll, help.lines(80).len() - 10);
    }

    /// The screen covers the body, so it cannot outlive its own focus — the
    /// same rule the diff viewer follows (ADR-037, ADR-038).
    #[test]
    fn the_help_screen_closes_when_another_pane_takes_focus() {
        let mut app = app();
        execute_command(&mut app, Command::ShowHelp);
        execute_command(&mut app, Command::FocusPane(FocusTarget::Editor));
        assert!(app.help.is_none());
        assert_eq!(app.focus, FocusTarget::Editor);
    }

    /// A menu and a dialog draw over the screen rather than replacing it, so
    /// neither closes it — again the viewer's rule.
    #[test]
    fn a_menu_over_the_help_screen_leaves_it_open() {
        let mut app = app();
        execute_command(&mut app, Command::ShowHelp);
        execute_command(&mut app, Command::MenuOpen(0));
        assert!(app.help.is_some());
        execute_command(&mut app, Command::MenuClose);
        assert!(app.help.is_some());
        assert_eq!(app.focus, FocusTarget::Help);
    }

    /// Opening the screen from a menu that is itself over the screen must not
    /// leave it returning to itself.
    #[test]
    fn reopening_the_help_screen_from_over_itself_still_closes_to_a_pane() {
        let mut app = app();
        execute_command(&mut app, Command::ShowHelp);
        execute_command(&mut app, Command::MenuOpen(0));
        execute_command(&mut app, Command::ShowHelp);
        execute_command(&mut app, Command::HelpClose);
        assert!(app.help.is_none());
        assert_eq!(app.focus, FocusTarget::Editor);
    }

    #[test]
    fn every_menu_item_runs_without_panicking_from_every_focus() {
        // Phase 9's acceptance, as a test: the menu is wired to commands, and
        // no entry is a hole. The Git four and the help screen report
        // themselves; nothing else may.
        for (menu_index, menu) in MENUS.iter().enumerate() {
            for (item_index, entry) in menu.items.iter().enumerate() {
                // A rule is not an entry: activating one is a no-op that
                // deliberately leaves the menu open (see the test below).
                let Some(item) = entry.item() else { continue };
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

    /// A click on a rule does nothing at all — the menu stays open, so the
    /// user can go on to the entry they were aiming for.
    #[test]
    fn activating_a_rule_leaves_the_menu_open() {
        let (menu_index, rule_index) = MENUS
            .iter()
            .enumerate()
            .find_map(|(m, menu)| {
                menu.items
                    .iter()
                    .position(MenuEntry::is_separator)
                    .map(|i| (m, i))
            })
            .expect("some menu has a rule in it");

        let mut app = App::fixture();
        execute_command(&mut app, Command::MenuOpen(menu_index));
        execute_command(&mut app, Command::MenuActivateItem(rule_index));
        assert_eq!(app.menu.open, Some(menu_index));
    }

    /// The selection steps over the rules rather than landing on one.
    #[test]
    fn stepping_through_a_menu_skips_its_rules() {
        let menu_index = MENUS
            .iter()
            .position(|menu| menu.items.iter().any(MenuEntry::is_separator))
            .expect("some menu has a rule in it");
        let items = MENUS[menu_index].items;

        let mut app = App::fixture();
        execute_command(&mut app, Command::MenuOpen(menu_index));
        // All the way round, in both directions: every stop is an entry.
        for delta in [1, -1] {
            for _ in 0..items.len() + 1 {
                execute_command(&mut app, Command::MenuNextItem);
                if delta < 0 {
                    execute_command(&mut app, Command::MenuPrevItem);
                    execute_command(&mut app, Command::MenuPrevItem);
                }
                assert!(
                    !items[app.menu.item].is_separator(),
                    "the selection landed on a rule at row {}",
                    app.menu.item
                );
            }
        }
    }

    // --- themes (SPEC §43) -------------------------------------------------

    /// Switching themes writes the choice down, and re-selecting the one
    /// already on screen does nothing at all — so walking the View menu does
    /// not rewrite the config file on every pass.
    #[test]
    fn setting_a_theme_records_it_and_says_so() {
        let mut app = App::fixture();
        assert_eq!(app.settings.theme, ThemeKind::Dark);
        assert!(!app.persist_settings, "a test never writes the real config");

        execute_command(&mut app, Command::SetTheme(ThemeKind::Borland));
        assert_eq!(app.settings.theme, ThemeKind::Borland);
        let said = app.notifications.current().unwrap().message.clone();
        assert!(said.contains("Borland"), "{said}");

        execute_command(&mut app, Command::SetTheme(ThemeKind::Borland));
        assert_eq!(
            app.notifications.current().unwrap().message,
            said,
            "the theme already on screen is not news, so nothing was said again"
        );
    }

    // --- the tab strip (SPEC §11) ------------------------------------------

    /// The wheel over the strip moves the window over the tabs and nothing
    /// else: which tab is in front is not the strip's business (ADR-054).
    #[test]
    fn scrolling_the_strip_leaves_the_active_tab_alone() {
        let mut app = App::fixture_with_tabs(12);
        execute_command(&mut app, Command::ScrollTabs(3));
        assert_eq!(app.tab_scroll, 3);
        assert_eq!(app.active_tab, Some(0));

        execute_command(&mut app, Command::ScrollTabs(-99));
        assert_eq!(app.tab_scroll, 0, "there is nothing before the first tab");
        execute_command(&mut app, Command::ScrollTabs(99));
        assert_eq!(app.tab_scroll, 11, "nor anything after the last");
    }

    // --- diff viewer (SPEC §36) -------------------------------------------

    /// Puts the panel's selection on a named row, which is what every diff
    /// test starts from.
    fn select_row(app: &mut App, name: &str) {
        let row = app
            .git
            .entries()
            .iter()
            .position(|entry| entry.path == Path::new(name))
            .unwrap_or_else(|| panic!("{name} is listed: {:?}", app.git.entries()));
        app.git.selected = row;
    }

    fn diff_text(app: &App) -> String {
        app.diff()
            .expect("a viewer is open")
            .diff
            .lines
            .iter()
            .map(|line| format!("{}\n", line.text))
            .collect()
    }

    #[test]
    fn the_panels_selection_opens_its_own_unstaged_diff() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        select_row(&mut app, "a.txt");

        execute_command(&mut app, Command::GitDiff);

        let viewer = app.diff().expect("a viewer is open");
        assert_eq!(viewer.path, Path::new("a.txt"));
        assert_eq!(viewer.side, DiffSide::Worktree);
        assert_eq!(app.focus, FocusTarget::Diff);
        assert!(diff_text(&app).contains("+b"), "{}", diff_text(&app));
        assert!(diff_text(&app).contains("-a"));
    }

    /// A file that is staged and unchanged since has only one diff worth
    /// showing, and it is the staged one.
    #[test]
    fn a_file_with_nothing_unstaged_opens_its_staged_diff_instead() {
        let repo = changed_repo();
        let (mut app, rx) = app_over(&repo);
        select_row(&mut app, "a.txt");
        execute_command(&mut app, Command::GitStage);
        settle(&mut app, &rx);
        select_row(&mut app, "a.txt");

        execute_command(&mut app, Command::GitDiff);
        assert_eq!(app.diff().unwrap().side, DiffSide::Staged);
        assert!(diff_text(&app).contains("+b"));
    }

    #[test]
    fn the_other_side_of_the_same_file_is_one_key_away() {
        let repo = changed_repo();
        let (mut app, rx) = app_over(&repo);
        select_row(&mut app, "a.txt");
        execute_command(&mut app, Command::GitStage);
        settle(&mut app, &rx);
        // Staged `b`, then changed the worktree again: both sides now hold
        // something, and they are not the same thing.
        repo.write("a.txt", "c\n");
        execute_command(&mut app, Command::GitRefresh);
        select_row(&mut app, "a.txt");

        execute_command(&mut app, Command::GitDiff);
        assert_eq!(app.diff().unwrap().side, DiffSide::Worktree);
        assert!(diff_text(&app).contains("+c"));

        execute_command(&mut app, Command::GitDiffToggleSide);
        assert_eq!(app.diff().unwrap().side, DiffSide::Staged);
        assert!(diff_text(&app).contains("+b"));
    }

    /// An untracked file has nothing to be compared with, and an empty pane
    /// would look like a bug rather than an answer.
    #[test]
    fn an_untracked_file_says_why_it_has_no_diff() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        select_row(&mut app, "new.txt");

        execute_command(&mut app, Command::GitDiff);
        assert!(app.diff().is_none());
        let said = app.notifications.current().unwrap().message.clone();
        assert!(said.contains("untracked"), "{said}");
    }

    #[test]
    fn a_file_with_no_changes_opens_no_viewer_and_says_so() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        let (mut app, _rx) = app_over(&repo);
        // Nothing is listed at all, so there is nothing selected either.
        execute_command(&mut app, Command::GitDiff);
        assert!(app.diff().is_none());
        assert!(app
            .notifications
            .current()
            .unwrap()
            .message
            .contains("Nothing to diff"));
    }

    /// Outside the panel the file on screen is what "Diff" means — the Git
    /// menu, pressed mid-edit.
    #[test]
    fn the_editors_own_file_is_what_the_menu_entry_diffs() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        app.open_path(&repo.path().join("a.txt"), None).unwrap();
        assert_eq!(app.focus, FocusTarget::Editor);
        // The panel's selection is deliberately somewhere else.
        select_row(&mut app, "new.txt");

        execute_command(&mut app, Command::GitDiff);
        assert_eq!(app.diff().unwrap().path, Path::new("a.txt"));
    }

    /// A diff is a tab, so focus moving elsewhere leaves it open — and coming
    /// back to it is switching to its tab, not opening it again.
    #[test]
    fn the_diff_tab_outlives_the_focus_that_opened_it() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        select_row(&mut app, "a.txt");
        execute_command(&mut app, Command::GitDiff);
        let tab = app.active_tab.expect("the diff opened a tab");

        execute_command(&mut app, Command::CycleFocus);
        assert!(app.tabs[tab].diff().is_some(), "the tab is still open");
        assert_ne!(app.focus, FocusTarget::Diff);

        execute_command(&mut app, Command::FocusPane(FocusTarget::Editor));
        assert_eq!(
            app.focus,
            FocusTarget::Diff,
            "focusing the pane a diff tab is in focuses the diff"
        );
    }

    /// Opening the same file's diff twice reuses its tab, the way opening the
    /// same file twice reuses its own.
    #[test]
    fn a_second_diff_of_the_same_file_reuses_its_tab() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        select_row(&mut app, "a.txt");
        execute_command(&mut app, Command::GitDiff);
        let tabs = app.tabs.len();

        execute_command(&mut app, Command::GitDiff);
        assert_eq!(app.tabs.len(), tabs);
        // And so does turning it over to the other side.
        execute_command(&mut app, Command::GitDiffToggleSide);
        assert_eq!(app.tabs.len(), tabs);
    }

    #[test]
    fn closing_the_diff_closes_its_tab() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        select_row(&mut app, "a.txt");
        execute_command(&mut app, Command::GitDiff);
        let tabs = app.tabs.len();

        execute_command(&mut app, Command::DiffClose);
        assert!(app.diff().is_none());
        assert_eq!(app.tabs.len(), tabs - 1, "the tab went with it");
    }

    /// A click on a changed file opens its diff in one go, and leaves the
    /// panel focused so the next row is one click away.
    #[test]
    fn clicking_a_changed_file_opens_its_diff() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        let row = app
            .git
            .entries()
            .iter()
            .position(|entry| entry.path == Path::new("a.txt"))
            .expect("a.txt is listed");

        execute_command(&mut app, Command::GitDiffRow(row));
        assert_eq!(
            app.tabs[app.active_tab.unwrap()]
                .diff()
                .expect("a diff tab is in front")
                .path,
            Path::new("a.txt")
        );
        assert_eq!(app.focus, FocusTarget::GitPanel);
    }

    /// Staging what the viewer is showing empties the diff it was showing, so
    /// the viewer closes rather than keeping a change that is no longer there.
    #[test]
    fn staging_the_file_on_screen_closes_the_viewer_it_emptied() {
        let repo = changed_repo();
        let (mut app, rx) = app_over(&repo);
        select_row(&mut app, "a.txt");
        execute_command(&mut app, Command::GitDiff);
        assert!(app.diff().is_some());

        // Focus is on the viewer, so the panel's own keys are not what stages
        // it here; the command is the same one they produce.
        execute_command(&mut app, Command::GitStage);
        settle(&mut app, &rx);
        assert!(app.diff().is_none(), "the unstaged diff is empty now");
    }

    #[test]
    fn an_edit_to_the_file_on_screen_is_picked_up_by_a_refresh() {
        let repo = changed_repo();
        let (mut app, _rx) = app_over(&repo);
        select_row(&mut app, "a.txt");
        execute_command(&mut app, Command::GitDiff);
        assert!(diff_text(&app).contains("+b"));

        repo.write("a.txt", "z\n");
        execute_command(&mut app, Command::DiffRefresh);
        assert!(diff_text(&app).contains("+z"), "{}", diff_text(&app));
    }

    #[test]
    fn the_viewer_scrolls_by_lines_and_by_pages_and_stops_at_the_ends() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("a.txt", "");
        repo.run(&["add", "."]);
        repo.commit("init");
        let body: String = (0..100).map(|i| format!("line {i}\n")).collect();
        repo.write("a.txt", &body);
        let (mut app, _rx) = app_over(&repo);
        app.diff_rows = 10;
        select_row(&mut app, "a.txt");
        execute_command(&mut app, Command::GitDiff);

        execute_command(&mut app, Command::DiffScroll(3));
        assert_eq!(app.diff().unwrap().scroll, 3);
        execute_command(&mut app, Command::DiffScrollPage(1));
        assert_eq!(app.diff().unwrap().scroll, 13);
        execute_command(&mut app, Command::DiffHome);
        assert_eq!(app.diff().unwrap().scroll, 0);
        execute_command(&mut app, Command::DiffEnd);
        let viewer = app.diff().unwrap();
        assert_eq!(viewer.scroll, viewer.diff.len() - 10);
        execute_command(&mut app, Command::DiffScrollHorizontal(1));
        assert_eq!(app.diff().unwrap().h_scroll, HORIZONTAL_STEP);
    }

    #[test]
    fn a_diff_command_outside_a_repository_says_so_and_opens_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::fixture_in(dir.path());
        execute_command(&mut app, Command::GitRefresh);

        execute_command(&mut app, Command::GitDiff);
        assert!(app.diff().is_none());
        assert_eq!(
            app.notifications.current().map(|n| n.message.as_str()),
            Some("Not a Git repository")
        );
    }
}
