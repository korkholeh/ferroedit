//! UI: menu bar and the open drop-down.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::commands::{menu_enabled, menu_state, MenuEntry, MenuItem, MENUS};
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
    // The mark column belongs to the whole popup rather than to the rows that
    // have a state: a menu whose entries did not share a left edge would read
    // as two lists.
    let marks = has_marks(app, items);
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
            let mark = if marks { mark_for(app, item) } else { "" };
            // The row is " mark label" + gap + "shortcut ", so the two padding
            // columns are already accounted for outside the gap.
            let gap = inner_width.saturating_sub(
                mark.chars().count() + item.label.chars().count() + shortcut.chars().count() + 2,
            );
            let selected = app.menu.item == i;
            // An entry the state has greyed out (ADR-084). It keeps its row,
            // its label and its key — what it loses is its contrast, and the
            // selection cannot rest on it, so it is never drawn selected.
            let enabled = menu_enabled(app, &item.command);
            let base = match (selected, enabled) {
                // Greyed wins over selected. The walk cannot put the cursor on
                // a row like this, but the state behind a menu can change
                // under an open drop-down, and a bar drawn across an entry
                // that will not run is the one thing worse than no bar at all.
                (_, false) => theme.menu_popup.fg(theme.menu_disabled),
                (true, true) => theme.menu_item_selected,
                (false, true) => theme.menu_popup,
            };
            let shortcut_style = if selected {
                base
            } else {
                Style::new().fg(theme.menu_shortcut)
            };
            Line::from(vec![
                Span::styled(format!(" {mark}{}", item.label), base),
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

/// Whether a drop-down draws the mark column, and so how wide its rows are.
///
/// This has to answer the same for both states of every entry — otherwise the
/// popup would grow by two columns the moment something was switched on, and
/// the labels would step sideways under the cursor.
pub fn has_marks(app: &App, items: &[MenuEntry]) -> bool {
    items
        .iter()
        .filter_map(MenuEntry::item)
        .any(|item| menu_state(app, &item.command).is_some())
}

/// The two columns in front of a label: the mark, or the space it keeps.
fn mark_for(app: &App, item: &MenuItem) -> &'static str {
    match menu_state(app, &item.command) {
        Some(true) => "✓ ",
        Some(false) | None => "  ",
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::{Command, MenuEntry, MENUS};
    use crate::config::ThemeKind;
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

    /// The row of an open menu whose label contains `label`.
    fn row_with(app: &App, label: &str) -> String {
        popup_rows(app)
            .into_iter()
            .find(|row| row.contains(label))
            .unwrap_or_else(|| panic!("a row for {label}"))
    }

    /// Where `label` starts in `row`, counted in columns rather than in bytes:
    /// the border and the mark are both multi-byte.
    fn column_of(row: &str, label: &str) -> Option<usize> {
        let at = row.find(label)?;
        Some(row[..at].chars().count())
    }

    fn open(title: &str) -> usize {
        MENUS
            .iter()
            .position(|menu| menu.title == title)
            .unwrap_or_else(|| panic!("a {title} menu"))
    }

    /// An entry that switches something says which way it is pointing, rather
    /// than leaving the user to press it and read the status bar (ADR-066).
    #[test]
    fn a_switch_is_marked_while_it_is_on() {
        let mut app = App::fixture();
        app.menu.open = Some(open("View"));

        app.settings.word_wrap = false;
        assert!(!row_with(&app, "Word Wrap").contains('✓'));

        app.settings.word_wrap = true;
        assert!(row_with(&app, "Word Wrap").contains('✓'));
    }

    /// The mark is a column, not a prefix: an entry with no state keeps its
    /// label on the same edge as the ones that have one, whichever way they
    /// are pointing.
    #[test]
    fn the_labels_of_a_marked_menu_share_a_left_edge() {
        let mut app = App::fixture();
        app.menu.open = Some(open("View"));
        app.settings.word_wrap = true;

        let wrap = row_with(&app, "Word Wrap");
        let refresh = row_with(&app, "Refresh Explorer");
        assert_eq!(
            column_of(&wrap, "Word Wrap"),
            column_of(&refresh, "Refresh Explorer"),
            "{wrap}\n{refresh}"
        );
    }

    /// A greyed entry keeps its row and its label — what it loses is its
    /// colour (ADR-084). Checked as a colour rather than as a string, because
    /// the whole point is that the text is unchanged.
    #[test]
    fn a_git_entry_goes_grey_outside_a_repository() {
        use crate::app::git::{GitAvailability, GitState};

        let mut app = App::fixture();
        app.menu.open = Some(open("Git"));

        let colour = |app: &App, label: &str| {
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
            let row = (popup.y..popup.bottom())
                .find(|y| {
                    (popup.x..popup.right())
                        .map(|x| buffer[(x, *y)].symbol())
                        .collect::<String>()
                        .contains(label)
                })
                .unwrap_or_else(|| panic!("a row for {label}"));
            buffer[(popup.x + 2, row)].fg
        };

        // The fixture's panel is a repository, so Stage is live and
        // Initialize Repository is the one that is not.
        let theme = Theme::default();
        assert_ne!(colour(&app, "Stage All"), theme.menu_disabled);
        assert_eq!(colour(&app, "Initialize Repository"), theme.menu_disabled);

        app.git = GitState::default();
        app.git.availability = GitAvailability::NotARepository;
        assert_eq!(colour(&app, "Stage All"), theme.menu_disabled);
        assert_ne!(colour(&app, "Initialize Repository"), theme.menu_disabled);
        // Still there, still where it was: greying a row must not move the
        // rows around it.
        assert_eq!(
            column_of(&row_with(&app, "Stage All"), "Stage All"),
            Some(2)
        );
    }

    /// A menu with nothing to switch has no mark column at all: the Git
    /// entries keep the one padding column they always had.
    #[test]
    fn a_menu_without_a_switch_is_not_indented() {
        let mut app = App::fixture();
        app.menu.open = Some(open("Git"));
        assert_eq!(
            column_of(&row_with(&app, "Stage All"), "Stage All"),
            Some(2)
        );
    }

    /// The themes are a choice of one, and the one in use is the one the
    /// picker opens on and marks with git's own `*` (ADR-079).
    ///
    /// It used to be a mark against one of five menu entries. The menu entry is
    /// now a door, so what has to be checked is behind it.
    #[test]
    fn the_theme_picker_opens_on_the_live_theme() {
        use crate::app::dialog::DialogState;
        use crate::app::focus::FocusTarget;

        let dialog = DialogState::theme(ThemeKind::Retro, FocusTarget::Editor);
        assert_eq!(
            dialog.selected_item().map(|row| row.label.as_str()),
            Some(ThemeKind::Retro.label())
        );
        let marked: Vec<&str> = dialog
            .rows()
            .filter(|row| row.current)
            .map(|row| row.label.as_str())
            .collect();
        assert_eq!(marked, vec![ThemeKind::Retro.label()]);
    }

    /// Opening a menu must not resize it: the mark column is there whether or
    /// not anything in the menu is switched on.
    #[test]
    fn a_switch_does_not_move_the_labels_when_it_flips() {
        let mut app = App::fixture();
        app.menu.open = Some(open("View"));

        app.settings.word_wrap = false;
        let off = popup_rows(&app);
        app.settings.word_wrap = true;
        let on = popup_rows(&app);

        assert_eq!(off[0].chars().count(), on[0].chars().count());
        assert_eq!(
            column_of(&row_with(&app, "Refresh Explorer"), "Refresh Explorer"),
            off.iter()
                .find(|row| row.contains("Refresh Explorer"))
                .and_then(|row| column_of(row, "Refresh Explorer"))
        );
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
