//! UI: the five-zone layout, and the rects the mouse hit-tests against.
//!
//! `compute` is a pure function of the frame area and `&App`, so the main loop
//! can keep the last frame's rects for hit-testing without rendering ever
//! mutating state.

use ratatui::layout::{Constraint, Layout, Rect};
use unicode_width::UnicodeWidthStr;

use crate::app::dialog::{DialogButton, DialogState};
use crate::app::App;
use crate::commands::MENUS;
use crate::event::keyboard::shortcut_for;
use crate::ui::statusbar::{self, StatusZone};

/// Below this, the five zones cannot be drawn legibly and a notice is shown
/// instead. The phase acceptance size (60x20) is comfortably above it.
pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 8;

/// Narrowest usable editor column count, text only; the sidebar yields space
/// first.
const MIN_EDITOR_WIDTH: u16 = 20;

/// The column the editor's scrollbar sits in (ADR-052). It is on top of
/// `MIN_EDITOR_WIDTH` rather than out of it: the minimum is about how much text
/// is readable, and the bar is not text.
const EDITOR_SCROLLBAR_WIDTH: u16 = 1;

/// What a dialog costs besides its body: two border rows and the button row.
///
/// It was a fixed `DIALOG_HEIGHT` until the branch picker, whose body is as
/// tall as the list in it (ADR-035); a message and an input body both report a
/// height of two, so the boxes that existed before are unchanged at five rows.
const DIALOG_CHROME: u16 = 3;

/// Narrow enough for a 40-column terminal, wide enough that two buttons and a
/// short message are not squeezed onto each other.
const DIALOG_MIN_WIDTH: u16 = 24;

/// An input dialog is wider: a file name is typed into it, and a box that fits
/// only "Create in src" would scroll after a dozen characters.
const DIALOG_INPUT_MIN_WIDTH: u16 = 40;

/// The browser is wider again: it shows a directory path, and a box that elides
/// all but the last word of one is a box that cannot say where it is (ADR-051).
const DIALOG_BROWSER_MIN_WIDTH: u16 = 52;

/// Width of the ` Find: ` / ` Repl: ` labels at the left of each bar row.
const SEARCH_LABEL_WIDTH: u16 = 7;

/// `[Aa✓]` plus the space in front of it.
///
/// Five cells and not four: the toggle says which way it is pointing with a
/// mark inside the brackets as well as with a colour, so that a terminal whose
/// palette flattens the highlight — or a reader who does not see the
/// difference — still has the answer.
const CASE_TOGGLE_WIDTH: u16 = 6;

/// Room kept on the find row for the `3/17` readout.
const COUNT_WIDTH: u16 = 8;

/// The narrowest query field worth typing into. Furniture that would squeeze it
/// below this is dropped instead — a readout nobody can use is worth less than
/// the field it is crowding.
const MIN_SEARCH_FIELD: u16 = 8;

/// `[Replace]` and `[Replace all]`, each with the two blank columns that keep
/// it off the button beside it.
///
/// `[All]` said what it did to nobody who had not already guessed: the row it
/// sits on is the *replacement* row, and "all" of something is not a verb. The
/// two columns rather than one because the two buttons were touching, and a
/// pair of bracketed words with a single space between them reads as one.
const REPLACE_BUTTON_WIDTH: u16 = 11;
const REPLACE_ALL_BUTTON_WIDTH: u16 = 15;

/// Where the pieces of the find/replace bar are.
///
/// Computed here rather than in `ui/search.rs` for the same reason the tab
/// rects are: the mouse hit-tests against the last drawn frame, and a click
/// landing somewhere other than what it looked like it hit is the bug this
/// arrangement exists to prevent.
#[derive(Debug, Default, Clone)]
pub struct SearchRects {
    /// The whole bar, one row or two.
    pub bar: Rect,
    /// The query field, without its label.
    pub query: Rect,
    /// The `3/17` readout, right-aligned inside its own space.
    pub count: Rect,
    /// The `[Aa]` toggle.
    pub case_toggle: Rect,
    /// The replacement row's pieces, present only while replacing.
    pub replacement: Option<Rect>,
    pub replace_button: Option<Rect>,
    pub replace_all_button: Option<Rect>,
}

