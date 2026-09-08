//! UI: the vertical scrollbar every scrolling pane draws.
//!
//! One function rather than three copies of the same `ScrollbarState` arithmetic:
//! the explorer, the git panel and the editor all scroll a list of rows past a
//! window of `visible` of them, and the only thing that differs is which column
//! the bar lands in (ADR-052).
//!
//! The bar is drawn only when there *is* something to scroll. A full-height
//! thumb is a control that lies about having something to do, and in the two
//! sidebar panels the track sits on the pane's own border — so a pane that fits
//! keeps an unbroken `│` down its edge instead of a second vertical line.

use ratatui::layout::Rect;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

use crate::ui::theme::Theme;

/// Draws a vertical scrollbar in `track` — a one-cell-wide column as tall as
/// the rows it describes.
///
/// `total` is how many rows the pane has, `visible` how many of them fit, and
/// `offset` the index of the first one drawn. An `offset` past the last
/// scrollable position pins the thumb at the bottom rather than wrapping: the
/// editor may scroll until only its last line is left, and "you are at the end"
/// is what that state means.
pub fn render(
    frame: &mut Frame,
    track: Rect,
    theme: &Theme,
    focused: bool,
    total: usize,
    visible: usize,
    offset: usize,
) {
    if track.width == 0 || track.height == 0 || visible == 0 || total <= visible {
        return;
    }
    let last = total - visible;
    let mut state = ScrollbarState::new(last)
        .position(offset.min(last))
        .viewport_content_length(visible);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            // No arrow caps: they cost the two rows a short pane can least
            // afford, and nothing in the app clicks them.
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            // The thumb stays visible in an unfocused pane — it is a readout of
            // where the pane is, not a focus ring — but the focused pane's is
            // the one that draws the eye.
            .thumb_style(if focused {
                theme.border_for(true)
            } else {
                ratatui::style::Style::new().fg(theme.dim)
            })
            .track_style(theme.border_for(false)),
        track,
        &mut state,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// The scrollbar column as one string per row, `.` for a blank cell.
    fn column(total: usize, visible: usize, offset: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(1, visible as u16)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &Theme::default(), true, total, visible, offset);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..visible as u16)
            .map(|y| match buffer[(0, y)].symbol() {
                " " => '.',
                symbol => symbol.chars().next().unwrap(),
            })
            .collect()
    }

    #[test]
    fn a_pane_that_fits_draws_nothing() {
        assert_eq!(column(4, 4, 0), "....");
        assert_eq!(column(1, 8, 0), "........");
    }

    #[test]
    fn the_thumb_starts_at_the_top_and_reaches_the_bottom() {
        let top = column(40, 8, 0);
        assert!(top.starts_with('█'), "{top:?}");
        let bottom = column(40, 8, 32);
        assert!(bottom.ends_with('█'), "{bottom:?}");
    }

    #[test]
    fn scrolling_past_the_last_page_leaves_the_thumb_at_the_bottom() {
        // The editor scrolls until only its last line is on screen, which is
        // past the last position a list scrolls to.
        let past = column(40, 8, 39);
        assert!(past.ends_with('█'), "{past:?}");
    }

    #[test]
    fn the_track_fills_the_rest_of_the_column() {
        assert!(column(40, 8, 0).contains('│'));
    }
}
