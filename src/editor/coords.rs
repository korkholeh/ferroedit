//! Byte / char / grapheme / visual-column conversions.
//!
//! The four coordinate systems are deliberately distinct types; see
//! `docs/ARCHITECTURE.md` for why cursors are stored in char indices.
//!
//! Every function here takes **one line without its terminator** as `&str`, so
//! the whole module is testable without a rope and without a terminal. The
//! document layer is what knows about lines; this layer only knows about text.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Cells a tab advances to. Configurable in Phase 14; one constant until then.
pub const DEFAULT_TAB_WIDTH: usize = 4;

/// Defines one index newtype.
///
/// Only trait impls are generated, never inherent methods: the wrapped value is
/// read as `.0`, so there is no accessor to go unused as the crate grows.
macro_rules! index_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub usize);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

index_type! {
    /// Offset in bytes. File IO and (from Phase 7) syntect speak this.
    ByteIdx
}
index_type! {
    /// Offset in `char`s — Unicode scalar values. The canonical cursor storage,
    /// because `ropey` is char-indexed and so every rope operation is O(1) here.
    CharIdx
}
index_type! {
    /// Offset in user-perceived characters. `é` as `e` + U+0301 is one of these
    /// and two `CharIdx`. Movement steps and the status bar's column use it.
    GraphemeIdx
}
index_type! {
    /// Terminal display column. Not invertible — many positions share one — so
    /// it is used for rendering, hit-testing and the preferred column only.
    VisualCol
}

/// The grapheme clusters of a line, each with the char index it starts at.
///
/// This is the one place the crate iterates text for editing purposes; both
/// movement and rendering go through it so they cannot disagree about where a
/// character begins.
pub fn clusters(line: &str) -> impl Iterator<Item = (CharIdx, &str)> {
    line.graphemes(true).scan(0usize, |chars, cluster| {
        let at = CharIdx(*chars);
        *chars += cluster.chars().count();
        Some((at, cluster))
    })
}

/// Cells one cluster occupies when drawn starting at `col`.
///
/// A tab is elastic — it advances to the next tab stop — which is why the
/// column has to be an argument rather than a property of the cluster.
///
/// Combining marks and zero-width joiners report 0 and so contribute nothing,
/// which is what keeps `é` one cell wide however it is encoded. ZWJ emoji
/// sequences are the known imperfection recorded in ADR-004: we report what
/// `unicode-width` reports, and terminals disagree with each other anyway.
pub fn cluster_width(cluster: &str, col: VisualCol, tab_width: usize) -> usize {
    if cluster == "\t" {
        tab_width - (col.0 % tab_width)
    } else {
        cluster.width()
    }
}

/// Number of `char`s in the line — i.e. the char index one past its end.
pub fn char_len(line: &str) -> CharIdx {
    CharIdx(line.chars().count())
}

/// The display column a char index sits at.
pub fn visual_col(line: &str, char_idx: CharIdx, tab_width: usize) -> VisualCol {
    let mut col = 0;
    for (at, cluster) in clusters(line) {
        if at.0 >= char_idx.0 {
            break;
        }
        col += cluster_width(cluster, VisualCol(col), tab_width);
    }
    VisualCol(col)
}

/// The char index a display column points at: the start of the cluster whose
/// cell range contains `target`, or the end of the line when it is past it.
///
/// Clicking anywhere inside a wide character or a tab therefore lands *before*
/// it, never in the middle of it.
pub fn char_at_visual_col(line: &str, target: VisualCol, tab_width: usize) -> CharIdx {
    let mut col = 0;
    for (at, cluster) in clusters(line) {
        let width = cluster_width(cluster, VisualCol(col), tab_width);
        if target.0 < col + width {
            return at;
        }
        col += width;
    }
    char_len(line)
}

/// How many user-perceived characters precede a char index. The status bar's
/// `Col` is this plus one, which is the count a human would arrive at.
pub fn grapheme_index(line: &str, char_idx: CharIdx) -> GraphemeIdx {
    GraphemeIdx(
        clusters(line)
            .take_while(|(at, _)| at.0 < char_idx.0)
            .count(),
    )
}

