//! UI: menu bar and the open drop-down.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::commands::MENUS;
use crate::event::keyboard::shortcut_for;
use crate::ui::layout::LayoutRects;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, rects: &LayoutRects, theme: &Theme) {
    frame.render_widget(Block::new().style(theme.menu_bar), rects.menu_bar);

    for (index, menu) in MENUS.iter().enumerate() {
        let Some(rect) = rects.menu_titles.get(index) else {
            continue;
        };
        if rect.width == 0 {
            continue;
        }
        let style = if app.menu.open == Some(index) {
            theme.menu_title_open
        } else {
            theme.menu_bar
        };
        frame.render_widget(
            Paragraph::new(Line::from(format!(" {} ", menu.title))).style(style),
            *rect,
        );
    }

    if let (Some(open), Some(popup)) = (app.menu.open, rects.menu_popup) {
        render_popup(frame, app, MENUS[open].items, popup, theme);
    }
}

fn render_popup(
    frame: &mut Frame,
    app: &App,
    items: &[crate::commands::MenuItem],
    popup: Rect,
    theme: &Theme,
) {
    // The drop-down covers whatever is behind it, so the buffer underneath has
    // to be cleared rather than blended with.
    frame.render_widget(Clear, popup);

    let inner_width = popup.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let shortcut = shortcut_for(&item.command).unwrap_or("");
            // The row is " label" + gap + "shortcut ", so the two padding
            // columns are already accounted for outside the gap.
            let gap = inner_width
                .saturating_sub(item.label.chars().count() + shortcut.chars().count() + 2);
            let selected = app.menu.item == i;
            let base = if selected {
                theme.menu_item_selected
            } else {
                theme.menu_popup
            };
            let shortcut_style = if selected {
                base
            } else {
                Style::new().fg(theme.menu_shortcut)
            };
            Line::from(vec![
                Span::styled(format!(" {}", item.label), base),
                Span::styled(" ".repeat(gap), base),
                Span::styled(format!("{shortcut} "), shortcut_style),
            ])
        })
        .collect();

    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.border_for(true))
        .style(theme.menu_popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}
