//! UI: menu bar and the open drop-down.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::commands::{MenuEntry, MENUS};
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

fn render_popup(frame: &mut Frame, app: &App, items: &[MenuEntry], popup: Rect, theme: &Theme) {
    // The drop-down covers whatever is behind it, so the buffer underneath has
    // to be cleared rather than blended with.
    frame.render_widget(Clear, popup);

    let inner_width = popup.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let Some(item) = entry.item() else {
                // A rule across the popup's inside, drawn in the border's
                // colour so it reads as part of the frame rather than as a row
                // of text that happens to be dashes.
                return Line::from(Span::styled(
                    "─".repeat(inner_width),
                    theme.menu_popup.fg(theme.popup_border),
                ));
            };
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
        .border_style(Style::new().fg(theme.popup_border))
        .style(theme.menu_popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::{Command, MenuEntry, MENUS};
    use crate::ui::{layout, theme::Theme};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// The open drop-down, one string per row.
    fn popup_rows(app: &App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let theme = Theme::default();
        let mut popup = None;
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), app);
                crate::ui::render(frame, app, &rects, &theme);
                popup = rects.menu_popup;
            })
            .unwrap();
        let popup = popup.expect("a menu is open");
        let buffer = terminal.backend().buffer().clone();
        (popup.y..popup.bottom())
            .map(|y| {
                (popup.x..popup.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// A separator draws as a rule across the popup rather than as a row of
    /// text, and the entries around it keep their own rows (ADR-056).
    #[test]
    fn a_rule_is_drawn_between_the_groups_of_a_menu() {
        let file = MENUS
            .iter()
            .position(|menu| menu.title == "File")
            .expect("a File menu");
        let mut app = App::fixture();
        app.menu.open = Some(file);

        let rows = popup_rows(&app);
        let screen = rows.join("\n");
        assert!(screen.contains("New File"), "{screen}");
        assert!(screen.contains("Delete"), "{screen}");
        // One drawn rule per separator in the table, plus the box's own two.
        let separators = MENUS[file]
            .items
            .iter()
            .filter(|entry| entry.is_separator())
            .count();
        let rules = rows
            .iter()
            .filter(|row| row.matches('─').count() > 4)
            .count();
        assert_eq!(rules, separators + 2, "{screen}");
    }

    /// Delete is alone between two rules: it is the one entry in the File menu
    /// that destroys something, and it used to sit one row above Open.
    #[test]
    fn delete_is_fenced_off_from_the_entries_around_it() {
        let file = MENUS
            .iter()
            .position(|menu| menu.title == "File")
            .expect("a File menu");
        let items = MENUS[file].items;
        let at = items
            .iter()
            .position(|entry| entry.item().map(|i| &i.command) == Some(&Command::DeletePrompt))
            .expect("a Delete entry");
        assert!(items[at - 1].is_separator(), "nothing is above Delete");
        assert!(items[at + 1].is_separator(), "nothing is below it either");
        assert!(matches!(items[at], MenuEntry::Item(_)));
    }
}
