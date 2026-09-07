//! UI: explorer rendering.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::App;
use crate::filesystem::tree::EntryKind;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let focused = app.focus == FocusTarget::Explorer;
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(theme.border_for(focused))
        .title(Span::styled(
            format!(" {} ", app.workspace.name()),
            theme.panel_title,
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let rows = app.sidebar.rows();
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
