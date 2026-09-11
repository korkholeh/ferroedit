//! UI: the row of keys on a read-only pane's bottom border (ADR-071).
//!
//! The log viewer's own keys are bare letters, which the menu deliberately does
//! not advertise, so the pane says what they are itself. The image viewer's are
//! the same kind of key and now say the same way (ADR-080), which is what this
//! module is: one legend, two panes, so the two cannot drift into different
//! shapes for the same idea.
//!
//! Every label is read out of the keymap by the caller rather than written
//! here, so a legend can never advertise a key that is not bound — the rule the
//! menu's shortcut column follows (SPEC §25, ADR-028).

use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::ui::theme::Theme;

/// What separates two pieces of a legend.
const SEPARATOR: &str = "  ·  ";

/// One piece: the key, and what pressing it does.
pub type Piece = (&'static str, &'static str);

/// The legend as spans, with the pieces that do not fit dropped from the end.
///
/// From the end, so a narrow terminal keeps the first and most useful keys
/// rather than losing the line altogether; a piece whose key is empty — a
/// command nothing is bound to — is left out before anything is measured.
pub fn spans(pieces: &[Piece], room: usize, theme: &Theme) -> Vec<Span<'static>> {
    let mut pieces: Vec<Piece> = pieces
        .iter()
        .copied()
        .filter(|(key, _)| !key.is_empty())
        .collect();
    while !pieces.is_empty() && width(&pieces) > room {
        pieces.pop();
    }
    if pieces.is_empty() {
        return Vec::new();
    }

    let mut spans = vec![Span::raw(" ")];
    for (index, (key, what)) in pieces.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(
                SEPARATOR,
                ratatui::style::Style::new().fg(theme.dim),
            ));
        }
        spans.push(Span::styled(
            key,
            ratatui::style::Style::new().fg(theme.diff_hunk),
        ));
        spans.push(Span::styled(
            format!(" {what}"),
            ratatui::style::Style::new().fg(theme.dim),
        ));
    }
    spans.push(Span::raw(" "));
    spans
}

fn width(pieces: &[Piece]) -> usize {
    pieces
        .iter()
        .map(|(key, what)| key.width() + 1 + what.width())
        .sum::<usize>()
        + SEPARATOR.width() * pieces.len().saturating_sub(1)
        // A space at each end, so the text does not touch the corners.
        + 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn a_legend_that_fits_keeps_every_piece() {
        let theme = Theme::default();
        let drawn = text(&spans(&[("↑↓", "move"), ("Esc", "close")], 40, &theme));
        assert!(drawn.contains("↑↓ move"), "{drawn}");
        assert!(drawn.contains("Esc close"), "{drawn}");
    }

    /// The pieces go from the end, so what survives a narrow pane is the first
    /// and most useful key rather than nothing at all.
    #[test]
    fn a_narrow_pane_keeps_the_first_pieces() {
        let theme = Theme::default();
        let pieces = [("↑↓", "move"), ("Esc", "close"), ("F5", "refresh")];
        let drawn = text(&spans(&pieces, 20, &theme));
        assert!(drawn.contains("↑↓ move"), "{drawn}");
        assert!(!drawn.contains("refresh"), "{drawn}");
        assert!(drawn.width() <= 20, "{drawn:?}");
    }

    /// A key nothing is bound to is not advertised, and takes no cells.
    #[test]
    fn an_unbound_key_is_left_out() {
        let theme = Theme::default();
        let drawn = text(&spans(&[("", "nothing"), ("q", "quit")], 40, &theme));
        assert!(!drawn.contains("nothing"), "{drawn}");
        assert!(drawn.contains("q quit"), "{drawn}");
    }

    #[test]
    fn a_pane_with_no_room_draws_no_legend() {
        let theme = Theme::default();
        assert!(spans(&[("↑↓", "move")], 3, &theme).is_empty());
    }
}