/// The start of the next cluster, or the end of the line.
pub fn next_grapheme(line: &str, char_idx: CharIdx) -> CharIdx {
    clusters(line)
        .map(|(at, _)| at)
        .find(|at| at.0 > char_idx.0)
        .unwrap_or_else(|| char_len(line))
}

/// The start of the previous cluster, or the start of the line.
pub fn prev_grapheme(line: &str, char_idx: CharIdx) -> CharIdx {
    clusters(line)
        .map(|(at, _)| at)
        .take_while(|at| at.0 < char_idx.0)
        .last()
        .unwrap_or_default()
}

/// Moves a char index onto the nearest cluster boundary at or before it, and
/// never past the end of the line.
///
/// Vertical movement lands on an arbitrary column of another line, so every
/// such landing is snapped: a cursor must never sit between `e` and its accent.
pub fn snap(line: &str, char_idx: CharIdx) -> CharIdx {
    let len = char_len(line);
    if char_idx.0 >= len.0 {
        return len;
    }
    clusters(line)
        .map(|(at, _)| at)
        .take_while(|at| at.0 <= char_idx.0)
        .last()
        .unwrap_or_default()
}

/// Start of the next word, or the end of the line.
pub fn next_word(line: &str, char_idx: CharIdx) -> CharIdx {
    word_starts(line)
        .find(|at| at.0 > char_idx.0)
        .unwrap_or_else(|| char_len(line))
}

/// Start of the previous word, or the start of the line.
pub fn prev_word(line: &str, char_idx: CharIdx) -> CharIdx {
    word_starts(line)
        .take_while(|at| at.0 < char_idx.0)
        .last()
        .unwrap_or_default()
}

/// Display width of a whole line — the column one past its last cell.
///
/// Selection rendering needs it to know how far a highlighted line reaches,
/// and it is the same walk as `visual_col` to the end of the line.
pub fn line_width(line: &str, tab_width: usize) -> VisualCol {
    visual_col(line, char_len(line), tab_width)
}

/// The word surrounding a char index, as a half-open char range.
///
/// This is what a double-click selects. The segment under the caret is
/// returned whatever it is — a run of spaces double-clicks as that run rather
/// than as nothing — because a selection that silently comes back empty reads
/// as a broken click.
pub fn word_bounds(line: &str, char_idx: CharIdx) -> (CharIdx, CharIdx) {
    let len = char_len(line);
    let mut last = (len, len);
    for (at, segment) in word_segments(line) {
        let end = CharIdx(at.0 + segment.chars().count());
        if char_idx.0 < end.0 {
            return (at, end);
        }
        last = (at, end);
    }
    // Past the last segment: the caret sits at the end of the line, where the
    // word behind it is the one the user pointed at.
    last
}

/// Char indices at which a word begins, using UAX #29 word boundaries so that
/// `Привіт` and `日本語` break where a reader expects rather than at ASCII.
fn word_starts(line: &str) -> impl Iterator<Item = CharIdx> + '_ {
    word_segments(line)
        .filter(|(_, segment)| !segment.chars().all(char::is_whitespace))
        .map(|(at, _)| at)
}

