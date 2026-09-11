//! UI: the find/replace bar under the editor (SPEC §22, §23).
//!
//! One row to find, two to replace. Everything on it that can be clicked —
//! the two fields, the `[Aa]` toggle and the two replace buttons — is a rect
//! computed in `ui/layout.rs`, so what is drawn and what the mouse hits are the
//! same numbers.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::search::SearchField;
use crate::app::App;
use crate::ui::layout::{LayoutRects, SearchRects};
use crate::ui::theme::Theme;
use unicode_width::UnicodeWidthStr;

pub fn render(frame: &mut Frame, app: &App, rects: &LayoutRects, theme: &Theme) {
    let Some(search) = rects.search.as_ref() else {
        return;
    };
    frame.render_widget(Block::new().style(theme.search_bar), search.bar);

    let focused = app.focus == FocusTarget::Search;
    render_find_row(frame, app, search, theme, focused);
    render_replace_row(frame, app, search, theme, focused);
}

/// Whether the caret is in this row's field.
///
/// The bar's own ground, its labels and its field styles all key off this: the
/// find bar is two rows of the same furniture, and "which of them am I typing
/// into" was answerable only from where the terminal had put its cursor.
fn field_focused(app: &App, focused: bool, field: SearchField) -> bool {
    focused && app.search.field == field
}

fn render_find_row(
    frame: &mut Frame,
    app: &App,
    rects: &SearchRects,
    theme: &Theme,
    focused: bool,
) {
    let here = field_focused(app, focused, SearchField::Query);
    label(frame, theme, rects.bar.x, rects.bar.y, " Find: ", here);
    crate::ui::field::render(
        frame,
        &app.search.query,
        rects.query,
        field_style(theme, here),
        here,
    );

    // Right-aligned so the number does not walk left and right as it changes.
    // On a file too large to search as the query is typed, the same eight cells
    // say what the bar is waiting for instead of counting nothing (ADR-075):
    // there is no count until the search has been run, and a `0/0` that means
    // "not asked yet" is a lie the user would act on.
    // A walk still running takes the same cells for its spinner and its running
    // count: the only moving thing on the bar, and the only sign that a long
    // search is working rather than wedged (ADR-076).
    let scanning = app.search.scan_label();
    let deferred = app.search.is_deferred();
    let count = match (&scanning, deferred) {
        (Some(label), _) => label.clone(),
        (None, true) => "Enter".to_string(),
        (None, false) => app.search.count_label(),
    };
    let count_style = if scanning.is_some() {
        theme.search_bar.fg(theme.search_label)
    } else if deferred || (app.search.matches.is_empty() && !app.search.query.value.is_empty()) {
        theme.search_bar.fg(theme.warning)
    } else {
        theme.search_bar.fg(theme.chrome_dim)
    };
    if rects.count.width > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(count, count_style))).right_aligned(),
            rects.count,
        );
    }

    // The case-sensitivity toggle. A five-cell label rather than a checkbox
    // because the bar has that many cells for it, not fourteen — but the mark
    // inside the brackets is what says which way it is pointing, and the
    // colour only agrees with it. It was the highlight and nothing else, which
    // on a bar the same grey as the button was a difference nobody read.
    if rects.case_toggle.width > 0 {
        let (text, style) = if app.search.case_sensitive {
            ("[Aa✓]", theme.search_option_on)
        } else {
            ("[Aa ]", theme.search_button)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ", theme.search_bar),
                Span::styled(text, style),
            ])),
            rects.case_toggle,
        );
    }
}

fn render_replace_row(
    frame: &mut Frame,
    app: &App,
    rects: &SearchRects,
    theme: &Theme,
    focused: bool,
) {
    let Some(row) = rects.replacement else {
        return;
    };
    let here = field_focused(app, focused, SearchField::Replacement);
    label(frame, theme, rects.bar.x, rects.bar.y + 1, " Repl: ", here);
    crate::ui::field::render(
        frame,
        &app.search.replacement,
        row,
        field_style(theme, here),
        here,
    );
    // `[All]` was the shortest true thing the button could be called and not
    // the clearest: it sits beside `[Replace]` on the replacement row, where
    // "all" of what was left to the reader.
    button(frame, theme, rects.replace_button, "[Replace]");
    button(frame, theme, rects.replace_all_button, "[Replace all]");
}

/// The ground a field is drawn on, and whether it is underlined.
fn field_style(theme: &Theme, focused: bool) -> Style {
    if focused {
        theme.search_field_focused
    } else {
        theme.search_field
    }
}

fn label(frame: &mut Frame, theme: &Theme, x: u16, y: u16, text: &str, focused: bool) {
    let style = if focused {
        theme.search_label_focused
    } else {
        theme.search_bar.fg(theme.search_label)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text.to_string(), style))),
        Rect::new(x, y, text.len() as u16, 1),
    );
}

fn button(frame: &mut Frame, theme: &Theme, rect: Option<Rect>, text: &str) {
    let Some(rect) = rect.filter(|r| r.width > 0) else {
        return;
    };
    // The blank columns in front are drawn in the bar's own style, so the two
    // buttons have the bar between them rather than a seam.
    let lead = usize::from(rect.width).saturating_sub(text.width());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ".repeat(lead), theme.search_bar),
            Span::styled(text.to_string(), theme.search_button),
        ]))
        .style(Style::new()),
        rect,
    );
}
