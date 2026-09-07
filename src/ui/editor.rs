//! UI: editor rendering.
//!
//! Paints the slice of the document the tab's viewport points at, and puts the
//! terminal's own cursor where the document's cursor is. Read-only over `&App`
//! like the rest of `ui/`: the scrolling that makes the cursor visible has
//! already happened in `execute_command`.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::App;
use crate::editor::coords::{self, VisualCol};
use crate::editor::search::Match;
use crate::editor::selection::Selection;
use crate::editor::viewport::gutter_width;
use crate::syntax::highlighter::{self, StyleKind, Token};
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(tab) = app.active() else {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No file open",
            Style::new().fg(theme.dim),
        )));
        frame.render_widget(empty, area);
        return;
    };

    let document = &tab.document;
    let tab_width = document.tab_width();
    let gutter = gutter_width(document.line_count());
    let text_width = (area.width as usize).saturating_sub(gutter);
    let top = tab.viewport.top_line;
    let left = tab.viewport.left_col;

    let selection = document.selection();
    let hits = app.search.open.then_some(app.search.matches.as_slice());
    let mut columns = Vec::new();
    let lines: Vec<Line> = (top..document.line_count())
        .take(area.height as usize)
        .map(|index| {
            let text = document.line(index);
            let mut spans = vec![Span::styled(
                format!("{:>width$}  ", index + 1, width = gutter - 2),
                Style::new().fg(theme.line_number),
            )];
            match_columns(hits, index, &text, tab_width, &mut columns);
            spans.extend(visible_spans(
                &text,
                left,
                text_width,
                tab_width,
                selected_columns(selection, index, &text, tab_width),
                &columns,
                theme,
                tab.highlights.tokens(index),
            ));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);

    // The terminal's real cursor is the caret: it blinks the way the user's
    // terminal is configured to blink, and screen readers follow it. It is only
    // shown while the editor has focus, so the focused pane is unambiguous.
    if app.focus != FocusTarget::Editor {
        return;
    }
    let cursor = document.cursor();
    let col = document.cursor_visual_col();
    let row = cursor.line.checked_sub(top);
    let cell = col.0.checked_sub(left.0);
    if let (Some(row), Some(cell)) = (row, cell) {
        if row < area.height as usize && cell < text_width {
            frame.set_cursor_position((area.x + (gutter + cell) as u16, area.y + row as u16));
        }
    }
}

/// The display columns of `line` covered by the selection, if any.
///
/// A line whose newline is inside the selection is highlighted one cell past
/// its last character: that empty cell is how the user sees that the line break
/// itself was taken, which is what makes a multi-line selection look like one
/// block rather than a set of ragged fragments.
fn selected_columns(
    selection: Option<Selection>,
    line_index: usize,
    line: &str,
    tab_width: usize,
) -> Option<(VisualCol, VisualCol)> {
    let span = selection?.line_span(line_index, coords::char_len(line))?;
    let start = coords::visual_col(line, span.start, tab_width);
    let end = if span.to_line_end {
        VisualCol(coords::line_width(line, tab_width).0 + 1)
    } else {
        coords::visual_col(line, span.end, tab_width)
    };
    (end > start).then_some((start, end))
}

/// What is drawn behind a cell.
///
/// Ordered by which wins: the current hit *is* the selection, so a cell that is
/// both is drawn as selected, and the other hits stay distinguishable from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behind {
    Nothing,
    /// A search hit that is not the current one.
    Hit,
    Selected,
}

/// What decides a run's colour: the syntax kind under it, and what is behind
/// it. Two adjacent cells that agree on both are one span.
type Paint = (StyleKind, Behind);