#[derive(Debug, Default, Clone)]
pub struct LayoutRects {
    pub too_small: bool,
    pub menu_bar: Rect,
    /// One rect per entry in `commands::MENUS`.
    pub menu_titles: Vec<Rect>,
    pub menu_popup: Option<Rect>,
    pub explorer: Rect,
    pub git_panel: Rect,
    pub tab_bar: Rect,
    /// One rect per open tab, in tab order. A tab scrolled out of the bar gets
    /// a zero-width rect rather than being left out, so an index into this
    /// vector is always the index of the tab itself.
    pub tabs: Vec<Rect>,
    /// The close button inside each tab, parallel to `tabs`. Zero-width for a
    /// tab that is off screen or only partly drawn: half a tab has no × to
    /// click.
    pub tab_closes: Vec<Rect>,
    /// One-cell markers saying there are more tabs off the left or right edge.
    pub tab_overflow_left: Option<Rect>,
    pub tab_overflow_right: Option<Rect>,
    pub editor: Rect,
    /// The one-cell column at the editor's right edge that its scrollbar lives
    /// in (ADR-052). Reserved whether or not a bar is drawn in it, so opening a
    /// longer file does not shift every line of the one already on screen.
    pub editor_scrollbar: Rect,
    /// The help screen, when one is open (SPEC §6). The whole body — the
    /// sidebar as well as the editor — because it is a screen and not a pane,
    /// and a key table squeezed into a 40-column editor is not readable
    /// (ADR-038). Everything under it must be hit-tested after it.
    pub help: Option<Rect>,
    /// The diff viewer, when one is open (SPEC §36). It is the whole editor
    /// pane — the editor rect and the scrollbar column beside it: the viewer
    /// covers the pane rather than splitting it, so a hit test that finds it
    /// must be made before the editor's own (ADR-037).
    pub diff: Option<Rect>,
    /// The log viewer, when the tab in front is one (ADR-068). The whole editor
    /// pane, for the reason the diff takes it, and hit-tested before the
    /// editor's own for the same reason.
    pub log: Option<Rect>,
    /// The rows of that viewer: its inside, less the search field's row while
    /// the field is open. Read back out of the frame the way `dialog_rows` is,
    /// because how far a list scrolls depends on how much of the pane it got.
    pub log_rows: Option<Rect>,
    /// The image viewer, when the tab in front is one (ADR-078). The whole
    /// editor pane, for the reason the diff takes it, and hit-tested before
    /// the editor's own for the same reason.
    pub image: Option<Rect>,
    /// The metadata column inside the viewer's frame, when there is room for
    /// it. `None` when the column is switched off or the pane is too narrow to
    /// give it twenty-six columns and still show a picture.
    pub image_meta: Option<Rect>,
    /// Where the blocks go: the rest of the viewer's frame. Read back out of
    /// the frame the way `log_rows` is, because the zoom, the pan and the
    /// resampling are all measured against it.
    pub image_canvas: Option<Rect>,
    /// The CSV table, when the tab in front is being read as one (SPEC §65).
    /// The whole editor pane, for the same reason the diff takes it: the view
    /// covers the pane rather than splitting it, and a hit test that finds it
    /// must be made before the editor's own.
    pub table: Option<Rect>,
    /// The find/replace bar under the editor, when it is open.
    pub search: Option<SearchRects>,
    pub status_bar: Rect,
    /// Where each clickable piece of the status readout landed (ADR-058).
    /// Computed by `ui::statusbar` itself, from the same pieces it draws, so a
    /// click cannot land on something other than what it looked like it hit.
    pub status_zones: Vec<(StatusZone, Rect)>,
    pub dialog: Option<Rect>,
    /// One rect per dialog button, in button order.
    pub dialog_buttons: Vec<Rect>,
    /// The list body's own area, when the dialog has one: where the rows are
    /// drawn, and what a click on a row is measured against. For the browser
    /// this is the *inside* of `dialog_list_frame`.
    pub dialog_list: Option<Rect>,
    /// The box drawn around the browser's rows, scrollbar included (ADR-051).
    /// `None` for the branch picker, whose rows sit directly on the dialog.
    pub dialog_list_frame: Option<Rect>,
}

