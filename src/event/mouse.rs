//! Mouse hit-testing against the layout rects of the last frame.
//!
//! Hit-testing deliberately reads only `LayoutRects` and `&App` and produces a
//! `Command`: mouse input goes through exactly the same mutation path as the
//! keyboard and the menu (SPEC §25).

use std::time::Duration;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::app::focus::FocusTarget;
use crate::app::search::SearchField;
use crate::app::App;
use crate::commands::Command;
use crate::editor::viewport::gutter_width;
use crate::ui::layout::LayoutRects;
use crate::ui::table;

/// Rows the wheel moves per notch, matching the usual terminal convention.
const WHEEL_STEP: i16 = 3;

/// Rows of chrome above the first list row inside a sidebar panel: one border
/// line plus the panel title.
const PANEL_HEADER_ROWS: u16 = 1;

/// How long after a click a second one on the same cell is a double-click.
///
/// Terminals report presses, not double clicks, so the pairing is ours to do.
/// 400 ms is the usual desktop default and is forgiving enough over a slow SSH
/// link, where the two presses arrive further apart than they were made.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

pub fn hit_test(app: &App, rects: &LayoutRects, event: MouseEvent) -> Option<Command> {
    if rects.too_small {
        return None;
    }
    let at = Position::new(event.column, event.row);

    // A dialog is modal: it takes a click on one of its buttons and swallows
    // everything else, the wheel included (SPEC §40).
    if rects.dialog.is_some() {
        return dialog_click(app, rects, event.kind, at);
    }

    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => left_click(app, rects, at),
        MouseEventKind::Down(MouseButton::Middle) => middle_click(rects, at),
        MouseEventKind::Drag(MouseButton::Left) => drag(app, rects, at),
        // Shift turns the wheel sideways, which is the gesture a terminal
        // without a horizontal wheel has; a terminal that does have one sends
        // `ScrollLeft`/`ScrollRight` itself and both arrive here.
        MouseEventKind::ScrollDown if event.modifiers.contains(KeyModifiers::SHIFT) => {
            scroll_sideways(rects, at, 1)
        }
        MouseEventKind::ScrollUp if event.modifiers.contains(KeyModifiers::SHIFT) => {
            scroll_sideways(rects, at, -1)
        }
        MouseEventKind::ScrollDown => scroll(rects, at, WHEEL_STEP),
        MouseEventKind::ScrollUp => scroll(rects, at, -WHEEL_STEP),
        MouseEventKind::ScrollRight => scroll_sideways(rects, at, 1),
        MouseEventKind::ScrollLeft => scroll_sideways(rects, at, -1),
        _ => None,
    }
}

fn dialog_click(
    app: &App,
    rects: &LayoutRects,
    kind: MouseEventKind,
    at: Position,
) -> Option<Command> {
    // The wheel over a list body moves the selection, which is what scrolls it
    // (ADR-051). Moving the highlight rather than the window alone is what the
    // keyboard already does, and a picker whose selection scrolls out of sight
    // would need a second rule for what Enter then means.
    let over_list = |rect: Option<Rect>| rect.is_some_and(|list| list.contains(at));
    if matches!(kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
        if !over_list(rects.dialog_list) && !over_list(rects.dialog_list_frame) {
            return None;
        }
        let delta = if kind == MouseEventKind::ScrollUp {
            -WHEEL_STEP
        } else {
            WHEEL_STEP
        };
        return Some(Command::DialogListMove(delta));
    }
    if kind != MouseEventKind::Down(MouseButton::Left) {
        return None;
    }
    if let Some(index) = rects
        .dialog_buttons
        .iter()
        .position(|rect| rect.contains(at))
    {
        return Some(Command::DialogActivateButton(index));
    }
    // A click on a list row selects it rather than acting on it: unlike the
    // explorer's rows (ADR-020), a picker's confirm button is right there and
    // choosing a branch by accident is a checkout.
    let list = rects.dialog_list?;
    if !list.contains(at) {
        return None;
    }
    let row = (at.y - list.y) as usize;
    // The browser is the one list where a second click on the row already
    // selected opens it — walking into a directory is what browsing *is*, and
    // a file manager where every step costs a click and a button press is not
    // the convenience this dialog exists to be. It is the same "click twice"
    // as a double-click without the timing: the first click still only selects,
    // so nothing opens by surprise (ADR-051).
    let already = app
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.browser())
        .is_some_and(|browser| browser.selected() == browser.scroll() + row);
    Some(if already {
        Command::BrowserOpen
    } else {
        Command::DialogSelectItem(row)
    })
}

/// Middle click closes the tab under the pointer (SPEC §11).
///
/// Not every terminal reports the middle button, which is why it is the third
/// way to close a tab rather than the only one: `Ctrl+W` and the tab's own `×`
/// are always there.
fn middle_click(rects: &LayoutRects, at: Position) -> Option<Command> {
    tab_at(rects, at).map(Command::CloseTabAt)
}

/// The tab under a point. Tabs scrolled out of the bar have zero-width rects,
/// which `contains` already rejects.
fn tab_at(rects: &LayoutRects, at: Position) -> Option<usize> {
    rects.tabs.iter().position(|rect| rect.contains(at))
}