/// The display columns of `line` covered by search hits.
///
/// Written into a buffer the caller reuses: this runs once per visible row per
/// frame, and a fresh vector for each of them would be forty allocations a
/// keystroke for a list that is almost always empty.
fn match_columns(
    hits: Option<&[Match]>,
    line_index: usize,
    line: &str,
    tab_width: usize,
    out: &mut Vec<(VisualCol, VisualCol)>,
) {
    out.clear();
    let Some(hits) = hits else { return };
    // The hits are in reading order, so the ones on this line are a contiguous
    // run and a binary search finds its start — a five-megabyte file can have
    // ten thousand of them, and the viewport is forty rows.
    let from = hits.partition_point(|hit| hit.line < line_index);
    for hit in hits[from..].iter().take_while(|hit| hit.line == line_index) {
        let start = coords::visual_col(line, hit.start, tab_width);
        let end = coords::visual_col(line, hit.end, tab_width);
        if end > start {
            out.push((start, end));
        }
    }
}

/// The part of a line that falls inside the horizontal viewport, with tabs
/// expanded, split into a styled span per run of same-looking cells.
///
/// Clusters are the unit throughout: a wide character or a tab straddling
/// either edge of the window contributes spaces for the cells that are inside
/// it, so the text after it stays in the column it belongs in instead of
/// sliding by a cell. Selection is decided per *cell* rather than per cluster,
/// so a selection that ends inside a tab highlights only the cells it reaches.
///
/// `tokens` are the highlighter's runs for this line, in byte offsets — empty
/// when the line is not being highlighted, which paints everything as `Text`.
fn visible_spans(
    line: &str,
    left: VisualCol,
    width: usize,
    tab_width: usize,
    selected: Option<(VisualCol, VisualCol)>,
    hits: &[(VisualCol, VisualCol)],
    theme: &Theme,
    tokens: &[Token],
) -> Vec<Span<'static>> {
    let mut runs: Vec<(String, Paint)> = Vec::new();
    let mut push = |text: &str, paint: Paint| {
        if text.is_empty() {
            return;
        }
        match runs.last_mut() {
            Some((run, last)) if *last == paint => run.push_str(text),
            _ => runs.push((text.to_string(), paint)),
        }
    };

    let right = left.0 + width;
    let mut col = 0;
    // Byte offset of the current cluster, which is how the highlighter's runs
    // are indexed — char and byte indices part company on the first non-ASCII
    // character, and syntect counts bytes.
    let mut byte = 0;
    for (_, cluster) in coords::clusters(line) {
        if col >= right {
            break;
        }
        let kind = highlighter::kind_at(tokens, byte);
        byte += cluster.len();
        let cluster_width = coords::cluster_width(cluster, VisualCol(col), tab_width);
        let end = col + cluster_width;
        if end > left.0 {
            let from = col.max(left.0);
            let to = end.min(right);
            let behind = |at: usize| {
                if selected.is_some_and(|(s, e)| at >= s.0 && at < e.0) {
                    Behind::Selected
                } else if hits.iter().any(|(s, e)| at >= s.0 && at < e.0) {
                    Behind::Hit
                } else {
                    Behind::Nothing
                }
            };
            if cluster == "\t" || col < left.0 || end > right {
                // Partially visible, or elastic: draw its cells, not its text —
                // and each cell can fall on a different side of the selection.
                for cell in from..to {
                    push(" ", (kind, behind(cell)));
                }
            } else {
                push(cluster, (kind, behind(col)));
            }
        }
        col = end;
    }

    // The cell standing in for a selected newline sits past the text.
    if let Some((_, end)) = selected {
        for _ in col.max(left.0)..end.0.min(right) {
            push(" ", (StyleKind::Text, Behind::Selected));
        }
    }

    runs.into_iter()
        .map(|(text, paint)| Span::styled(text, paint_style(theme, paint)))
        .collect()
}

/// The style of one run.
///
/// The background is patched *over* the syntax colour and sets only a
/// background (Phase 3), so highlighted code keeps its own colours instead of
/// flattening into one block — which is what makes both a selection and a
/// search hit readable.
fn paint_style(theme: &Theme, (kind, behind): Paint) -> Style {
    let style = theme.syntax.style(kind);
    match behind {
        Behind::Nothing => style,
        Behind::Hit => style.patch(theme.search_match),
        Behind::Selected => style.patch(theme.editor_selection),
    }
}