pub fn compute(area: Rect, app: &App) -> LayoutRects {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return LayoutRects {
            too_small: true,
            ..LayoutRects::default()
        };
    }

    let [menu_bar, body, status_bar] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    // A quarter of the width, bounded so the sidebar is neither a sliver on a
    // wide terminal nor the majority of a narrow one.
    let sidebar_width = (area.width / 4).clamp(16, 32).min(
        area.width
            .saturating_sub(MIN_EDITOR_WIDTH + EDITOR_SCROLLBAR_WIDTH),
    );
    let [sidebar, right] = Layout::horizontal([
        Constraint::Length(sidebar_width),
        Constraint::Min(MIN_EDITOR_WIDTH + EDITOR_SCROLLBAR_WIDTH),
    ])
    .areas(body);

    let git_height = (body.height / 3).clamp(4, 10);
    let [explorer, git_panel] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(git_height)]).areas(sidebar);

    // The bar takes its rows out of the editor's, so opening it scrolls the
    // cursor rather than hiding it: `sync_editor_view` sees the smaller pane on
    // the next frame exactly as it sees a resize.
    let [tab_bar, pane, search_bar] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(app.search.height()),
    ])
    .areas(right);
    // The rightmost column of the pane is the editor's scrollbar (ADR-052). It
    // is taken from the text and not from the gutter, and taken always: a
    // column that appears the moment a file grows past the window would reflow
    // every line on screen at the least welcome moment. The diff viewer keeps
    // the whole pane — it covers the editor rather than sharing it, and a
    // viewer one column short of its own pane reads as a drawing bug.
    let [editor, editor_scrollbar] = Layout::horizontal([
        Constraint::Min(1),
        Constraint::Length(EDITOR_SCROLLBAR_WIDTH),
    ])
    .areas(pane);
    let search = (search_bar.height > 0).then(|| search_rects(search_bar, app.search.replacing));

    let menu_titles = menu_title_rects(menu_bar);
    let menu_popup = app
        .menu
        .open
        .and_then(|i| menu_titles.get(i).map(|t| popup_rect(area, *t, i, app)));

    let bar = tab_bar_layout(app, tab_bar);
    let dialog = app.dialog.as_ref().map(|d| dialog_rect(area, d));
    let dialog_buttons = match (dialog, app.dialog.as_ref()) {
        (Some(rect), Some(state)) => button_rects(rect, &state.buttons),
        _ => Vec::new(),
    };
    // The browser's rows live inside a frame of their own; the picker's sit
    // straight on the dialog, as they always have.
    // A field pushes the rows one row down, and is also what says the rows are
    // a pane worth framing: every list with a filter over it is one long enough
    // to scroll (ADR-051, ADR-058).
    let framed = app
        .dialog
        .as_ref()
        .is_some_and(DialogState::rows_are_framed);
    let has_field = app.dialog.as_ref().is_some_and(|d| d.field().is_some());
    let dialog_list_frame = match (dialog, app.dialog.as_ref()) {
        (Some(rect), Some(state)) if state.has_list_body() && framed => {
            Some(list_rect(rect, has_field))
        }
        _ => None,
    };
    let dialog_list = match (dialog_list_frame, dialog, app.dialog.as_ref()) {
        (Some(frame), _, _) => Some(inset(frame)),
        (None, Some(rect), Some(state)) if state.has_list_body() => {
            Some(list_rect(rect, has_field))
        }
        _ => None,
    };

    LayoutRects {
        too_small: false,
        menu_bar,
        menu_titles,
        menu_popup,
        explorer,
        git_panel,
        tab_bar,
        tabs: bar.tabs,
        tab_closes: bar.closes,
        tab_overflow_left: bar.overflow_left,
        tab_overflow_right: bar.overflow_right,
        editor,
        editor_scrollbar,
        help: app.help.as_ref().map(|_| body),
        diff: app.diff().map(|_| pane),
        log: app.log().map(|_| pane),
        log_rows: app.log().map(|_| log_rows(app, pane)),
        image: app.image().map(|_| pane),
        image_meta: app.image().and_then(|image| image_meta(image, pane)),
        image_canvas: app.image().map(|image| image_canvas(image, pane)),
        table: app.active().filter(|tab| tab.shows_table()).map(|_| pane),
        search,
        status_bar,
        status_zones: statusbar::zones(app, status_bar),
        dialog,
        dialog_buttons,
        dialog_list,
        dialog_list_frame,
    }
}

/// The metadata column of the image viewer: the left edge of the frame's
/// inside, when the pane is wide enough to spare it (ADR-078).
fn image_meta(image: &crate::app::image::ImageState, pane: Rect) -> Option<Rect> {
    let inner = inset(pane);
    if !image.meta_fits(inner.width) {
        return None;
    }
    Some(Rect {
        width: crate::app::image::META_WIDTH,
        ..inner
    })
}

/// What is left of the viewer's frame once the metadata column has had its
/// share: the cells the picture is drawn into.
fn image_canvas(image: &crate::app::image::ImageState, pane: Rect) -> Rect {
    let inner = inset(pane);
    match image_meta(image, pane) {
        Some(meta) => Rect {
            x: inner.x + meta.width,
            width: inner.width.saturating_sub(meta.width),
            ..inner
        },
        None => inner,
    }
}

/// The rows of the log viewer: inside its border, less the search field's row.
fn log_rows(app: &App, pane: Rect) -> Rect {
    let inner = inset(pane);
    match crate::ui::log::search_row(app, inner) {
        Some(_) => Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(1),
        ),
        None => inner,
    }
}

