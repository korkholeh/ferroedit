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
use unicode_segmentation::UnicodeSegmentation;

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
            let base = kind_style(theme, line.kind);
            Line::from(visible_spans(
                &line.text,
                line.changed,
                word_style(theme, line.kind),
                base,
                left,
                width,
                DEFAULT_TAB_WIDTH,
            ))
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// The ground the changed part of a replaced line is drawn on (ADR-082).
///
/// `None` for every kind that is not one half of a replacement: a context line
/// has nothing to mark, and a header that acquired a range would be a parse
/// error rather than a change.
fn word_style(theme: &Theme, kind: DiffLineKind) -> Option<Style> {
    match kind {
        DiffLineKind::Added => Some(theme.diff_word_added),
        DiffLineKind::Removed => Some(theme.diff_word_removed),
        _ => None,
    }
}

/// The visible part of a line, as one span per run of the same style.
///
/// The changed range is a byte range into the *whole* line, prefix included, so
/// it is tested against each cluster's own byte offset — which means the
/// horizontal window can start or end inside a marked run and the run keeps its
/// ground for exactly the cells it covers. A cluster that straddles the
/// window's edge contributes spaces, as it always did, and those spaces belong
/// to whichever run the cluster is in: a tab inside a changed range is part of
/// the change.
fn visible_spans(
    line: &str,
    changed: Option<(usize, usize)>,
    changed_style: Option<Style>,
    base: Style,
    left: VisualCol,
    width: usize,
    tab_width: usize,
) -> Vec<Span<'static>> {
    let marked = changed.zip(changed_style);
    let right = left.0 + width;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_marked = false;
    let mut col = 0;

    let mut flush = |run: &mut String, was_marked: bool| {
        if run.is_empty() {
            return;
        }
        let style = match (was_marked, marked) {
            (true, Some((_, style))) => base.patch(style),
            _ => base,
        };
        spans.push(Span::styled(std::mem::take(run), style));
    };

    for (at, cluster) in line.grapheme_indices(true) {
        if col >= right {
            break;
        }
        let cluster_width = coords::cluster_width(cluster, VisualCol(col), tab_width);
        let end = col + cluster_width;
        let here = marked.is_some_and(|((from, to), _)| at >= from && at < to);
        if end > left.0 {
            if here != run_marked {
                flush(&mut run, run_marked);
                run_marked = here;
            }
            if cluster == "\t" || col < left.0 || end > right {
                for _ in col.max(left.0)..end.min(right) {
                    run.push(' ');
                }
            } else {
                run.push_str(cluster);
            }
        }
        col = end;
    }
    flush(&mut run, run_marked);
    spans
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
    use crate::config::ThemeKind;

    const TAB: usize = DEFAULT_TAB_WIDTH;

    /// The text of the drawn line, whatever it was cut into.
    ///
    /// Every test about the *window* asks this: a change to how the line is
    /// split into spans must not change which cells it occupies.
    fn visible_slice(line: &str, left: VisualCol, width: usize, tab_width: usize) -> String {
        visible_spans(line, None, None, Style::new(), left, width, tab_width)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// The drawn line as `(text, is marked)` runs.
    fn runs(
        line: &str,
        changed: Option<(usize, usize)>,
        left: VisualCol,
        width: usize,
    ) -> Vec<(String, bool)> {
        let theme = Theme::new(ThemeKind::Dark);
        let base = kind_style(&theme, DiffLineKind::Added);
        visible_spans(
            line,
            changed,
            Some(theme.diff_word_added),
            base,
            left,
            width,
            TAB,
        )
        .into_iter()
        .map(|span| {
            let marked = span.style.bg == theme.diff_word_added.bg;
            (span.content.into_owned(), marked)
        })
        .collect()
    }

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
    /// The changed part of a replaced line is marked and the rest is not
    /// (ADR-082).
    #[test]
    fn only_the_changed_part_of_a_line_is_marked() {
        let line = "+    width.saturating_sub(GAP) as usize";
        let from = line.find("saturating_sub(GAP)").unwrap();
        let to = from + "saturating_sub(GAP)".len();
        assert_eq!(
            runs(line, Some((from, to)), VisualCol(0), 80),
            vec![
                ("+    width.".to_string(), false),
                ("saturating_sub(GAP)".to_string(), true),
                (" as usize".to_string(), false),
            ]
        );
    }

    /// A line with no partner is one run, in the colour it always had.
    #[test]
    fn an_unpaired_line_is_one_unmarked_run() {
        assert_eq!(
            runs("+added", None, VisualCol(0), 40),
            vec![("+added".to_string(), false)]
        );
    }

    /// The horizontal window can cut a marked run at either end, and the run
    /// keeps its ground for exactly the cells it still covers.
    #[test]
    fn a_mark_survives_being_scrolled_half_off_the_pane() {
        // `+abcdefgh`, with `cde` changed: bytes 3..6 of the line.
        let line = "+abcdefgh";
        let marked = Some((3usize, 6usize));
        assert_eq!(
            runs(line, marked, VisualCol(0), 5),
            vec![("+ab".to_string(), false), ("cd".to_string(), true)]
        );
        // Column four is `d`, so what is left of the mark is `de`.
        assert_eq!(
            runs(line, marked, VisualCol(4), 5),
            vec![("de".to_string(), true), ("fgh".to_string(), false)]
        );
    }

    /// A tab inside a marked range is part of the change, and the cells it
    /// expands to carry the mark.
    #[test]
    fn a_tab_inside_a_change_is_marked_across_the_cells_it_fills() {
        let line = "+a\tb";
        // The tab alone: byte 2..3.
        let drawn = runs(line, Some((2, 3)), VisualCol(0), 20);
        assert_eq!(drawn[0], ("+a".to_string(), false));
        assert!(drawn[1].1, "the tab is inside the change");
        assert_eq!(drawn[1].0.len(), TAB - 2);
        assert_eq!(drawn[2], ("b".to_string(), false));
    }
}
