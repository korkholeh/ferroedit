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

pub fn render(frame: &mut Frame, app: &App, rects: &LayoutRects, theme: &Theme) {
    let Some(search) = rects.search.as_ref() else {
        return;
    };
    frame.render_widget(Block::new().style(theme.search_bar), search.bar);

    let focused = app.focus == FocusTarget::Search;
    render_find_row(frame, app, search, theme, focused);
    render_replace_row(frame, app, search, theme, focused);
}

fn render_find_row(
    frame: &mut Frame,
    app: &App,
    rects: &SearchRects,
    theme: &Theme,
    focused: bool,
) {
    label(frame, theme, rects.bar.x, rects.bar.y, " Find: ");
    crate::ui::field::render(
        frame,
        &app.search.query,
        rects.query,
        theme.search_field,
        focused && app.search.field == SearchField::Query,
    );

    // Right-aligned so the number does not walk left and right as it changes.
    let count = app.search.count_label();
    let count_style = if app.search.matches.is_empty() && !app.search.query.value.is_empty() {
        theme.search_bar.fg(theme.warning)
    } else {
        theme.search_bar.fg(theme.dim)
    };
    if rects.count.width > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(count, count_style))).right_aligned(),
            rects.count,
        );
    }

    // `[Aa]` filled in means case-sensitive. A label rather than a checkbox
    // because the bar has four cells for it, not fourteen.
    if rects.case_toggle.width > 0 {
        let style = if app.search.case_sensitive {
            theme.search_option_on
        } else {
            theme.search_button
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ", theme.search_bar),
                Span::styled("[Aa]", style),
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
    label(frame, theme, rects.bar.x, rects.bar.y + 1, " Repl: ");
    crate::ui::field::render(
        frame,
        &app.search.replacement,
        row,
        theme.search_field,
        focused && app.search.field == SearchField::Replacement,
    );
    button(frame, theme, rects.replace_button, "[Replace]");
    button(frame, theme, rects.replace_all_button, "[All]");
}

fn label(frame: &mut Frame, theme: &Theme, x: u16, y: u16, text: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text.to_string(),
            theme.search_bar.fg(theme.search_label),
        ))),
        Rect::new(x, y, text.len() as u16, 1),
    );
}

fn button(frame: &mut Frame, theme: &Theme, rect: Option<Rect>, text: &str) {
    let Some(rect) = rect.filter(|r| r.width > 0) else {
        return;
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", theme.search_bar),
            Span::styled(text.to_string(), theme.search_button),
        ]))
        .style(Style::new()),
        rect,
    );
}