/// One cell in on every side — the inside of a bordered box.
fn inset(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

/// The rows between a list dialog's prompt and its buttons.
///
/// The browser has one more row above its list than the picker does — its
/// filter field — so the rows start one lower.
fn list_rect(popup: Rect, has_field: bool) -> Rect {
    // Border, prompt; and the button row plus the bottom border below.
    let top = popup.y.saturating_add(2 + u16::from(has_field));
    let height = popup.bottom().saturating_sub(2).saturating_sub(top);
    Rect::new(
        popup.x + 1,
        top.min(popup.bottom()),
        popup.width.saturating_sub(2),
        height,
    )
}

/// Splits the bar into its label, field, readout and buttons.
///
/// Everything on the right is measured from the right edge, and the field is
/// whatever is left between the label and the leftmost of them. A bar too
/// narrow for a piece gets a zero-width rect for it: the renderer skips those,
/// so a click can never land on something that was not drawn, and the field
/// takes the space instead — typing a query matters more than a readout that
/// does not fit.
fn search_rects(bar: Rect, replacing: bool) -> SearchRects {
    let find = Rect::new(bar.x, bar.y, bar.width, 1);
    let (case_toggle, count) = right_slices(find, &[CASE_TOGGLE_WIDTH, COUNT_WIDTH]);
    let query = field_slice(find, &[case_toggle, count]);

    let replace_row =
        (replacing && bar.height > 1).then(|| Rect::new(bar.x, bar.y + 1, bar.width, 1));
    let buttons =
        replace_row.map(|row| right_slices(row, &[REPLACE_ALL_BUTTON_WIDTH, REPLACE_BUTTON_WIDTH]));
    let replacement = replace_row.map(|row| match buttons {
        Some((all, one)) => field_slice(row, &[all, one]),
        None => field_slice(row, &[]),
    });

    SearchRects {
        bar,
        query,
        count,
        case_toggle,
        replacement,
        replace_button: buttons.map(|(_, one)| one),
        replace_all_button: buttons.map(|(all, _)| all),
    }
}

/// Two fixed-width slices packed against the row's right edge, outermost first.
///
/// A slice that would leave the field below `MIN_SEARCH_FIELD` is empty, and so
/// is every slice to its left: the furniture disappears from the inside out as
/// the bar narrows.
fn right_slices(row: Rect, widths: &[u16; 2]) -> (Rect, Rect) {
    let empty = Rect::new(row.right(), row.y, 0, 1);
    let mut end = row.right();
    let mut out = [empty; 2];
    for (slot, width) in out.iter_mut().zip(widths) {
        let start = end.saturating_sub(*width);
        if start < row.x + SEARCH_LABEL_WIDTH + MIN_SEARCH_FIELD {
            break;
        }
        *slot = Rect::new(start, row.y, *width, 1);
        end = start;
    }
    (out[0], out[1])
}

/// What is left of a row between its label and the furniture on its right.
fn field_slice(row: Rect, furniture: &[Rect]) -> Rect {
    let x = row.x + SEARCH_LABEL_WIDTH;
    let end = furniture
        .iter()
        .filter(|rect| rect.width > 0)
        .map(|rect| rect.x)
        .min()
        .unwrap_or(row.right());
    if x >= end {
        return Rect::new(row.right(), row.y, 0, 1);
    }
    Rect::new(x, row.y, end - x, 1)
}

/// Menu titles are laid out as ` File `, left to right, and clipped at the
/// right edge rather than wrapped.
fn menu_title_rects(menu_bar: Rect) -> Vec<Rect> {
    let mut rects = Vec::with_capacity(MENUS.len());
    let mut x = menu_bar.x;
    let end = menu_bar.right();
    for menu in MENUS {
        let width = menu.title.width() as u16 + 2;
        if x >= end {
            rects.push(Rect::new(end, menu_bar.y, 0, 1));
            continue;
        }
        let width = width.min(end - x);
        rects.push(Rect::new(x, menu_bar.y, width, 1));
        x += width;
    }
    rects
}

struct TabBarLayout {
    tabs: Vec<Rect>,
    closes: Vec<Rect>,
    overflow_left: Option<Rect>,
    overflow_right: Option<Rect>,
}

/// Tabs are laid out as ` main.rs ● × │`, left to right, scrolled so the active
/// one is on screen (SPEC §11).
///
/// The scroll offset is *derived* rather than stored: it is the earliest tab
/// that still leaves room for the active one. There is therefore no offset to
/// keep in step with opening, closing and reordering — the tab bar cannot be
/// scrolled to a tab that no longer exists, because it is recomputed from the
/// tabs themselves on every frame.
fn tab_bar_layout(app: &App, bar: Rect) -> TabBarLayout {
    let widths = tab_widths(app);
    let strip = strip_width(&widths, bar.width);
    let overflowing = strip < bar.width;
    // A cell at each end for the "more tabs this way" markers, taken out of the
    // space the tabs are laid out in so a marker never covers a file name.
    let inner = if overflowing {
        Rect::new(bar.x + 1, bar.y, strip, 1)
    } else {
        bar
    };

    let first = first_visible_tab(&widths, app.tab_scroll, inner.width);
    let mut tabs = Vec::with_capacity(widths.len());
    let mut closes = Vec::with_capacity(widths.len());
    let hidden = Rect::new(inner.x, bar.y, 0, 1);
    let end = inner.right();
    let mut x = inner.x;
    let mut last_whole: Option<usize> = None;

    for (index, width) in widths.iter().copied().enumerate() {
        if index < first || x >= end {
            tabs.push(hidden);
            closes.push(hidden);
            continue;
        }
        let drawn = width.min(end - x);
        tabs.push(Rect::new(x, bar.y, drawn, 1));
        if drawn == width {
            last_whole = Some(index);
            closes.push(Rect::new(x + width - CLOSE_FROM_RIGHT, bar.y, 1, 1));
        } else {
            closes.push(hidden);
        }
        x += drawn;
    }

    let more_after = last_whole.map_or(true, |last| last + 1 < widths.len());
    TabBarLayout {
        tabs,
        closes,
        overflow_left: (overflowing && first > 0).then(|| Rect::new(bar.x, bar.y, 1, 1)),
        overflow_right: (overflowing && more_after)
            .then(|| Rect::new(bar.right().saturating_sub(1), bar.y, 1, 1)),
    }
}

/// Which tab the strip starts at: what it was scrolled to, clamped.
///
/// Nothing here follows the active tab. The strip is a window the user moves
/// and the editor moves *for* them when the active tab changes — and the two
/// cannot both be decided every frame, or scrolling away from the file being
/// edited would spring back on the next redraw. `tab_scroll_showing` is the
/// other half, and the run loop calls it when the active tab moves.
fn first_visible_tab(widths: &[u16], requested: usize, width: u16) -> usize {
    if widths.is_empty() {
        return 0;
    }
    requested
        .min(widths.len() - 1)
        .min(max_first(widths, width))
}

/// Where the strip has to be scrolled to for the active tab to be whole.
///
/// The answer is the current scroll whenever the active tab is already on
/// screen, so switching between two visible tabs does not move the strip at
/// all. Called by the run loop when the active tab changes, and only then.
pub fn tab_scroll_showing(app: &App) -> usize {
    let widths = tab_widths(app);
    if widths.is_empty() {
        return 0;
    }
    let width = strip_width(&widths, app.tab_bar_width);
    let last = widths.len() - 1;
    let active = app.active_tab.unwrap_or(0).min(last);
    let mut first = first_visible_tab(&widths, app.tab_scroll, width);

    // In front of the window: the strip goes back to it.
    if active < first {
        return active;
    }
    // Behind it: the left edge walks in until it fits. The walk stops at the
    // active tab itself, so a tab wider than the whole bar is shown from its
    // own left edge rather than not at all.
    let mut used: u32 = widths[first..=active].iter().map(|w| *w as u32).sum();
    while first < active && used > width as u32 {
        used -= widths[first] as u32;
        first += 1;
    }
    first
}

fn tab_widths(app: &App) -> Vec<u16> {
    app.tabs.iter().map(|tab| tab_width(&tab.title())).collect()
}

/// Cells the tabs are laid out in: the whole bar, or two less when the strip
/// overflows and the two overflow arrows need a cell each.
fn strip_width(widths: &[u16], bar_width: u16) -> u16 {
    let total: u32 = widths.iter().map(|w| *w as u32).sum();
    if total > bar_width as u32 {
        bar_width.saturating_sub(2)
    } else {
        bar_width
    }
}

/// The furthest the strip may be scrolled: the earliest tab from which the
/// rest of them still fit inside the bar.
///
/// Scrolling one tab further would leave a gap at the right edge, which is
/// motion that shows nothing new.
fn max_first(widths: &[u16], width: u16) -> usize {
    let last = widths.len().saturating_sub(1);
    let mut tail = 0u32;
    for (index, w) in widths.iter().enumerate().rev() {
        tail += *w as u32;
        if tail > width as u32 {
            // The tabs from `index` on overflow the bar, so the last start
            // that does not is the one after it — unless there is no such
            // start at all, which is a single tab wider than the whole bar.
            return (index + 1).min(last);
        }
    }
    0
}

/// Cells between a tab's right edge and its close button: `× ` .
const CLOSE_FROM_RIGHT: u16 = 3;

/// Display width of a tab label, which is ` title ● × │`.
///
/// Uses `unicode_width` because file names are arbitrary text: a CJK name is
/// two cells per character, not one. The dirty marker keeps its cell whether or
/// not the file is modified, so that typing the first character into a file
/// does not shift every tab after it sideways under the pointer.
pub fn tab_width(title: &str) -> u16 {
    title.width() as u16 + 7
}

/// A centred modal box (SPEC §40).
fn dialog_rect(area: Rect, dialog: &DialogState) -> Rect {
    let content = [
        dialog.title.width(),
        dialog.prompt().width(),
        buttons_row_width(&dialog.buttons) as usize,
        // A branch name is the widest thing in a picker, and eliding one would
        // hide the part that distinguishes `feature/a` from `feature/b`.
        dialog.body_width(),
    ]
    .into_iter()
    .max()
    .unwrap_or(0) as u16;
    let minimum = match (dialog.browser().is_some(), dialog.field().is_some()) {
        (true, _) => DIALOG_BROWSER_MIN_WIDTH,
        (_, true) => DIALOG_INPUT_MIN_WIDTH,
        _ => DIALOG_MIN_WIDTH,
    };
    // Two cells of border and two of padding.
    let width = content.saturating_add(4).max(minimum).min(area.width);
    let height = (dialog.body_height() as u16)
        .saturating_add(DIALOG_CHROME)
        .min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// A button is drawn as `[ Label ]`.
fn button_width(label: &str) -> u16 {
    label.width() as u16 + 4
}

/// The whole button row, with one cell between neighbours.
fn buttons_row_width(buttons: &[DialogButton]) -> u16 {
    let labels: u16 = buttons.iter().map(|b| button_width(&b.label)).sum();
    labels + buttons.len().saturating_sub(1) as u16
}

/// Where each button lands, so a click can be matched to one.
///
/// The row is centred, and clipped from the right on a box too narrow to hold
/// it — a button that would fall outside the frame gets a zero-width rect and
/// so cannot be clicked by accident.
fn button_rects(popup: Rect, buttons: &[DialogButton]) -> Vec<Rect> {
    let inner_width = popup.width.saturating_sub(2);
    let row = buttons_row_width(buttons);
    // The row above the bottom border, wherever that is: a list body makes the
    // box as tall as the list.
    let y = popup.bottom().saturating_sub(2).max(popup.y);
    let mut x = popup.x + 1 + inner_width.saturating_sub(row) / 2;
    let end = popup.right().saturating_sub(1);

    buttons
        .iter()
        .map(|button| {
            let width = button_width(&button.label);
            if x >= end || end - x < width {
                return Rect::new(end, y, 0, 1);
            }
            let rect = Rect::new(x, y, width, 1);
            x += width + 1;
            rect
        })
        .collect()
}

/// Drop-down under a menu title, pushed left when it would overflow the frame.
fn popup_rect(area: Rect, title: Rect, index: usize, app: &App) -> Rect {
    let menu = &MENUS[index];
    // Two columns for the mark in front of the labels, in the menus that have
    // an entry carrying a state at all (SPEC §24).
    let marks = if crate::ui::menu::has_marks(app, menu.items) {
        2
    } else {
        0
    };
    // Separators have no label, so they never widen a popup: the box is as
    // wide as its widest entry, rule or no rule.
    let width = menu
        .entries()
        .map(|i| i.label.width() + shortcut_for(&i.command).unwrap_or("").width() + 4 + marks)
        .max()
        .unwrap_or(10) as u16
        + 2;
    let width = width.min(area.width);
    let height = (menu.items.len() as u16 + 2).min(area.height.saturating_sub(1));
    let x = title.x.min(area.right().saturating_sub(width));
    Rect::new(x, title.bottom(), width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::fixture()
    }

    fn contains(outer: Rect, inner: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.intersects(b)
    }

    #[test]
    fn the_five_zones_tile_the_frame_at_the_acceptance_size() {
        let area = Rect::new(0, 0, 60, 20);
        let rects = compute(area, &app());
        assert!(!rects.too_small);

        for zone in [
            rects.menu_bar,
            rects.explorer,
            rects.git_panel,
            rects.tab_bar,
            rects.editor,
            rects.editor_scrollbar,
            rects.status_bar,
        ] {
            assert!(contains(area, zone), "{zone:?} escapes {area:?}");
            assert!(zone.width > 0 && zone.height > 0, "{zone:?} is empty");
        }

        let zones = [
            rects.menu_bar,
            rects.explorer,
            rects.git_panel,
            rects.tab_bar,
            rects.editor,
            rects.editor_scrollbar,
            rects.status_bar,
        ];
        for (i, a) in zones.iter().enumerate() {
            for b in &zones[i + 1..] {
                assert!(!overlaps(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
        assert_eq!(
            zones.iter().map(|z| z.area()).sum::<u32>(),
            area.area(),
            "the zones must cover the frame exactly"
        );
    }

    #[test]
    fn a_tiny_terminal_reports_too_small_instead_of_panicking() {
        for (w, h) in [(10, 4), (39, 20), (60, 7), (0, 0)] {
            let rects = compute(Rect::new(0, 0, w, h), &app());
            assert!(rects.too_small, "{w}x{h} should be too small");
        }
    }

    #[test]
    fn the_editor_keeps_its_minimum_width_as_the_frame_narrows() {
        for width in MIN_WIDTH..200 {
            let rects = compute(Rect::new(0, 0, width, 24), &app());
            assert!(
                rects.editor.width >= MIN_EDITOR_WIDTH,
                "editor collapsed to {} at width {width}",
                rects.editor.width
            );
        }
    }

    #[test]
    fn the_strip_starts_where_it_was_scrolled_to() {
        let widths = [10, 10, 10, 10, 10];
        assert_eq!(first_visible_tab(&widths, 2, 30), 2);
    }

    /// No scrolling into a half-empty bar.
    #[test]
    fn the_strip_stops_when_the_last_tabs_fill_the_bar() {
        let widths = [10, 10, 10, 10, 10];
        assert_eq!(max_first(&widths, 30), 2, "three tabs fill 30 cells");
        assert_eq!(first_visible_tab(&widths, 4, 30), 2);
        assert_eq!(max_first(&widths, 100), 0, "they all fit already");
        assert_eq!(first_visible_tab(&widths, 4, 100), 0);
    }

    /// A tab wider than the bar is drawn from its own left edge rather than
    /// skipped, and a strip with nothing on it has nowhere to scroll to.
    #[test]
    fn the_awkward_shapes_do_not_panic() {
        assert_eq!(first_visible_tab(&[40], 0, 20), 0);
        assert_eq!(first_visible_tab(&[], 3, 20), 0);
        // Both fit, so there is nothing to scroll past.
        assert_eq!(first_visible_tab(&[10, 10], 99, 20), 0);
    }

    /// Switching to a tab off the right edge pulls the strip along; switching
    /// back to one in front of the window pulls it back.
    #[test]
    fn the_strip_follows_the_active_tab_when_it_changes() {
        let mut app = App::fixture_with_tabs(12);
        // Twelve `file00.rs` tabs are fifteen cells each; a 60-cell bar with
        // an arrow at each end lays them out in 58, so three fit at a time.
        app.tab_bar_width = 60;
        assert_eq!(tab_width("file00.rs"), 16);

        app.active_tab = Some(11);
        assert_eq!(tab_scroll_showing(&app), 9);

        app.tab_scroll = 9;
        app.active_tab = Some(10);
        assert_eq!(
            tab_scroll_showing(&app),
            9,
            "a tab already on screen does not move the strip"
        );

        app.active_tab = Some(0);
        assert_eq!(tab_scroll_showing(&app), 0);
    }

    #[test]
    fn tab_rects_are_ordered_and_stay_inside_the_tab_bar() {
        let rects = compute(Rect::new(0, 0, 120, 30), &app());
        assert_eq!(rects.tabs.len(), 3);
        let mut previous = rects.tab_bar.x;
        for tab in &rects.tabs {
            assert!(tab.x >= previous);
            assert!(contains(rects.tab_bar, *tab));
            previous = tab.right();
        }
    }

    #[test]
    fn a_tab_is_as_wide_as_its_name_plus_its_two_markers() {
        // ` main.rs ● × ` — the same width dirty or clean, so nothing moves
        // sideways when a file is edited.
        assert_eq!(tab_width("main.rs"), 14);
        // A CJK name is two cells per character, not one: three ideographs
        // take as much room as six letters.
        assert_eq!(tab_width("日本語.rs"), tab_width("abcdef.rs"));
    }

    #[test]
    fn every_whole_tab_has_a_close_button_two_cells_off_its_right_edge() {
        let app = app();
        let rects = compute(Rect::new(0, 0, 120, 30), &app);
        for (tab, close) in rects.tabs.iter().zip(&rects.tab_closes) {
            assert_eq!(close.width, 1);
            // A space and the separator rule sit after it (ADR-072).
            assert_eq!(close.x, tab.right() - 3);
            assert!(contains(*tab, *close));
        }
    }

    #[test]
    fn the_tab_bar_scrolls_to_whichever_tab_is_active() {
        let mut app = App::fixture_with_tabs(12);
        let area = Rect::new(0, 0, 60, 30);

        let rects = compute(area, &app);
        assert!(rects.tabs[0].width > 0, "the active tab is on screen");
        assert_eq!(rects.tabs[11].width, 0, "and the last one is not");
        assert!(rects.tab_overflow_left.is_none());
        assert!(rects.tab_overflow_right.is_some());

        // What the run loop does when the active tab changes.
        app.tab_bar_width = rects.tab_bar.width;
        app.active_tab = Some(11);
        app.tab_scroll = tab_scroll_showing(&app);
        let rects = compute(area, &app);
        assert!(rects.tabs[11].width > 0, "now it is");
        assert_eq!(rects.tabs[0].width, 0);
        assert!(rects.tab_overflow_left.is_some());
        assert!(rects.tab_overflow_right.is_none());

        for tab in rects.tabs.iter().filter(|t| t.width > 0) {
            assert!(contains(rects.tab_bar, *tab), "{tab:?} escapes the bar");
        }
    }

    #[test]
    fn a_bar_wide_enough_for_every_tab_has_no_overflow_markers() {
        let rects = compute(Rect::new(0, 0, 200, 30), &App::fixture_with_tabs(4));
        assert!(rects.tab_overflow_left.is_none());
        assert!(rects.tab_overflow_right.is_none());
        assert!(rects.tabs.iter().all(|t| t.width > 0));
    }

    #[test]
    fn a_partly_drawn_tab_has_no_close_button_to_click() {
        let app = App::fixture_with_tabs(12);
        let rects = compute(Rect::new(0, 0, 60, 30), &app);
        let clipped = rects
            .tabs
            .iter()
            .zip(&rects.tab_closes)
            .find(|(tab, _)| tab.width > 0 && tab.width < tab_width("file00.rs"));
        if let Some((_, close)) = clipped {
            assert_eq!(close.width, 0, "half a tab has no × to click");
        }
    }

    #[test]
    fn a_dialog_is_centred_and_its_buttons_stay_inside_it() {
        for (w, h) in [(40, 8), (60, 20), (80, 24), (200, 60)] {
            let mut app = app();
            app.dialog = Some(crate::app::dialog::DialogState::unsaved_changes(
                0,
                "a-very-long-file-name-indeed.rs",
                crate::app::focus::FocusTarget::Editor,
            ));
            let area = Rect::new(0, 0, w, h);
            let rects = compute(area, &app);
            let popup = rects.dialog.expect("an open dialog has a box");
            assert!(contains(area, popup), "{w}x{h}: {popup:?}");
            for button in &rects.dialog_buttons {
                assert!(
                    button.width == 0 || contains(popup, *button),
                    "{w}x{h}: {button:?} escapes {popup:?}"
                );
            }
        }
    }

    #[test]
    fn the_dialog_buttons_are_ordered_and_do_not_overlap() {
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::unsaved_changes(
            0,
            "main.rs",
            crate::app::focus::FocusTarget::Editor,
        ));
        let rects = compute(Rect::new(0, 0, 80, 24), &app);
        assert_eq!(rects.dialog_buttons.len(), 3);
        let mut previous = 0;
        for button in &rects.dialog_buttons {
            assert!(button.width > 0, "all three fit at 80 columns");
            assert!(button.x >= previous);
            previous = button.right();
        }
    }

    /// A list body makes the box as tall as the list, and the button row moves
    /// with the bottom border rather than staying at a fixed row (ADR-035).
    #[test]
    fn a_picker_grows_with_its_list_and_keeps_its_buttons_inside() {
        use crate::git::models::Branch;
        let branches: Vec<Branch> = (0..6)
            .map(|i| Branch {
                name: format!("feature/{i}"),
                is_head: i == 0,
                remote: false,
            })
            .collect();
        let mut app = App::fixture();
        app.dialog = Some(crate::app::dialog::DialogState::switch_branch(
            &branches,
            crate::app::focus::FocusTarget::GitPanel,
        ));
        let rects = compute(Rect::new(0, 0, 80, 24), &app);
        let popup = rects.dialog.expect("a box");
        // Prompt, six rows, two borders and the button row.
        assert_eq!(popup.height, 6 + 4);
        let list = rects.dialog_list.expect("a list area");
        assert_eq!(list.height, 6);
        assert!(popup.contains(list.as_position()));
        for button in &rects.dialog_buttons {
            assert!(button.y < popup.bottom() - 1, "{button:?} in {popup:?}");
            assert!(button.y >= list.bottom(), "below the list: {button:?}");
        }
    }

    /// A terminal too short for the whole list must still draw a box that fits
    /// in it, with nothing hanging outside the frame.
    #[test]
    fn a_picker_taller_than_the_terminal_is_clipped_rather_than_overflowing() {
        use crate::git::models::Branch;
        let branches: Vec<Branch> = (0..20)
            .map(|i| Branch {
                name: format!("b{i}"),
                is_head: i == 0,
                remote: false,
            })
            .collect();
        let mut app = App::fixture();
        app.dialog = Some(crate::app::dialog::DialogState::switch_branch(
            &branches,
            crate::app::focus::FocusTarget::GitPanel,
        ));
        let area = Rect::new(0, 0, 80, MIN_HEIGHT);
        let rects = compute(area, &app);
        let popup = rects.dialog.expect("a box");
        assert!(popup.bottom() <= area.bottom(), "{popup:?} in {area:?}");
        for button in &rects.dialog_buttons {
            assert!(button.bottom() <= area.bottom(), "{button:?}");
        }
    }

    /// The browser is the widest dialog and the tallest, so a 40x12 terminal
    /// is where it would break first (ADR-051).
    #[test]
    fn the_browser_fits_the_smallest_terminal_it_can_be_drawn_in() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();

        let mut app = App::fixture();
        app.dialog = Some(crate::app::dialog::DialogState::browse(
            dir.path(),
            crate::app::focus::FocusTarget::Editor,
        ));
        let area = Rect::new(0, 0, 40, 12);
        let rects = compute(area, &app);
        let popup = rects.dialog.expect("a box");
        assert!(popup.width <= area.width && popup.height <= area.height);

        let list = rects.dialog_list.expect("a list area");
        assert!(list.y >= popup.y + 3, "under the location and the filter");
        assert!(list.bottom() <= popup.bottom() - 2, "above the button row");
        for button in &rects.dialog_buttons {
            assert!(popup.contains(ratatui::layout::Position::new(button.x, button.y)));
        }
    }

    #[test]
    fn a_closed_dialog_has_no_box_and_no_buttons() {
        let rects = compute(Rect::new(0, 0, 80, 24), &app());
        assert!(rects.dialog.is_none());
        assert!(rects.dialog_buttons.is_empty());
    }

    #[test]
    fn the_menu_popup_never_escapes_the_frame() {
        let area = Rect::new(0, 0, 60, 20);
        for index in 0..MENUS.len() {
            let mut app = app();
            app.menu.open = Some(index);
            let rects = compute(area, &app);
            let popup = rects.menu_popup.expect("an open menu has a popup");
            assert!(contains(area, popup), "menu {index}: {popup:?}");
        }
    }

    /// Nothing on the find bar is ever drawn over anything else on it, at any
    /// width the editor draws at all.
    ///
    /// The pieces are packed from the right and the field takes what is left,
    /// so a bar too narrow for a piece drops it — and a piece that was dropped
    /// has a zero-width rect the renderer skips, which is what stops a click
    /// landing on something that was not drawn. Every command the dropped
    /// furniture stands for is on the Search menu, so a narrow window loses a
    /// button and not the ability.
    #[test]
    fn the_find_bars_pieces_never_overlap_at_any_width() {
        for width in MIN_WIDTH..=200 {
            let bar = Rect::new(0, 0, width, 2);
            let rects = search_rects(bar, true);
            let mut spans: Vec<Rect> = vec![rects.query, rects.count, rects.case_toggle];
            spans.extend(rects.replacement);
            spans.extend(rects.replace_button);
            spans.extend(rects.replace_all_button);
            spans.retain(|rect| rect.width > 0);
            for (i, a) in spans.iter().enumerate() {
                assert!(
                    a.x >= bar.x + SEARCH_LABEL_WIDTH && a.right() <= bar.right(),
                    "at {width}: {a:?} escapes the bar"
                );
                for b in spans.iter().skip(i + 1) {
                    assert!(
                        a.y != b.y || !a.intersects(*b),
                        "at {width}: {a:?} and {b:?} overlap"
                    );
                }
            }
        }
    }

    #[test]
    fn a_closed_menu_has_no_popup() {
        assert!(compute(Rect::new(0, 0, 80, 24), &app())
            .menu_popup
            .is_none());
    }
}