fn left_click(app: &App, rects: &LayoutRects, at: Position) -> Option<Command> {
    // An open menu swallows the click: either it lands on an item, or it
    // dismisses the menu. Nothing behind the popup is reachable.
    if let Some(popup) = rects.menu_popup {
        if let Some(index) = menu_title_at(rects, at) {
            return Some(if app.menu.open == Some(index) {
                Command::MenuClose
            } else {
                Command::MenuOpen(index)
            });
        }
        if popup.contains(at) {
            // Skip the popup's top border to reach the first item.
            let item = at.y.saturating_sub(popup.y + 1) as usize;
            return Some(Command::MenuActivateItem(item));
        }
        return Some(Command::MenuClose);
    }

    if let Some(index) = menu_title_at(rects, at) {
        return Some(Command::MenuOpen(index));
    }
    // The help screen covers the body — tabs, sidebar and editor alike — so it
    // is tested before any of them. The menu bar above it is still reachable,
    // which is how the screen is closed with the mouse: opening a menu moves
    // focus, and the screen closes with it (ADR-038).
    if rects.help.is_some_and(|help| help.contains(at)) {
        return Some(Command::FocusPane(FocusTarget::Help));
    }
    // The overflow arrows sit at the two ends of the strip, outside every tab
    // rect, and each scrolls the strip one tab towards the tabs it points at.
    if rects.tab_overflow_left.is_some_and(|r| r.contains(at)) {
        return Some(Command::ScrollTabs(-1));
    }
    if rects.tab_overflow_right.is_some_and(|r| r.contains(at)) {
        return Some(Command::ScrollTabs(1));
    }
    // The close button is inside the tab, so it has to be tested first or a
    // click on the × would only select the tab it is trying to close.
    if let Some(index) = rects.tab_closes.iter().position(|r| r.contains(at)) {
        return Some(Command::CloseTabAt(index));
    }
    if let Some(index) = tab_at(rects, at) {
        return Some(Command::SelectTab(index));
    }
    if let Some(command) = search_click(rects, at) {
        return Some(command);
    }
    // The viewer covers the editor pane, so it is tested first: a click on a
    // diff must not place a cursor in the document behind it.
    if rects.diff.is_some_and(|diff| diff.contains(at)) {
        return Some(Command::FocusPane(FocusTarget::Diff));
    }
    // The table covers the editor pane the same way, and for the same reason
    // has to be tested before it: a click on a cell must not put a caret in the
    // text behind it (SPEC §65).
    if let Some(area) = rects.table.filter(|table| table.contains(at)) {
        if let Some((row, column)) = table::cell_at(app, area, at) {
            // A click on a column's name takes the whole column, which is what
            // it does in every grid — and leaves the cursor on the name, so
            // renaming it is still one key away (SPEC §65).
            return Some(match row {
                Some(row) => Command::SelectCell {
                    row: Some(row),
                    column,
                },
                None => Command::SelectColumnAt { column },
            });
        }
        // A click on a record's number takes the whole record, which is the
        // gesture a grid has for selecting a row (SPEC §65).
        if let Some(row) = table::record_at(app, area, at) {
            return Some(Command::SelectRowAt { row });
        }
        // What is left is chrome — the rule, and the blank above the numbers —
        // and a click on it focuses the pane and selects nothing.
        return Some(Command::FocusPane(FocusTarget::Editor));
    }
    if rects.editor.contains(at) {
        return Some(editor_click(app, rects.editor, at));
    }
    // The bar is not draggable — there is no cell of it that means a document
    // position — so a click on it does the one thing a click on a pane always
    // does.
    if rects.editor_scrollbar.contains(at) {
        return Some(Command::FocusPane(FocusTarget::Editor));
    }
    if rects.explorer.contains(at) {
        return Some(explorer_click(app, rects.explorer, at));
    }
    if rects.git_panel.contains(at) {
        return Some(git_click(app, rects.git_panel, at));
    }
    // The readout on the status bar is a row of questions and statements
    // (ADR-058, SPEC §65). A click on one of the questions opens the dialog that
    // answers it; the rest of the row — the notification, the focus label, a
    // table's row count — is not a control and stays inert.
    if rects.status_bar.contains(at) {
        return rects
            .status_zones
            .iter()
            .find(|(_, rect)| rect.contains(at))
            .map(|(zone, _)| zone.command());
    }
    None
}

/// A click on the find/replace bar.
///
/// Every clickable piece is a rect `ui/layout.rs` computed and `ui/search.rs`
/// drew from, so the `[Aa]` on screen and the `[Aa]` a click lands on cannot be
/// in two different places. A click anywhere else on the bar puts the caret in
/// the row's field, which is what a click on a form does.
fn search_click(rects: &LayoutRects, at: Position) -> Option<Command> {
    let search = rects.search.as_ref()?;
    if !search.bar.contains(at) {
        return None;
    }
    if search.case_toggle.contains(at) {
        return Some(Command::SearchToggleCase);
    }
    if search.replace_button.is_some_and(|r| r.contains(at)) {
        return Some(Command::ReplaceCurrent);
    }
    if search.replace_all_button.is_some_and(|r| r.contains(at)) {
        return Some(Command::ReplaceAll);
    }
    let field = match search.replacement {
        Some(row) if row.y == at.y => SearchField::Replacement,
        _ => SearchField::Query,
    };
    Some(Command::SearchFocusField(field))
}

/// Turns a click in the editor pane into a document position.
///
/// The gutter is measured with the same function the renderer uses, so the
/// character under the pointer is the one the cursor lands on. A click on the
/// gutter itself lands at column 0 of that line, which is the usual way to
/// select a line's start.
///
/// A second click on the same cell inside the double-click window selects the
/// word there instead (SPEC §27).
fn editor_click(app: &App, editor: Rect, at: Position) -> Command {
    let Some((line, col)) = document_position(app, editor, at) else {
        return Command::FocusPane(FocusTarget::Editor);
    };
    let double = app.last_click.is_some_and(|last| {
        last.line == line && last.col == col && last.at.elapsed() < DOUBLE_CLICK
    });
    if double {
        Command::SelectWordAt { line, col }
    } else {
        Command::PlaceCursor { line, col }
    }
}

