//! Mouse hit-testing against the layout rects of the last frame.
//!
//! Hit-testing deliberately reads only `LayoutRects` and `&App` and produces a
//! `Command`: mouse input goes through exactly the same mutation path as the
//! keyboard and the menu (SPEC §25).

use std::time::Duration;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::app::focus::FocusTarget;
use crate::app::search::SearchField;
use crate::app::{App, SidebarMode};
use crate::commands::Command;
use crate::editor::viewport::gutter_width;
use crate::ui::layout::LayoutRects;

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
        return dialog_click(rects, event.kind, at);
    }

    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => left_click(app, rects, at),
        MouseEventKind::Down(MouseButton::Middle) => middle_click(rects, at),
        MouseEventKind::Drag(MouseButton::Left) => drag(app, rects, at),
        MouseEventKind::ScrollDown => scroll(rects, at, WHEEL_STEP),
        MouseEventKind::ScrollUp => scroll(rects, at, -WHEEL_STEP),
        _ => None,
    }
}

fn dialog_click(rects: &LayoutRects, kind: MouseEventKind, at: Position) -> Option<Command> {
    if kind != MouseEventKind::Down(MouseButton::Left) {
        return None;
    }
    rects
        .dialog_buttons
        .iter()
        .position(|rect| rect.contains(at))
        .map(Command::DialogActivateButton)
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
    if rects.editor.contains(at) {
        return Some(editor_click(app, rects.editor, at));
    }
    if rects.explorer.contains(at) {
        return Some(explorer_click(app, rects.explorer, at));
    }
    if rects.git_panel.contains(at) {
        return Some(sidebar_click(
            app,
            rects.git_panel,
            at,
            FocusTarget::GitPanel,
            SidebarMode::Git,
        ));
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
    Some((tab.viewport.top_line + row, tab.viewport.left_col.0 + cell))
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

/// Clicking an unfocused sidebar panel focuses it; clicking the focused one
/// selects the row under the cursor.
fn sidebar_click(
    app: &App,
    panel: Rect,
    at: Position,
    target: FocusTarget,
    mode: SidebarMode,
) -> Command {
    if app.focus != target || app.sidebar.mode != mode {
        return Command::FocusPane(target);
    }
    let row = at.y.saturating_sub(panel.y + PANEL_HEADER_ROWS) as usize;
    Command::SelectSidebarRow(row + sidebar_scroll(app, mode))
}

fn sidebar_scroll(app: &App, mode: SidebarMode) -> usize {
    match mode {
        SidebarMode::Explorer => app.sidebar.scroll,
        SidebarMode::Git => app.git.scroll,
    }
}

fn scroll(rects: &LayoutRects, at: Position, delta: i16) -> Option<Command> {
    if rects.editor.contains(at) || rects.tab_bar.contains(at) {
        return Some(Command::ScrollEditor(delta));
    }
    if rects.explorer.contains(at) || rects.git_panel.contains(at) {
        return Some(Command::ScrollSidebar(delta));
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

    fn app() -> App {
        App::fixture()
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

    #[test]
    fn clicking_an_unfocused_git_panel_focuses_it_before_selecting() {
        let app = app();
        let r = rects(&app);
        assert_eq!(app.focus, FocusTarget::Editor);
        assert_eq!(
            hit_test(&app, &r, click(r.git_panel.x + 2, r.git_panel.y + 2)),
            Some(Command::FocusPane(FocusTarget::GitPanel))
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
        // visible row rather than stopping dead where the pointer left.
        let command = hit_test(&app, &r, drag_event(0, r.editor.bottom() + 5));
        assert_eq!(
            command,
            Some(Command::ExtendCursorTo {
                line: (r.editor.height - 1) as usize,
                col: 0,
            })
        );
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
    fn a_click_on_the_status_bar_is_not_a_command() {
        let app = app();
        let r = rects(&app);
        assert_eq!(hit_test(&app, &r, click(1, r.status_bar.y)), None);
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
}
