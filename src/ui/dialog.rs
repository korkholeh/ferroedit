//! UI: dialog rendering.
//!
//! One renderer for every modal window (SPEC §40): a bordered box with a title,
//! a message and a row of buttons. The button rects come from `ui::layout`, the
//! same ones the mouse hit-tests against, so what is drawn and what is clickable
//! cannot drift apart.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::browser::Browser;
use crate::app::dialog::DialogState;
use crate::app::input_field::InputField;
use crate::app::App;
use crate::ui::layout::LayoutRects;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, rects: &LayoutRects, theme: &Theme) {
    let (Some(dialog), Some(area)) = (app.dialog.as_ref(), rects.dialog) else {
        return;
    };

    // The box covers whatever is behind it, so the buffer underneath has to be
    // cleared rather than blended with.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .border_style(theme.border_for(true))
            .title(format!(" {} ", dialog.title))
            .title_style(theme.dialog_title)
            .style(theme.dialog),
        area,
    );

    render_message(frame, dialog, area, theme);
    if let Some(field) = dialog.field() {
        // A field that narrows a list needs saying what it is: a box you can
        // type into that is not asking for a name is a box nobody types into.
        let label = dialog.field_filters().then_some("Filter: ");
        render_field(frame, field, label, area, theme);
    }
    if let Some(list) = rects.dialog_list {
        if let Some(outline) = rects.dialog_list_frame {
            let (len, scroll) = dialog.list_extent();
            render_list_frame(frame, outline, list, len, scroll, theme);
        }
        match dialog.browser() {
            Some(browser) => render_browser(frame, browser, list, theme),
            None => render_list(frame, dialog, list, theme),
        }
    }
    render_buttons(frame, dialog, rects, theme);
}

/// The box around a scrolling list's rows, and the scrollbar down its right
/// edge.
///
/// The frame is what says the rows are a pane and not three lines of the
/// dialog; the scrollbar is what says there is more of it. The bar is drawn
/// only when there *is* more — a full-height thumb on a five-entry folder is a
/// control that lies about having something to do.
///
/// It was the browser's alone (ADR-051) until the syntax picker, whose seventy
/// rows needed to look like seventy (ADR-058); `len` and `scroll` are passed in
/// rather than a body, so the two lists share it without either knowing about
/// the other.
fn render_list_frame(
    frame: &mut Frame,
    outline: Rect,
    rows: Rect,
    len: usize,
    scroll: usize,
    theme: &Theme,
) {
    if outline.width < 2 || outline.height < 2 {
        return;
    }
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .border_style(theme.border_for(false))
            .style(theme.dialog),
        outline,
    );
    let height = rows.height as usize;
    if height == 0 || len <= height {
        return;
    }
    let mut state = ScrollbarState::new(len.saturating_sub(height))
        .position(scroll)
        .viewport_content_length(height);
    // Down the right border, between the corners: the bar replaces the border
    // it sits on, and a frame missing its corners looks broken rather than
    // scrollable. The track is the border's own `│`, so the box still reads as
    // a box where there is nothing to scroll past.
    let track = Rect::new(outline.right() - 1, rows.y, 1, rows.height);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_style(theme.dialog.fg(theme.border_focused))
            .track_style(theme.dialog.fg(theme.border)),
        track,
        &mut state,
    );
}

/// The browser's rows (ADR-051).
///
/// A directory is marked by the same trailing `/` a shell uses rather than by
/// an icon, so the mark survives a terminal with no glyphs for one and takes a
/// column that the name would not have used anyway. `..` is drawn like the
/// directory it is, at the top, where every file manager puts it.
fn render_browser(frame: &mut Frame, browser: &Browser, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let selected = browser.selected();
    let rows: Vec<Line> = browser
        .rows()
        .enumerate()
        .skip(browser.scroll())
        .take(area.height as usize)
        .map(|(index, entry)| {
            // `..` is a directory, but it is not a *name* with children under
            // it, and `../` reads like a path fragment rather than a row.
            let name = if entry.is_dir && !entry.parent {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            };
            let style = if entry.is_dir {
                theme.dialog.fg(theme.directory)
            } else {
                theme.dialog
            };
            let line = Line::from(Span::styled(format!("  {name}"), style));
            if index == selected {
                line.style(theme.dialog_button_selected)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), area);
}

/// The rows of a list body (SPEC §33).
///
/// `*` is git's own marker for the branch `HEAD` is on, and it is deliberately
/// not the same thing as the highlight: one says where you are, the other says
/// what Enter would do.
fn render_list(frame: &mut Frame, dialog: &DialogState, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let (selected, scroll) = dialog.list_view();
    let rows: Vec<Line> = dialog
        .rows()
        .enumerate()
        .skip(scroll)
        .take(area.height as usize)
        .map(|(index, item)| {
            let marker = if item.current { "*" } else { " " };
            let line = Line::from(Span::styled(
                format!("{marker} {}", item.label),
                if item.current {
                    theme.dialog.fg(theme.git_added)
                } else {
                    theme.dialog
                },
            ));
            if index == selected {
                line.style(theme.dialog_button_selected)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), area);
}

fn render_message(frame: &mut Frame, dialog: &DialogState, area: Rect, theme: &Theme) {
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2).min(1),
    );
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    // The browser's top line is where it is, not a question — and it is left
    // aligned, because a path that moves sideways as you walk into directories
    // is a path nobody can read.
    // One column of padding, so the path lines up with the filter and the rows
    // below it rather than leaning on the border.
    if let Some(location) = dialog.browser_location(inner.width.saturating_sub(1) as usize) {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {location}"),
                theme.dialog_title,
            ))),
            inner,
        );
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(dialog.prompt(), theme.dialog))).centered(),
        inner,
    );
}

/// The text field of an input dialog, on the row between the prompt and the
/// buttons.
fn render_field(
    frame: &mut Frame,
    field: &InputField,
    label: Option<&str>,
    area: Rect,
    theme: &Theme,
) {
    let mut row = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(3).min(1),
    );
    if let Some(label) = label {
        let width = (label.width() as u16).min(row.width);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(label, theme.dialog))),
            Rect { width, ..row },
        );
        row.x += width;
        row.width -= width;
    }
    // A dialog is modal, so its field always has the caret.
    crate::ui::field::render(frame, field, row, theme.dialog_input, true);
}

fn render_buttons(frame: &mut Frame, dialog: &DialogState, rects: &LayoutRects, theme: &Theme) {
    for (index, button) in dialog.buttons.iter().enumerate() {
        let Some(rect) = rects.dialog_buttons.get(index) else {
            continue;
        };
        if rect.width == 0 {
            continue;
        }
        let style = if index == dialog.selected {
            theme.dialog_button_selected
        } else {
            theme.dialog_button
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("[ {} ]", button.label),
                style,
            ))),
            *rect,
        );
    }
}
