//! UI: the box a large file is read behind (ADR-077).
//!
//! One small window over the editor pane: what is being opened, how far the
//! read has got, and a spinner that turns while it does. It is the only thing
//! on screen that says the editor is busy rather than wedged, so it is drawn
//! wherever the eye already is — the middle of the pane the file is about to
//! appear in.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::ui::theme::Theme;

/// The widest the box is allowed to get. A readout is one short line, and a box
/// stretched over a 200-column terminal would be a dialog rather than a note.
const MAX_WIDTH: u16 = 52;

/// Border, readout, bar.
const HEIGHT: u16 = 4;

pub fn render(frame: &mut Frame, app: &App, editor: Rect, theme: &Theme) {
    let Some(opening) = app.opening.as_ref() else {
        return;
    };
    let Some(area) = box_area(editor) else {
        return;
    };

    // The box covers what is behind it, so the buffer underneath is cleared
    // rather than blended with — the same rule a dialog is drawn by.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme.popup_border))
            .style(theme.dialog),
        area,
    );

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width - 2,
        height: area.height - 2,
    };
    frame.render_widget(
        Paragraph::new(opening.label())
            .style(theme.dialog)
            .alignment(Alignment::Center),
        Rect { height: 1, ..inner },
    );

    // No bar for a file whose length the filesystem would not say: a bar has to
    // be a fraction of something, and the readout has already left the
    // percentage out for the same reason.
    if let Some(percent) = opening.percent() {
        let bar = Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(progress_line(percent, bar.width, theme)),
            bar,
        );
    }
}

/// The box, centred in the editor pane — or `None` when the pane is too small
/// to hold one, where a clipped window would be worse than no window.
fn box_area(editor: Rect) -> Option<Rect> {
    let width = MAX_WIDTH.min(editor.width.saturating_sub(4));
    if width < 20 || editor.height < HEIGHT + 2 {
        return None;
    }
    Some(Rect {
        x: editor.x + (editor.width - width) / 2,
        y: editor.y + (editor.height - HEIGHT) / 2,
        width,
        height: HEIGHT,
    })
}

/// The bar itself: the part that is done in the colour the cursor's row is
/// marked with, the rest in the frame's, so the two are told apart by shade as
/// well as by shape.
fn progress_line(percent: u16, width: u16, theme: &Theme) -> Line<'static> {
    let width = width as usize;
    let filled = (width * percent.min(100) as usize) / 100;
    Line::from(vec![
        Span::styled("█".repeat(filled), Style::new().fg(theme.highlight)),
        Span::styled(
            "░".repeat(width - filled),
            Style::new().fg(theme.popup_border),
        ),
    ])
}
