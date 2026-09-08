//! UI: the read-only diff viewer (SPEC §36).
//!
//! The editor pane, drawn for a tab that holds a diff instead of a document:
//! one line per line of `git diff`, coloured by what each line already is. The
//! classification happened once, when the diff was read (ARCHITECTURE
//! invariant 4), so this file decides colours and columns and nothing else.
//!
//! Long lines scroll sideways rather than wrapping. A wrapped diff loses the
//! one thing a diff has going for it — that the `+` and the `−` line up down
//! the left edge — and a minified line would push the rest of a hunk off the
//! bottom of the pane.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::App;
use crate::editor::coords::{self, VisualCol, DEFAULT_TAB_WIDTH};
use crate::git::diff::DiffLineKind;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(viewer) = app.diff() else {
        return;
    };
    let focused = app.focus == FocusTarget::Diff;
    // The editor is drawn underneath, so the ground has to be taken back
    // before anything is written on it.
    frame.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.border_for(focused))
        .style(Style::new().bg(theme.background))
        .title(Span::styled(viewer.title(), theme.panel_title))
        .title_bottom(
            Line::from(Span::styled(viewer.position(), Style::new().fg(theme.dim))).right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let width = inner.width as usize;
    let left = VisualCol(viewer.h_scroll);
    let lines: Vec<Line> = viewer
        .diff
        .lines
        .iter()
        .skip(viewer.scroll)
        .take(inner.height as usize)
        .map(|line| {
            Line::from(Span::styled(
                visible_slice(&line.text, left, width, DEFAULT_TAB_WIDTH),
                kind_style(theme, line.kind),
            ))
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// The part of a line inside the horizontal window, with tabs expanded.
///
/// Cells rather than characters, the same rule the editor's own renderer
/// follows: a wide character or a tab straddling an edge contributes spaces
/// for the cells that are inside it, so the text after it keeps the column it
/// belongs in instead of sliding sideways by one.
fn visible_slice(line: &str, left: VisualCol, width: usize, tab_width: usize) -> String {
    let right = left.0 + width;
    let mut out = String::new();
    let mut col = 0;
    for (_, cluster) in coords::clusters(line) {
        if col >= right {
            break;
        }
        let cluster_width = coords::cluster_width(cluster, VisualCol(col), tab_width);
        let end = col + cluster_width;
        if end > left.0 {
            if cluster == "\t" || col < left.0 || end > right {
                for _ in col.max(left.0)..end.min(right) {
                    out.push(' ');
                }
            } else {
                out.push_str(cluster);
            }
        }
        col = end;
    }
    out
}

fn kind_style(theme: &Theme, kind: DiffLineKind) -> Style {
    match kind {
        DiffLineKind::Added => Style::new().fg(theme.git_added),
        DiffLineKind::Removed => Style::new().fg(theme.git_deleted),
        DiffLineKind::Hunk => Style::new().fg(theme.diff_hunk),
        DiffLineKind::Header | DiffLineKind::Meta => Style::new().fg(theme.dim),
        DiffLineKind::Context => Style::new().fg(theme.foreground),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: usize = DEFAULT_TAB_WIDTH;

    #[test]
    fn a_line_narrower_than_the_pane_is_drawn_whole() {
        assert_eq!(visible_slice("+added", VisualCol(0), 40, TAB), "+added");
    }

    #[test]
    fn the_window_clips_at_both_edges() {
        assert_eq!(visible_slice("+abcdefgh", VisualCol(2), 3, TAB), "bcd");
        assert_eq!(visible_slice("+ab", VisualCol(9), 3, TAB), "");
    }

    #[test]
    fn a_tab_is_drawn_as_the_cells_it_occupies() {
        // `+` then a tab, which advances from column one to the next stop.
        assert_eq!(
            visible_slice("+\tx", VisualCol(0), 10, TAB),
            format!("+{}x", " ".repeat(TAB - 1))
        );
    }

    #[test]
    fn a_wide_character_split_by_an_edge_keeps_the_columns_aligned() {
        // `日` is two cells; a window starting inside it gets one space, so
        // the character after it stays where it was.
        assert_eq!(visible_slice(" 日本", VisualCol(2), 3, TAB), " 本");
    }
}
