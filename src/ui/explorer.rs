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
    let name = app.workspace.name();
    // One column of the pane is its right border, and the title is drawn on
    // the row above the rows, so the room it has is the pane less that border.
    let room = (width as usize).saturating_sub(1);
    let for_name = room.saturating_sub(CHROME);
    if for_name == 0 {
        return " Files ".to_string();
    }
    format!(" Files — {} ", crate::ui::git::elide_left(name, for_name))
}

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let focused = app.focus == FocusTarget::Explorer;
    let block = Block::new()
        .borders(Borders::RIGHT)
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
            let line = Line::from(vec![
                // One column of padding, then two per nesting level.
                Span::raw(" ".repeat(1 + row.depth as usize * 2)),
                Span::raw(marker),
                Span::styled(row.name.clone(), name_style),
            ]);
            if index == app.sidebar.selected {
                line.style(selection_style(theme, focused))
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
