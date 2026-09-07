//! UI: dialog rendering.
//!
//! One renderer for every modal window (SPEC §40): a bordered box with a title,
//! a message and a row of buttons. The button rects come from `ui::layout`, the
//! same ones the mouse hit-tests against, so what is drawn and what is clickable
//! cannot drift apart.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

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
        render_field(frame, field, area, theme);
    }
    render_buttons(frame, dialog, rects, theme);
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
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(dialog.prompt(), theme.dialog))).centered(),
        inner,
    );
}

/// The text field of an input dialog, on the row between the prompt and the
/// buttons.
fn render_field(frame: &mut Frame, field: &InputField, area: Rect, theme: &Theme) {
    let row = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(3).min(1),
    );
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