/// Dragging with the left button held extends the selection to the pointer.
///
/// The position is clamped into the editor rect rather than discarded when the
/// pointer leaves it: a drag that runs off the edge keeps selecting to the edge,
/// which is what the same gesture does everywhere else.
fn drag(app: &App, rects: &LayoutRects, at: Position) -> Option<Command> {
    if rects.menu_popup.is_some() || app.focus != FocusTarget::Editor {
        return None;
    }
    // Over a table the drag makes a block of cells rather than a run of
    // characters, and it is clamped into the pane for the same reason: a drag
    // that runs off the edge keeps selecting to the edge (SPEC §65).
    if let Some(area) = rects.table {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let clamped = Position::new(
            at.x.clamp(area.x, area.right() - 1),
            at.y.clamp(area.y, area.bottom() - 1),
        );
        let (row, column) = table::cell_at(app, area, clamped)?;
        return Some(Command::ExtendCellTo { row, column });
    }
    let editor = rects.editor;
    if editor.width == 0 || editor.height == 0 {
        return None;
    }
    let clamped = Position::new(
        at.x.clamp(editor.x, editor.right() - 1),
        at.y.clamp(editor.y, editor.bottom() - 1),
    );
    let (line, col) = document_position(app, editor, clamped)?;
    Some(Command::ExtendCursorTo { line, col })
}

/// The document line and display column under a point in the editor pane.
///
/// `None` when no file is open, which is the one case where a click in the
/// editor means nothing more than "focus me".
fn document_position(app: &App, editor: Rect, at: Position) -> Option<(usize, usize)> {
    let tab = app.active()?;
    let gutter = gutter_width(tab.document.line_count()) as u16;
    let row = at.y.saturating_sub(editor.y) as usize;
    let cell = at.x.saturating_sub(editor.x + gutter) as usize;
    // The rows the pane drew, which is the only thing that knows where a
    // wrapped line was broken. A point below the last of them belongs to that
    // last row: a drag off the bottom of a short file selects to the end of
    // it rather than to a line that is not there.
    let layout = crate::ui::editor::pane_layout(app, editor, tab.document.line_count());
    let rows = tab.viewport.visible(&tab.document, layout);
    let drawn = rows.get(row).or_else(|| rows.last())?;
    let col = drawn.row.start_col.0 + cell + tab.viewport.left_col.0;
    Some((drawn.line, col))
}

/// A click in the explorer selects the row under the pointer and acts on it:
/// a file opens, a directory folds (SPEC §27).
///
/// Unlike the git panel, the explorer does not spend the first click on taking
/// focus. Clicking a file in a tree is how a file is opened, and making that
/// two clicks when the editor happens to be focused would be a rule the user
/// has to keep in their head.
fn explorer_click(app: &App, panel: Rect, at: Position) -> Command {
    let row = at.y.saturating_sub(panel.y + PANEL_HEADER_ROWS) as usize;
    Command::ExplorerActivateRow(row + app.sidebar.scroll)
}

/// A click on a changed file shows its diff (SPEC §36).
///
/// One click, like the explorer's: the panel is a list of changes, and what a
/// change is *for* is being read. It does not spend the first click on taking
/// focus either — `open_diff` moves focus to the diff tab it opens, so a click
/// that only focused the panel would be a click the user has to repeat.
///
/// A panel with no rows in it — no repository, or a clean tree — has nothing
/// to open, so a click there does the one thing a click on a pane always does.
fn git_click(app: &App, panel: Rect, at: Position) -> Command {
    if app.git.entries().is_empty() {
        return Command::FocusPane(FocusTarget::GitPanel);
    }
    let row = at.y.saturating_sub(panel.y + PANEL_HEADER_ROWS) as usize;
    Command::GitDiffRow(row + app.git.scroll)
}

fn scroll(rects: &LayoutRects, at: Position, delta: i16) -> Option<Command> {
    if rects.help.is_some_and(|help| help.contains(at)) {
        return Some(Command::HelpScroll(delta));
    }
    if rects.diff.is_some_and(|diff| diff.contains(at)) {
        return Some(Command::DiffScroll(delta));
    }
    if rects.table.is_some_and(|table| table.contains(at)) {
        return Some(Command::ScrollTable(delta));
    }
    // The wheel over the tab strip scrolls the strip, not the document under
    // it: a bar with more tabs on it than fit is the one place where the thing
    // the pointer is over has a sideways axis of its own.
    if rects.tab_bar.contains(at) {
        return Some(Command::ScrollTabs(delta));
    }
    // The scrollbar column counts as the editor: a wheel on the bar is a wheel
    // on the thing it scrolls, the same way the browser's frame works.
    if rects.editor.contains(at) || rects.editor_scrollbar.contains(at) {
        return Some(Command::ScrollEditor(delta));
    }
    if rects.explorer.contains(at) || rects.git_panel.contains(at) {
        return Some(Command::ScrollSidebar(delta));
    }
    None
}