/// The visible text of a line as one string — what the spans add up to.
#[cfg(test)]
fn visible_slice(line: &str, left: VisualCol, width: usize, tab_width: usize) -> String {
    visible_spans(
        line,
        left,
        width,
        tab_width,
        None,
        &[],
        &Theme::default(),
        &[],
    )
    .into_iter()
    .map(|span| span.content.into_owned())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::coords::DEFAULT_TAB_WIDTH as TAB;

    #[test]
    fn a_line_shorter_than_the_window_is_drawn_whole() {
        assert_eq!(
            visible_slice("fn main() {", VisualCol(0), 40, TAB),
            "fn main() {"
        );
    }

    #[test]
    fn the_window_clips_at_both_edges() {
        assert_eq!(visible_slice("abcdefgh", VisualCol(2), 3, TAB), "cde");
        assert_eq!(visible_slice("abcdefgh", VisualCol(6), 10, TAB), "gh");
        assert_eq!(visible_slice("abcdefgh", VisualCol(99), 10, TAB), "");
    }

    #[test]
    fn tabs_are_drawn_as_the_cells_they_occupy() {
        assert_eq!(visible_slice("\tx", VisualCol(0), 10, TAB), "    x");
        // Scrolled into the middle of the tab: only its remaining cells show.
        assert_eq!(visible_slice("\tx", VisualCol(2), 10, TAB), "  x");
    }

    #[test]
    fn a_wide_character_split_by_an_edge_keeps_the_columns_aligned() {
        // 日 spans columns 0..2, so scrolling to column 1 leaves one cell of it.
        assert_eq!(visible_slice("日本", VisualCol(1), 10, TAB), " 本");
        // Clipped at the right edge instead.
        assert_eq!(visible_slice("日本", VisualCol(0), 3, TAB), "日 ");
    }

    #[test]
    fn combining_marks_travel_with_their_base_character() {
        assert_eq!(
            visible_slice("e\u{301}x", VisualCol(0), 10, TAB),
            "e\u{301}x"
        );
        assert_eq!(visible_slice("e\u{301}x", VisualCol(1), 10, TAB), "x");
    }

    /// The visible spans as `(text, selected)` pairs, which is what the
    /// selection tests are actually about.
    fn runs(
        line: &str,
        left: VisualCol,
        width: usize,
        selected: Option<(usize, usize)>,
    ) -> Vec<(String, bool)> {
        let theme = Theme::default();
        visible_spans(
            line,
            left,
            width,
            TAB,
            selected.map(|(a, b)| (VisualCol(a), VisualCol(b))),
            &[],
            &theme,
            &[],
        )
        .into_iter()
        .map(|span| {
            let is_selected = span.style.bg == theme.editor_selection.bg;
            (span.content.into_owned(), is_selected)
        })
        .collect()
    }

    #[test]
    fn an_unselected_line_is_one_span() {
        assert_eq!(
            runs("fn main()", VisualCol(0), 40, None),
            vec![("fn main()".to_string(), false)]
        );
    }

    #[test]
    fn a_selection_splits_the_line_into_styled_runs() {
        assert_eq!(
            runs("fn main()", VisualCol(0), 40, Some((3, 7))),
            vec![
                ("fn ".to_string(), false),
                ("main".to_string(), true),
                ("()".to_string(), false),
            ]
        );
    }

    #[test]
    fn a_selected_wide_character_is_highlighted_whole() {
        // 日 spans columns 0..2 and 本 columns 2..4: selecting 2..4 must take
        // 本 and nothing of its neighbour.
        assert_eq!(
            runs("日本語", VisualCol(0), 40, Some((2, 4))),
            vec![
                ("日".to_string(), false),
                ("本".to_string(), true),
                ("語".to_string(), false),
            ]
        );
    }

    #[test]
    fn a_selection_ending_inside_a_tab_highlights_only_the_cells_it_reaches() {
        // The tab occupies four cells; the selection stops after two of them.
        assert_eq!(
            runs("\tx", VisualCol(0), 10, Some((0, 2))),
            vec![("  ".to_string(), true), ("  x".to_string(), false),]
        );
    }

    #[test]
    fn a_selected_newline_shows_as_one_cell_past_the_text() {
        assert_eq!(
            runs("ab", VisualCol(0), 10, Some((0, 3))),
            vec![("ab ".to_string(), true)],
            "the trailing cell is the line break itself"
        );
    }

    #[test]
    fn a_selection_scrolled_off_the_left_edge_keeps_its_columns() {
        assert_eq!(
            runs("abcdefgh", VisualCol(3), 3, Some((2, 5))),
            vec![("de".to_string(), true), ("f".to_string(), false)]
        );
    }

    #[test]
    fn selected_columns_cover_a_whole_middle_line_and_its_break() {
        let selection = Selection::new(
            crate::editor::selection::Position::new(0, coords::CharIdx(1)),
            crate::editor::selection::Position::new(2, coords::CharIdx(1)),
        );
        // The middle line is selected end to end, plus a cell for the newline.
        assert_eq!(
            selected_columns(Some(selection), 1, "middle", TAB),
            Some((VisualCol(0), VisualCol(7)))
        );
        // The last line stops at the head, with no extra cell.
        assert_eq!(
            selected_columns(Some(selection), 2, "last", TAB),
            Some((VisualCol(0), VisualCol(1)))
        );
        assert_eq!(selected_columns(Some(selection), 3, "after", TAB), None);
        assert_eq!(selected_columns(None, 1, "middle", TAB), None);
    }

    /// The visible runs as `(text, kind)` — what a highlighted line looks like.
    fn painted(line: &str, tokens: &[Token]) -> Vec<(String, StyleKind)> {
        let theme = Theme::default();
        visible_spans(line, VisualCol(0), 80, TAB, None, &[], &theme, tokens)
            .into_iter()
            .map(|span| {
                let kind = [
                    StyleKind::Text,
                    StyleKind::Comment,
                    StyleKind::String,
                    StyleKind::Keyword,
                    StyleKind::Number,
                ]
                .into_iter()
                .find(|kind| theme.syntax.style(*kind).fg == span.style.fg)
                .expect("the span's colour is one of the kinds this test uses");
                (span.content.into_owned(), kind)
            })
            .collect()
    }

    #[test]
    fn a_run_that_ends_inside_a_multibyte_line_splits_on_the_right_character() {
        // The highlighter counts bytes and the renderer walks clusters, so a
        // line where the two disagree is the case that catches a mix-up: each
        // Cyrillic character here is two bytes wide.
        let line = "let s = \"Привіт\";";
        let string_start = line.find('"').unwrap();
        let string_end = line.rfind('"').unwrap() + 1;
        let tokens = [
            Token {
                end: 3,
                kind: StyleKind::Keyword,
            },
            Token {
                end: string_start,
                kind: StyleKind::Text,
            },
            Token {
                end: string_end,
                kind: StyleKind::String,
            },
            Token {
                end: line.len(),
                kind: StyleKind::Text,
            },
        ];
        assert_eq!(
            painted(line, &tokens),
            vec![
                ("let".into(), StyleKind::Keyword),
                (" s = ".into(), StyleKind::Text),
                ("\"Привіт\"".into(), StyleKind::String),
                (";".into(), StyleKind::Text),
            ]
        );
    }

    #[test]
    fn a_wide_character_takes_the_colour_of_the_run_it_starts_in() {
        // 日 is three bytes and two cells; 本 starts at byte 3.
        let line = "日本";
        let tokens = [
            Token {
                end: 3,
                kind: StyleKind::Keyword,
            },
            Token {
                end: 6,
                kind: StyleKind::Number,
            },
        ];
        assert_eq!(
            painted(line, &tokens),
            vec![
                ("日".into(), StyleKind::Keyword),
                ("本".into(), StyleKind::Number),
            ]
        );
    }

    #[test]
    fn a_line_with_no_tokens_is_drawn_as_plain_text() {
        assert_eq!(
            painted("fn main()", &[]),
            vec![("fn main()".into(), StyleKind::Text)]
        );
    }

    #[test]
    fn a_zero_width_window_draws_nothing() {
        assert_eq!(visible_slice("abc", VisualCol(0), 0, TAB), "");
    }
}
