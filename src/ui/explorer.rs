//! UI: explorer rendering.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::App;
use crate::filesystem::tree::EntryKind;
use crate::ui::scrollbar;
use crate::ui::theme::Theme;

/// ` Files — ferroedit `, with the folder name cut from the left when the
/// sidebar is too narrow for both halves.
///
/// "Files" rather than the folder name alone: the sidebar has two panes, and
/// the one below it says "Git" — a title that named only the directory left
/// the top pane as the one that did not say what it was. It is the half that
/// survives a narrow sidebar, too, because it is the half that is the same
/// width in every workspace.
fn title(app: &App, width: u16) -> String {
    /// ` Files — ` and the trailing space: everything in the title that is
    /// not the folder's name.
    const CHROME: usize = 10;
    /// Cells of rule kept to the right of the title. The row is the pane's top
    /// border (ADR-072), and a title elided to fill every cell of it hides the
    /// line it is drawn on — which is the line that keeps the tree off the menu
    /// bar.
    const RULE: usize = 3;
    let name = app.workspace.name();
    // One column of the pane is its right border, and the title is drawn on
    // the row above the rows, so the room it has is the pane less that border.
    let room = (width as usize).saturating_sub(1);
    let for_name = room.saturating_sub(CHROME + RULE);
    if for_name == 0 {
        return " Files ".to_string();
    }
    format!(" Files — {} ", crate::ui::git::elide_left(name, for_name))
}

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let focused = app.focus == FocusTarget::Explorer;
    let block = Block::new()
        // A top border as well as the right one, so the tree is not sitting
        // directly against the menu bar: the pane above it is the menu, and
        // without the rule the two ran together (ADR-072). The git panel below
        // has drawn its own top rule all along, so this makes the sidebar
        // consistent rather than adding a new idea.
        .borders(Borders::TOP | Borders::RIGHT)
        .border_style(theme.border_for(focused))
        .title(Span::styled(title(app, area.width), theme.panel_title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let rows = app.sidebar.rows();
    // Down the border the pane already draws, so the tree keeps every cell of
    // its width — a sidebar is sixteen cells at its narrowest, and a name is
    // what it is there to show (ADR-052).
    scrollbar::render(
        frame,
        Rect::new(area.right() - 1, inner.y, 1, inner.height),
        theme,
        focused,
        rows.len(),
        inner.height as usize,
        app.sidebar.scroll,
    );
    if rows.is_empty() {
        // Short enough for a 16-cell sidebar: the panel is the one place in the
        // layout that gives up width first.
        let message = if app.sidebar.tree.show_hidden() {
            "(empty)"
        } else {
            "(empty or ignored)"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {message}"),
                Style::new().fg(theme.dim),
            ))),
            inner,
        );
        return;
    }

    // The file in front of the user is marked in the tree (SPEC §18), which is
    // what keeps "which of these am I editing" from being a question.
    let active = app.active().and_then(|tab| tab.document.path());

    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(app.sidebar.scroll)
        .take(inner.height as usize)
        .map(|(index, row)| {
            let (marker, mut name_style) = match row.kind {
                EntryKind::Directory if row.expanded => ("▼ ", Style::new().fg(theme.directory)),
                EntryKind::Directory => ("▶ ", Style::new().fg(theme.directory)),
                EntryKind::File => ("  ", Style::new().fg(theme.foreground)),
            };
            if active == Some(row.path.as_path()) {
                name_style = name_style.add_modifier(Modifier::BOLD);
            }
            let selected = index == app.sidebar.selected;
            let selection = selection_style(theme, focused);
            if selected {
                // A span's own foreground wins over the line's, so a row drawn
                // on the selection has to be *told* the selection's text
                // colour: the light theme paints directories in the same blue
                // it highlights with, and the name vanished into its own
                // background. A selection with no colour of its own — the
                // unfocused one — leaves the name the colour it had.
                if let Some(fg) = selection.fg {
                    name_style = name_style.fg(fg);
                }
            }
            let line = Line::from(vec![
                // One column of padding, then two per nesting level.
                Span::raw(" ".repeat(1 + row.depth as usize * 2)),
                Span::styled(marker, name_style),
                Span::styled(row.name.clone(), name_style),
            ]);
            if selected {
                line.style(selection)
            } else {
                line
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// A selection in an unfocused pane stays visible but stops competing with the
/// focused pane's for attention.
pub fn selection_style(theme: &Theme, focused: bool) -> Style {
    if focused {
        theme.selection
    } else {
        theme.selection_unfocused
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn the_title_names_the_pane_and_the_folder() {
        let app = App::fixture();
        assert_eq!(title(&app, 40), " Files — ferroedit-test ");
    }

    /// A selected row's name is drawn in the selection's own text colour, not
    /// in the colour it wears on the ground.
    ///
    /// The light theme highlights in the same blue it paints directories in,
    /// and a span's foreground wins over the line's: the selected folder was
    /// its own background, and all that was left of the row was the ▼.
    #[test]
    fn a_selected_row_is_not_drawn_in_the_colour_under_it() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let mut app = App::fixture_in(dir.path());
        app.focus = FocusTarget::Explorer;
        app.sidebar.selected = app
            .sidebar
            .rows()
            .iter()
            .position(|row| row.name == "src")
            .expect("the folder is in the tree");

        for kind in crate::config::ThemeKind::ALL.iter().copied() {
            let theme = Theme::new(kind);
            let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
            terminal
                .draw(|frame| render(frame, &app, frame.area(), &theme))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let row = (0..10)
                .find(|y| {
                    (0..30)
                        .map(|x| buffer[(x, *y)].symbol())
                        .collect::<String>()
                        .contains("src")
                })
                .expect("the selected folder is on screen");
            for x in 0..30 {
                let cell = &buffer[(x, row)];
                if cell.symbol().trim().is_empty() {
                    continue;
                }
                assert_ne!(
                    cell.fg,
                    cell.bg,
                    "{}: {:?} at {x} is invisible on the selection",
                    kind.label(),
                    cell.symbol()
                );
            }
        }
    }

    /// A sidebar too narrow for both keeps "Files" and cuts the folder name
    /// from the left, which is the end that says which folder it is.
    #[test]
    fn a_narrow_sidebar_keeps_the_word_and_elides_the_folder() {
        let app = App::fixture();
        let narrow = title(&app, 16);
        assert!(narrow.starts_with(" Files — "), "{narrow:?}");
        assert!(narrow.contains('…'), "{narrow:?}");
        assert!(narrow.width() <= 15, "{narrow:?}");
        // Narrower still: the folder name goes rather than the pane's name.
        assert_eq!(title(&app, 8), " Files ");
    }
}