/// The wheel's sideways axis: one step of the window per notch, over whichever
/// pane is under the pointer.
///
/// One step and not the wheel's three: a horizontal step is already eight
/// columns, and a notch that moved the text by two dozen of them would lose
/// the reader's place on every flick of a trackpad.
fn scroll_sideways(rects: &LayoutRects, at: Position, delta: i16) -> Option<Command> {
    if rects.diff.is_some_and(|diff| diff.contains(at)) {
        return Some(Command::DiffScrollHorizontal(delta));
    }
    if rects.table.is_some_and(|table| table.contains(at)) {
        return Some(Command::ScrollTableHorizontal(delta));
    }
    if rects.editor.contains(at) || rects.editor_scrollbar.contains(at) {
        return Some(Command::ScrollEditorHorizontal(delta));
    }
    None
}

fn menu_title_at(rects: &LayoutRects, at: Position) -> Option<usize> {
    if !rects.menu_bar.contains(at) {
        return None;
    }
    // Zero-width rects are titles clipped off the right edge; `contains` already
    // rejects them, so no extra guard is needed here.
    rects.menu_titles.iter().position(|r| r.contains(at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::coords::VisualCol;
    use crate::ui::layout;
    use crate::ui::statusbar::StatusZone;

    fn app() -> App {
        App::fixture()
    }

    /// The scrollbar column is part of the editor as far as the mouse is
    /// concerned (ADR-052): a wheel over it scrolls the document, and a click
    /// focuses the pane instead of falling through to the panel beside it.
    #[test]
    fn the_editor_scrollbar_column_belongs_to_the_editor() {
        let app = app();
        let r = rects(&app);
        let bar = r.editor_scrollbar;
        assert_eq!(bar.width, 1);
        assert_eq!(bar.x, r.editor.right(), "it sits against the text");
        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(MouseEventKind::ScrollDown, bar.x, bar.y + 1)
            ),
            Some(Command::ScrollEditor(WHEEL_STEP))
        );
        assert_eq!(
            hit_test(&app, &r, click(bar.x, bar.y + 1)),
            Some(Command::FocusPane(FocusTarget::Editor))
        );
    }

    fn rects(app: &App) -> LayoutRects {
        layout::compute(Rect::new(0, 0, 100, 30), app)
    }

    fn click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn wheel(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn clicking_a_tab_selects_it() {
        let app = app();
        let r = rects(&app);
        let third = r.tabs[2];
        assert_eq!(
            hit_test(&app, &r, click(third.x, third.y)),
            Some(Command::SelectTab(2))
        );
    }

    fn middle_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Middle),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn clicking_a_tabs_close_button_closes_it_rather_than_selecting_it() {
        let app = app();
        let r = rects(&app);
        let close = r.tab_closes[1];
        assert_eq!(close.width, 1, "every whole tab has a close button");
        assert_eq!(
            hit_test(&app, &r, click(close.x, close.y)),
            Some(Command::CloseTabAt(1))
        );
    }

    #[test]
    fn middle_clicking_a_tab_closes_it() {
        let app = app();
        let r = rects(&app);
        let second = r.tabs[1];
        assert_eq!(
            hit_test(&app, &r, middle_click(second.x, second.y)),
            Some(Command::CloseTabAt(1))
        );
        // Nowhere else acts on the middle button.
        assert_eq!(
            hit_test(&app, &r, middle_click(r.editor.x + 2, r.editor.y + 1)),
            None
        );
    }

    #[test]
    fn a_tab_scrolled_off_the_bar_cannot_be_clicked() {
        let app = App::fixture_with_tabs(12);
        let r = layout::compute(Rect::new(0, 0, 60, 30), &app);
        let hidden = r.tabs[11];
        assert_eq!(hidden.width, 0);
        assert_eq!(
            hit_test(&app, &r, click(hidden.x, hidden.y)),
            Some(Command::SelectTab(0)),
            "the point belongs to whichever tab is actually drawn there"
        );
    }

    #[test]
    fn an_open_dialog_takes_its_buttons_and_swallows_everything_else() {
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::unsaved_changes(
            0,
            "main.rs",
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        let r = rects(&app);

        let cancel = r.dialog_buttons[2];
        assert_eq!(
            hit_test(&app, &r, click(cancel.x, cancel.y)),
            Some(Command::DialogActivateButton(2))
        );
        for event in [
            click(r.editor.x + 2, r.editor.y + 1),
            click(r.tabs[1].x, r.tabs[1].y),
            middle_click(r.tabs[1].x, r.tabs[1].y),
            wheel(MouseEventKind::ScrollDown, r.editor.x + 1, r.editor.y + 1),
        ] {
            assert_eq!(
                hit_test(&app, &r, event),
                None,
                "{:?} reached behind the dialog",
                event.kind
            );
        }
    }

    /// The browser is the one dialog list you can be looking for something in,
    /// so the wheel has to work over it (ADR-051).
    #[test]
    fn the_wheel_scrolls_the_browser_and_still_nothing_else_behind_it() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..40 {
            std::fs::write(dir.path().join(format!("f{i:02}.txt")), "").unwrap();
        }
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::browse(
            dir.path(),
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        let r = rects(&app);
        let list = r.dialog_list.expect("a list area");

        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(MouseEventKind::ScrollDown, list.x + 2, list.y + 1)
            ),
            Some(Command::DialogListMove(WHEEL_STEP))
        );
        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(MouseEventKind::ScrollUp, list.x + 2, list.y + 1)
            ),
            Some(Command::DialogListMove(-WHEEL_STEP))
        );
        // The frame counts as the list: a wheel on the scrollbar is a wheel on
        // the thing it scrolls.
        let outline = r.dialog_list_frame.expect("a frame");
        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(
                    MouseEventKind::ScrollDown,
                    outline.right() - 1,
                    outline.y + 2
                )
            ),
            Some(Command::DialogListMove(WHEEL_STEP))
        );
        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(MouseEventKind::ScrollDown, r.editor.x + 1, r.editor.y + 1)
            ),
            None,
            "and the editor is still behind a modal window"
        );
    }

    #[test]
    fn a_second_click_on_the_selected_browser_row_opens_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::browse(
            dir.path(),
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        let r = rects(&app);
        let list = r.dialog_list.expect("a list area");

        // The selection starts on row 0, so row 1 only selects.
        assert_eq!(
            hit_test(&app, &r, click(list.x + 2, list.y + 1)),
            Some(Command::DialogSelectItem(1))
        );
        assert_eq!(
            hit_test(&app, &r, click(list.x + 2, list.y)),
            Some(Command::BrowserOpen),
            "the row already selected is the one a click opens"
        );
    }

    #[test]
    fn clicking_a_menu_title_opens_that_menu() {
        let app = app();
        let r = rects(&app);
        let edit = r.menu_titles[1];
        assert_eq!(
            hit_test(&app, &r, click(edit.x, edit.y)),
            Some(Command::MenuOpen(1))
        );
    }

    #[test]
    fn clicking_an_open_popup_activates_the_row_under_the_cursor() {
        let mut app = app();
        app.menu.open = Some(0);
        app.focus = FocusTarget::Menu;
        let r = rects(&app);
        let popup = r.menu_popup.unwrap();
        // Second item: one row of border, then item 0, then item 1.
        assert_eq!(
            hit_test(&app, &r, click(popup.x + 1, popup.y + 2)),
            Some(Command::MenuActivateItem(1))
        );
    }

    #[test]
    fn clicking_away_from_an_open_popup_closes_it() {
        let mut app = app();
        app.menu.open = Some(0);
        app.focus = FocusTarget::Menu;
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, click(r.editor.x + 5, r.editor.bottom() - 1)),
            Some(Command::MenuClose)
        );
    }

    /// A click on a changed file opens that file's diff, whatever had focus:
    /// the panel is a list of changes and reading one is what it is for
    /// (SPEC §36).
    #[test]
    fn clicking_a_changed_file_asks_for_its_diff() {
        let app = app();
        let r = rects(&app);
        assert_eq!(app.focus, FocusTarget::Editor);
        assert_eq!(
            hit_test(&app, &r, click(r.git_panel.x + 2, r.git_panel.y + 2)),
            Some(Command::GitDiffRow(1))
        );
    }

    /// A panel with no rows in it — no repository, or a clean tree — has
    /// nothing to open, so the click does what a click on a pane always does.
    #[test]
    fn clicking_an_empty_git_panel_only_focuses_it() {
        let mut app = app();
        app.git = crate::app::git::GitState::default();
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, click(r.git_panel.x + 2, r.git_panel.y + 2)),
            Some(Command::FocusPane(FocusTarget::GitPanel))
        );
    }

    /// A scrolled panel opens the row the offset points at, not the second
    /// one drawn.
    #[test]
    fn a_scrolled_git_panel_opens_the_row_the_offset_points_at() {
        let mut app = app();
        app.git.scroll = 3;
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, click(r.git_panel.x + 2, r.git_panel.y + 2)),
            Some(Command::GitDiffRow(4))
        );
    }

    /// The explorer is the exception: a click there acts on the row rather than
    /// spending itself on taking focus, because clicking a file in a tree is
    /// how a file is opened (SPEC §27).
    #[test]
    fn clicking_the_explorer_acts_on_the_row_under_the_cursor() {
        let app = app();
        assert_eq!(app.focus, FocusTarget::Editor);
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, click(r.explorer.x + 2, r.explorer.y + 3)),
            Some(Command::ExplorerActivateRow(2))
        );
    }

    #[test]
    fn a_scrolled_explorer_acts_on_the_row_the_offset_points_at() {
        let mut app = app();
        app.focus = FocusTarget::Explorer;
        app.sidebar.scroll = 2;
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, click(r.explorer.x + 2, r.explorer.y + 3)),
            Some(Command::ExplorerActivateRow(4))
        );
    }

    #[test]
    fn clicking_in_the_editor_places_the_cursor_under_the_pointer() {
        let app = app();
        let r = rects(&app);
        // Four cells of gutter, then the fifth column of the second line.
        let command = hit_test(&app, &r, click(r.editor.x + 4 + 4, r.editor.y + 1));
        assert_eq!(command, Some(Command::PlaceCursor { line: 1, col: 4 }));
    }

    #[test]
    fn clicking_the_gutter_places_the_cursor_at_the_start_of_the_line() {
        let app = app();
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, click(r.editor.x, r.editor.y + 2)),
            Some(Command::PlaceCursor { line: 2, col: 0 })
        );
    }

    #[test]
    fn a_scrolled_editor_clicks_the_line_the_offset_points_at() {
        let mut app = app();
        app.active_mut().unwrap().viewport.top_line = 1;
        app.active_mut().unwrap().viewport.left_col = VisualCol(10);
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, click(r.editor.x + 4 + 1, r.editor.y + 1)),
            Some(Command::PlaceCursor { line: 2, col: 11 })
        );
    }

    /// Sideways over the editor: the wheel with `Shift` held, and the
    /// horizontal wheel a terminal sends on its own. Both land on the same
    /// command, and the diff viewer gets its own.
    #[test]
    fn the_wheel_moves_the_editor_sideways() {
        let app = app();
        let r = rects(&app);
        let mut shifted = wheel(MouseEventKind::ScrollDown, r.editor.x + 1, r.editor.y + 1);
        shifted.modifiers = crossterm::event::KeyModifiers::SHIFT;
        assert_eq!(
            hit_test(&app, &r, shifted),
            Some(Command::ScrollEditorHorizontal(1))
        );
        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(MouseEventKind::ScrollLeft, r.editor.x + 1, r.editor.y + 1)
            ),
            Some(Command::ScrollEditorHorizontal(-1))
        );
        // Unshifted, the same wheel is still the vertical one.
        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(MouseEventKind::ScrollDown, r.editor.x + 1, r.editor.y + 1)
            ),
            Some(Command::ScrollEditor(WHEEL_STEP))
        );
    }

    /// With wrapping on, a screen row is a row of a line and not a line of its
    /// own: clicking the second row of a wrapped line has to land in that line,
    /// at the column that row starts on.
    #[test]
    fn clicking_a_wrapped_row_lands_in_the_line_it_belongs_to() {
        let mut app = app();
        app.settings.word_wrap = true;
        app.tabs[0] =
            crate::app::TabItem::editing(crate::app::Tab::scratch("prose.txt", &"ab ".repeat(60)));
        app.active_tab = Some(0);
        let r = rects(&app);
        // The second row starts at the last word boundary that fits, so its
        // first cell is that column and the third cell is two past it.
        let text_width = (r.editor.width - 4) as usize;
        let second_row = text_width - text_width % 3;
        assert_eq!(
            hit_test(&app, &r, click(r.editor.x + 4 + 2, r.editor.y + 1)),
            Some(Command::PlaceCursor {
                line: 0,
                col: second_row + 2,
            })
        );
    }

    #[test]
    fn clicking_an_editor_with_no_open_file_only_focuses_it() {
        let app = App::new(crate::app::workspace::Workspace::from_arg(None).unwrap());
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, click(r.editor.x + 2, r.editor.y + 1)),
            Some(Command::FocusPane(FocusTarget::Editor))
        );
    }

    fn drag_event(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn dragging_in_the_editor_extends_the_selection() {
        let app = app();
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, drag_event(r.editor.x + 4 + 6, r.editor.y + 1)),
            Some(Command::ExtendCursorTo { line: 1, col: 6 })
        );
    }

    #[test]
    fn a_drag_that_leaves_the_pane_keeps_selecting_to_its_edge() {
        let app = app();
        let r = rects(&app);
        // Off the bottom-left of the editor: the selection follows to the last
        // row the pane actually drew — the end of a three-line file, and not a
        // line number the pane's height happens to reach.
        let command = hit_test(&app, &r, drag_event(0, r.editor.bottom() + 5));
        assert_eq!(command, Some(Command::ExtendCursorTo { line: 2, col: 0 }));
    }

    #[test]
    fn dragging_outside_the_editor_focus_selects_nothing() {
        let mut app = app();
        app.focus = FocusTarget::Explorer;
        let r = rects(&app);
        assert_eq!(
            hit_test(&app, &r, drag_event(r.editor.x + 5, r.editor.y + 1)),
            None
        );
    }

    #[test]
    fn a_second_click_on_the_same_cell_selects_the_word() {
        let mut app = app();
        let r = rects(&app);
        let at = click(r.editor.x + 4 + 4, r.editor.y);
        assert_eq!(
            hit_test(&app, &r, at),
            Some(Command::PlaceCursor { line: 0, col: 4 })
        );
        app.last_click = Some(crate::app::LastClick {
            at: std::time::Instant::now(),
            line: 0,
            col: 4,
        });
        assert_eq!(
            hit_test(&app, &r, at),
            Some(Command::SelectWordAt { line: 0, col: 4 })
        );
    }

    #[test]
    fn a_click_elsewhere_or_long_ago_is_not_a_double_click() {
        let mut app = app();
        let r = rects(&app);
        app.last_click = Some(crate::app::LastClick {
            at: std::time::Instant::now(),
            line: 0,
            col: 9,
        });
        assert_eq!(
            hit_test(&app, &r, click(r.editor.x + 4 + 4, r.editor.y)),
            Some(Command::PlaceCursor { line: 0, col: 4 }),
            "a different cell is a fresh click"
        );

        app.last_click = Some(crate::app::LastClick {
            at: std::time::Instant::now() - DOUBLE_CLICK - Duration::from_millis(1),
            line: 0,
            col: 4,
        });
        assert_eq!(
            hit_test(&app, &r, click(r.editor.x + 4 + 4, r.editor.y)),
            Some(Command::PlaceCursor { line: 0, col: 4 }),
            "and so is one after the window has passed"
        );
    }

    #[test]
    fn the_wheel_scrolls_whichever_pane_is_under_the_pointer() {
        let app = app();
        let r = rects(&app);
        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(MouseEventKind::ScrollDown, r.editor.x + 1, r.editor.y + 1)
            ),
            Some(Command::ScrollEditor(WHEEL_STEP))
        );
        assert_eq!(
            hit_test(
                &app,
                &r,
                wheel(MouseEventKind::ScrollUp, r.explorer.x + 1, r.explorer.y + 1)
            ),
            Some(Command::ScrollSidebar(-WHEEL_STEP))
        );
    }

    #[test]
    fn clicks_do_nothing_while_the_terminal_is_too_small() {
        let app = app();
        let r = layout::compute(Rect::new(0, 0, 10, 5), &app);
        assert_eq!(hit_test(&app, &r, click(2, 2)), None);
    }

    #[test]
    fn a_click_on_the_left_half_of_the_status_bar_is_not_a_command() {
        let app = app();
        let r = rects(&app);
        assert_eq!(hit_test(&app, &r, click(1, r.status_bar.y)), None);
    }

    /// Every readout with an unambiguous dialog behind it opens that dialog
    /// (ADR-058, ADR-059). The rects come from `ui::statusbar` itself, so this
    /// is also the test that the pieces are where they were drawn.
    #[test]
    fn the_readouts_with_a_dialog_behind_them_are_clickable() {
        let app = app();
        let r = rects(&app);
        let row = r.status_bar.y;
        for (zone, rect) in &r.status_zones {
            assert_eq!(
                hit_test(&app, &r, click(rect.x, row)),
                Some(zone.command()),
                "{zone:?} at {rect:?}"
            );
        }
        let zones: Vec<StatusZone> = r.status_zones.iter().map(|(zone, _)| *zone).collect();
        assert_eq!(
            zones,
            vec![
                StatusZone::Cursor,
                StatusZone::Encoding,
                StatusZone::LineEnding,
                StatusZone::Language,
                StatusZone::Branch,
            ]
        );
    }

    /// The gap between two pieces is not either of them: a click there is a
    /// click on the bar, which does nothing.
    #[test]
    fn the_space_between_two_readouts_is_not_a_command() {
        let app = app();
        let r = rects(&app);
        let (_, cursor) = r.status_zones[0];
        assert_eq!(
            hit_test(&app, &r, click(cursor.right(), r.status_bar.y)),
            None
        );
    }
    /// The fixture with the replace bar open, which is the case with every
    /// clickable piece on screen at once.
    fn replacing() -> App {
        let mut app = app();
        crate::commands::execute::execute_command(&mut app, Command::ReplaceOpen);
        app
    }

    /// The centre of a rect, which is where a user aims.
    fn centre(rect: Rect) -> (u16, u16) {
        (rect.x + rect.width / 2, rect.y)
    }

    #[test]
    fn the_bars_buttons_do_what_they_say() {
        let app = replacing();
        let rects = rects(&app);
        let search = rects.search.as_ref().expect("the bar is open");

        let (x, y) = centre(search.case_toggle);
        assert_eq!(
            hit_test(&app, &rects, click(x, y)),
            Some(Command::SearchToggleCase)
        );
        let (x, y) = centre(search.replace_button.unwrap());
        assert_eq!(
            hit_test(&app, &rects, click(x, y)),
            Some(Command::ReplaceCurrent)
        );
        let (x, y) = centre(search.replace_all_button.unwrap());
        assert_eq!(
            hit_test(&app, &rects, click(x, y)),
            Some(Command::ReplaceAll)
        );
    }

    #[test]
    fn clicking_a_row_of_the_bar_puts_the_caret_in_that_rows_field() {
        let app = replacing();
        let rects = rects(&app);
        let search = rects.search.as_ref().unwrap();

        let (x, y) = centre(search.query);
        assert_eq!(
            hit_test(&app, &rects, click(x, y)),
            Some(Command::SearchFocusField(SearchField::Query))
        );
        let (x, y) = centre(search.replacement.unwrap());
        assert_eq!(
            hit_test(&app, &rects, click(x, y)),
            Some(Command::SearchFocusField(SearchField::Replacement))
        );
    }

    #[test]
    fn a_click_below_the_editor_does_not_reach_the_document_through_the_bar() {
        let app = replacing();
        let rects = rects(&app);
        let bar = rects.search.as_ref().unwrap().bar;
        // The bar sits where the editor's last rows used to be; a click there
        // must not place a cursor in the document behind it.
        let command = hit_test(&app, &rects, click(bar.x + 1, bar.y));
        assert!(
            matches!(command, Some(Command::SearchFocusField(_))),
            "got {command:?}"
        );
    }

    /// The viewer covers the editor, so the pointer must find it first: a
    /// click on a diff line is not a place to put a cursor (ADR-037).
    fn app_with_diff() -> App {
        use crate::app::diff::DiffState;
        use crate::git::diff::{Diff, DiffSide};

        let mut app = app();
        app.tabs.push(crate::app::TabItem::Diff(DiffState::new(
            std::path::Path::new("src/main.rs"),
            DiffSide::Worktree,
            Diff::parse("@@ -1 +1 @@\n-a\n+b\n"),
        )));
        app.active_tab = Some(app.tabs.len() - 1);
        app.focus = FocusTarget::Diff;
        app
    }

    #[test]
    fn clicking_the_open_viewer_focuses_it_instead_of_the_document_behind_it() {
        let app = app_with_diff();
        let rects = rects(&app);
        let (x, y) = centre(rects.diff.expect("the viewer is open"));
        assert_eq!(
            hit_test(&app, &rects, click(x, y)),
            Some(Command::FocusPane(FocusTarget::Diff))
        );
    }

    #[test]
    fn the_wheel_over_the_viewer_scrolls_the_diff_and_not_the_editor() {
        let app = app_with_diff();
        let rects = rects(&app);
        let (x, y) = centre(rects.diff.expect("the viewer is open"));
        assert_eq!(
            hit_test(&app, &rects, wheel(MouseEventKind::ScrollDown, x, y)),
            Some(Command::DiffScroll(3)),
            "three lines a notch, like every other pane"
        );
        assert_eq!(
            hit_test(&app, &rects, wheel(MouseEventKind::ScrollUp, x, y)),
            Some(Command::DiffScroll(-3))
        );
    }

    /// The table covers the editor the way the viewer does, so a click on a
    /// cell must not reach the text behind it (SPEC §65).
    fn app_with_csv() -> App {
        use crate::app::{Tab, TabItem};
        use crate::editor::document::Document;

        let mut app = app();
        let path = std::path::PathBuf::from("/dev/null/ferroedit-fixture/people.csv");
        let document = Document::from_text("name,country\nada,uk\ngrace,us\n", Some(path));
        app.tabs.push(TabItem::editing(Tab::new(document)));
        app.active_tab = Some(app.tabs.len() - 1);
        app.focus = FocusTarget::Editor;
        app.sync_table();
        app
    }

    #[test]
    fn clicking_a_cell_selects_it_rather_than_placing_a_cursor() {
        let app = app_with_csv();
        let rects = rects(&app);
        let table = rects.table.expect("the table is showing");
        // The first data row is under the header and its rule; the first
        // column starts after the row-number gutter and its separator.
        let gutter = app.active().unwrap().table.as_ref().unwrap().gutter() as u16;
        let command = hit_test(&app, &rects, click(table.x + gutter + 1, table.y + 2));
        assert_eq!(
            command,
            Some(Command::SelectCell {
                row: Some(0),
                column: 0
            })
        );
    }

    /// A click on a column's name takes the column, as it does in a
    /// spreadsheet (SPEC §65).
    #[test]
    fn clicking_the_header_selects_the_whole_column() {
        let app = app_with_csv();
        let rects = rects(&app);
        let table = rects.table.expect("the table is showing");
        let gutter = app.active().unwrap().table.as_ref().unwrap().gutter() as u16;
        assert_eq!(
            hit_test(&app, &rects, click(table.x + gutter + 1, table.y)),
            Some(Command::SelectColumnAt { column: 0 })
        );
        // The rule under the names is chrome: a click on it selects nothing.
        assert_eq!(
            hit_test(&app, &rects, click(table.x + gutter + 1, table.y + 1)),
            Some(Command::FocusPane(FocusTarget::Editor))
        );
    }

    #[test]
    fn clicking_a_record_number_takes_the_whole_row() {
        let app = app_with_csv();
        let rects = rects(&app);
        let table = rects.table.expect("the table is showing");
        assert_eq!(
            hit_test(&app, &rects, click(table.x, table.y + 2)),
            Some(Command::SelectRowAt { row: Some(0) })
        );
        // The gutter above the records is blank chrome and selects nothing.
        assert_eq!(
            hit_test(&app, &rects, click(table.x, table.y)),
            Some(Command::FocusPane(FocusTarget::Editor))
        );
    }

    #[test]
    fn dragging_over_the_grid_makes_a_block_of_cells() {
        let app = app_with_csv();
        let rects = rects(&app);
        let table = rects.table.expect("the table is showing");
        let gutter = app.active().unwrap().table.as_ref().unwrap().gutter() as u16;
        assert_eq!(
            hit_test(&app, &rects, drag_event(table.x + gutter + 1, table.y + 3)),
            Some(Command::ExtendCellTo {
                row: Some(1),
                column: 0
            })
        );
    }

    #[test]
    fn the_wheel_over_the_table_scrolls_the_grid_and_not_the_text() {
        let app = app_with_csv();
        let rects = rects(&app);
        let (x, y) = centre(rects.table.expect("the table is showing"));
        assert_eq!(
            hit_test(&app, &rects, wheel(MouseEventKind::ScrollDown, x, y)),
            Some(Command::ScrollTable(3))
        );
        assert_eq!(
            hit_test(&app, &rects, wheel(MouseEventKind::ScrollRight, x, y)),
            Some(Command::ScrollTableHorizontal(1))
        );
    }

    /// The screen covers the body, tab bar and sidebar included, so nothing
    /// under it is clickable while it is open (ADR-038).
    fn app_with_help() -> App {
        use crate::app::help::HelpState;

        let mut app = app();
        app.help = Some(HelpState::new(FocusTarget::Editor));
        app.focus = FocusTarget::Help;
        app
    }

    #[test]
    fn clicking_the_help_screen_focuses_it_and_not_what_is_under_it() {
        let app = app_with_help();
        let rects = rects(&app);
        let help = rects.help.expect("the screen is open");
        for target in [help, rects.explorer, rects.tab_bar, rects.editor] {
            let (x, y) = centre(target);
            assert_eq!(
                hit_test(&app, &rects, click(x, y)),
                Some(Command::FocusPane(FocusTarget::Help)),
                "a click at {x},{y} reached past the screen"
            );
        }
    }

    /// The menu bar is above the body, so it stays reachable — which is how
    /// the screen is dismissed with the mouse alone.
    #[test]
    fn the_menu_bar_is_still_clickable_over_the_help_screen() {
        let app = app_with_help();
        let rects = rects(&app);
        let (x, y) = centre(rects.menu_titles[0]);
        assert_eq!(
            hit_test(&app, &rects, click(x, y)),
            Some(Command::MenuOpen(0))
        );
    }

    #[test]
    fn the_wheel_over_the_help_screen_scrolls_it() {
        let app = app_with_help();
        let rects = rects(&app);
        let (x, y) = centre(rects.help.expect("the screen is open"));
        assert_eq!(
            hit_test(&app, &rects, wheel(MouseEventKind::ScrollDown, x, y)),
            Some(Command::HelpScroll(3))
        );
        assert_eq!(
            hit_test(&app, &rects, wheel(MouseEventKind::ScrollUp, x, y)),
            Some(Command::HelpScroll(-3))
        );
    }

    #[test]
    fn a_closed_viewer_leaves_the_editor_clickable() {
        let app = app();
        let rects = rects(&app);
        assert!(rects.diff.is_none());
    }
}