/// UAX #29 word segments of a line, each with the char index it starts at.
fn word_segments(line: &str) -> impl Iterator<Item = (CharIdx, &str)> {
    line.split_word_bounds().scan(0usize, |chars, segment| {
        let at = CharIdx(*chars);
        *chars += segment.chars().count();
        Some((at, segment))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC §48 fixture, compiled in so the tests need no filesystem.
    const FIXTURE: &str = include_str!("../../tests/fixtures/unicode.txt");

    const TAB: usize = DEFAULT_TAB_WIDTH;

    fn fixture(index: usize) -> &'static str {
        FIXTURE.lines().nth(index).expect("fixture line")
    }

    // The fixture in order, so a test reads as prose rather than as indices.
    const HELLO: usize = 0;
    const CYRILLIC: usize = 1;
    const CJK: usize = 3;
    const EMOJI: usize = 4;
    const FAMILY: usize = 5;
    const PRECOMPOSED: usize = 6;
    const DECOMPOSED: usize = 7;

    #[test]
    fn the_fixture_is_the_one_spec_48_asks_for() {
        assert_eq!(fixture(HELLO), "Hello");
        assert_eq!(fixture(CYRILLIC), "Привіт");
        assert_eq!(fixture(2), "Україна");
        assert_eq!(fixture(CJK), "日本語");
        assert_eq!(fixture(EMOJI), "🙂");
        assert_eq!(fixture(FAMILY), "👨‍👩‍👧");
        // The whole point of the last two: same glyph, different encodings.
        assert_eq!(fixture(PRECOMPOSED), "é");
        assert_eq!(fixture(DECOMPOSED), "e\u{301}");
        assert_ne!(fixture(PRECOMPOSED), fixture(DECOMPOSED));
    }

    #[test]
    fn a_multi_byte_scalar_is_one_char_and_one_cell() {
        let line = fixture(CYRILLIC);
        assert_eq!(line.len(), 12, "six two-byte scalars");
        assert_eq!(char_len(line), CharIdx(6));
        assert_eq!(line_width(line, TAB), VisualCol(6));
    }

    #[test]
    fn cjk_characters_are_two_cells_each() {
        let line = fixture(CJK);
        assert_eq!(char_len(line), CharIdx(3));
        assert_eq!(line_width(line, TAB), VisualCol(6));
        assert_eq!(visual_col(line, CharIdx(1), TAB), VisualCol(2));
        assert_eq!(visual_col(line, CharIdx(2), TAB), VisualCol(4));
    }

    #[test]
    fn a_decomposed_accent_is_one_step_and_one_cell() {
        let line = fixture(DECOMPOSED);
        assert_eq!(char_len(line), CharIdx(2), "e plus a combining acute");
        assert_eq!(line_width(line, TAB), VisualCol(1));
        // One Right press must clear the whole cluster, not land on the accent.
        assert_eq!(next_grapheme(line, CharIdx(0)), CharIdx(2));
        assert_eq!(prev_grapheme(line, CharIdx(2)), CharIdx(0));
    }

    #[test]
    fn a_precomposed_accent_behaves_the_same_from_the_outside() {
        let line = fixture(PRECOMPOSED);
        assert_eq!(line_width(line, TAB), line_width(fixture(DECOMPOSED), TAB));
        assert_eq!(grapheme_index(line, char_len(line)), GraphemeIdx(1));
        assert_eq!(
            grapheme_index(fixture(DECOMPOSED), char_len(fixture(DECOMPOSED))),
            GraphemeIdx(1)
        );
    }

    #[test]
    fn a_zwj_sequence_is_a_single_cursor_stop() {
        let line = fixture(FAMILY);
        assert_eq!(char_len(line), CharIdx(5), "three emoji and two joiners");
        assert_eq!(grapheme_index(line, char_len(line)), GraphemeIdx(1));
        assert_eq!(next_grapheme(line, CharIdx(0)), CharIdx(5));
        // The rendered width is whatever unicode-width says; terminals disagree
        // about ZWJ sequences and we follow the crate rather than guess (ADR-004).
        assert_eq!(line_width(line, TAB), VisualCol(line.width()));
    }

    #[test]
    fn a_lone_emoji_is_two_cells_and_one_stop() {
        let line = fixture(EMOJI);
        assert_eq!(line_width(line, TAB), VisualCol(2));
        assert_eq!(next_grapheme(line, CharIdx(0)), char_len(line));
    }

    #[test]
    fn movement_stops_at_the_ends_of_a_line() {
        let line = fixture(HELLO);
        assert_eq!(prev_grapheme(line, CharIdx(0)), CharIdx(0));
        assert_eq!(next_grapheme(line, char_len(line)), char_len(line));
    }

    #[test]
    fn walking_a_line_by_grapheme_visits_every_cluster_once() {
        for index in 0..8 {
            let line = fixture(index);
            let mut at = CharIdx(0);
            let mut steps = 0;
            while at < char_len(line) {
                at = next_grapheme(line, at);
                steps += 1;
                assert!(steps <= 16, "line {index} did not terminate");
            }
            assert_eq!(GraphemeIdx(steps), grapheme_index(line, char_len(line)));
        }
    }

    #[test]
    fn tabs_advance_to_the_next_stop_not_by_a_fixed_width() {
        // "\tab\tc": the first tab fills 0..4, "ab" is 4..6, the second fills 6..8.
        let line = "\tab\tc";
        assert_eq!(visual_col(line, CharIdx(1), TAB), VisualCol(4));
        assert_eq!(visual_col(line, CharIdx(3), TAB), VisualCol(6));
        assert_eq!(visual_col(line, CharIdx(4), TAB), VisualCol(8));
        assert_eq!(line_width(line, TAB), VisualCol(9));
    }

    #[test]
    fn a_click_inside_a_wide_character_lands_before_it() {
        let line = fixture(CJK);
        assert_eq!(char_at_visual_col(line, VisualCol(0), TAB), CharIdx(0));
        assert_eq!(char_at_visual_col(line, VisualCol(1), TAB), CharIdx(0));
        assert_eq!(char_at_visual_col(line, VisualCol(2), TAB), CharIdx(1));
        assert_eq!(char_at_visual_col(line, VisualCol(3), TAB), CharIdx(1));
    }

    #[test]
    fn a_click_inside_a_tab_lands_before_it() {
        let line = "\tx";
        for col in 0..4 {
            assert_eq!(char_at_visual_col(line, VisualCol(col), TAB), CharIdx(0));
        }
        assert_eq!(char_at_visual_col(line, VisualCol(4), TAB), CharIdx(1));
    }

    #[test]
    fn a_click_past_the_end_of_a_line_lands_at_its_end() {
        let line = fixture(HELLO);
        assert_eq!(char_at_visual_col(line, VisualCol(99), TAB), CharIdx(5));
        assert_eq!(char_at_visual_col("", VisualCol(3), TAB), CharIdx(0));
    }

    #[test]
    fn visual_col_and_char_at_visual_col_agree_on_every_boundary() {
        for index in 0..8 {
            let line = fixture(index);
            for (at, _) in clusters(line) {
                let col = visual_col(line, at, TAB);
                assert_eq!(
                    char_at_visual_col(line, col, TAB),
                    at,
                    "line {index} boundary {at}"
                );
            }
        }
    }

    #[test]
    fn snapping_never_leaves_the_cursor_inside_a_cluster() {
        let line = fixture(DECOMPOSED);
        assert_eq!(snap(line, CharIdx(1)), CharIdx(0), "between e and U+0301");
        assert_eq!(snap(line, CharIdx(2)), CharIdx(2));
        assert_eq!(snap(line, CharIdx(99)), char_len(line));

        let family = fixture(FAMILY);
        for inside in 1..5 {
            assert_eq!(snap(family, CharIdx(inside)), CharIdx(0));
        }
    }

    #[test]
    fn word_motion_steps_between_words_and_stops_at_the_ends() {
        let line = "let mut x = 1;";
        assert_eq!(next_word(line, CharIdx(0)), CharIdx(4), "let -> mut");
        assert_eq!(next_word(line, CharIdx(4)), CharIdx(8), "mut -> x");
        assert_eq!(prev_word(line, CharIdx(8)), CharIdx(4));
        assert_eq!(prev_word(line, CharIdx(0)), CharIdx(0));
        assert_eq!(next_word(line, char_len(line)), char_len(line));
    }

    #[test]
    fn word_motion_understands_non_ascii_words() {
        let line = "Привіт Україна";
        assert_eq!(next_word(line, CharIdx(0)), CharIdx(7));
        assert_eq!(prev_word(line, char_len(line)), CharIdx(7));
    }

    #[test]
    fn an_empty_line_has_no_positions_but_the_first() {
        assert_eq!(char_len(""), CharIdx(0));
        assert_eq!(line_width("", TAB), VisualCol(0));
        assert_eq!(next_grapheme("", CharIdx(0)), CharIdx(0));
        assert_eq!(prev_grapheme("", CharIdx(0)), CharIdx(0));
        assert_eq!(snap("", CharIdx(3)), CharIdx(0));
    }
}
