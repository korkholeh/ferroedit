//! Search and replace over the rope.
//!
//! Plain substring search, not regex (SPEC §22). Everything here works on one
//! line of `&str` at a time, for two reasons: a query typed into a one-line
//! field can never contain a newline, so no match can straddle a line break;
//! and a match is reported in the same (line, column) coordinates the renderer
//! and the cursor already speak, so nothing has to convert.

use crate::editor::coords::CharIdx;

/// One hit: a half-open char range inside one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: CharIdx,
    pub end: CharIdx,
}

impl Match {
    /// Reading order, for finding the hit at or after the cursor.
    pub fn starts_at_or_after(&self, line: usize, column: CharIdx) -> bool {
        (self.line, self.start) >= (line, column)
    }
}

/// Every non-overlapping occurrence of `query` in `line`, left to right.
///
/// Non-overlapping matters for replace-all: `aa` in `aaaa` is two hits, not
/// three, and replacing them must not leave a fragment behind.
pub fn find_in_line(line: &str, query: &str, case_sensitive: bool) -> Vec<(CharIdx, CharIdx)> {
    let mut hits = Vec::new();
    if query.is_empty() || line.is_empty() {
        return hits;
    }
    if case_sensitive {
        exact(line, query, &mut hits);
    } else {
        folded(line, query, &mut hits);
    }
    hits
}

/// Case-sensitive search, over `str::find`.
///
/// Worth the separate path: `find` is a two-way search that skips ahead,
/// against a comparison at every character. It is the case a large file is
/// most likely to be searched in, and the one the folding path cannot use.
fn exact(line: &str, query: &str, hits: &mut Vec<(CharIdx, CharIdx)>) {
    let width = query.chars().count();
    let mut byte = 0;
    let mut column = 0;
    while let Some(offset) = line[byte..].find(query) {
        column += line[byte..byte + offset].chars().count();
        hits.push((CharIdx(column), CharIdx(column + width)));
        byte += offset + query.len();
        column += width;
    }
}

/// Case-insensitive search, one character at a time.
fn folded(line: &str, query: &str, hits: &mut Vec<(CharIdx, CharIdx)>) {
    let mut byte = 0;
    let mut column = 0;
    while byte < line.len() {
        let rest = &line[byte..];
        match match_len(rest, query) {
            Some(chars) => {
                hits.push((CharIdx(column), CharIdx(column + chars)));
                byte += byte_len(rest, chars);
                column += chars;
            }
            None => {
                // Advance by a whole character: a byte step could land inside
                // one and slice a multi-byte character in half.
                let ch = rest.chars().next().expect("rest is non-empty");
                byte += ch.len_utf8();
                column += 1;
            }
        }
    }
}

/// How many characters of `hay` `query` matches at its start, ignoring case.
///
/// The two are compared character by character through `char::to_lowercase`,
/// which is what keeps `Привіт` and `привіт` the same word without lowercasing
/// the line into a second buffer whose indices no longer line up with the
/// first.
fn match_len(hay: &str, query: &str) -> Option<usize> {
    let mut wanted = query.chars().flat_map(char::to_lowercase);
    let mut consumed = 0;
    let mut next = wanted.next();
    for ch in hay.chars() {
        if next.is_none() {
            break;
        }
        for lowered in ch.to_lowercase() {
            match next {
                // The query ran out half way through this character's own
                // expansion: `İ` lowercases to two characters, and matching
                // only the first of them is not a match anyone asked for.
                None => return None,
                Some(expected) if expected != lowered => return None,
                Some(_) => next = wanted.next(),
            }
        }
        consumed += 1;
    }
    next.is_none().then_some(consumed)
}

/// Byte length of the first `chars` characters of `text`.
fn byte_len(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(line: &str, query: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
        find_in_line(line, query, case_sensitive)
            .into_iter()
            .map(|(start, end)| (start.0, end.0))
            .collect()
    }

    #[test]
    fn a_query_is_found_everywhere_it_occurs() {
        assert_eq!(hits("foo bar foo", "foo", true), vec![(0, 3), (8, 11)]);
    }

    #[test]
    fn overlapping_occurrences_are_counted_once() {
        // `aa` in `aaaa` is two hits, not three: replace-all has to be able to
        // rewrite them without leaving a fragment.
        assert_eq!(hits("aaaa", "aa", true), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn case_sensitivity_is_the_toggle_it_looks_like() {
        assert_eq!(hits("Foo foo FOO", "foo", true), vec![(4, 7)]);
        assert_eq!(
            hits("Foo foo FOO", "foo", false),
            vec![(0, 3), (4, 7), (8, 11)]
        );
    }

    #[test]
    fn columns_are_counted_in_characters_and_not_in_bytes() {
        // Each Cyrillic character is two bytes; a byte index here would put the
        // match three cells to the left of where it is.
        assert_eq!(hits("Привіт світ", "світ", true), vec![(7, 11)]);
    }

    #[test]
    fn case_folding_works_beyond_ascii() {
        assert_eq!(hits("ПРИВІТ", "привіт", false), vec![(0, 6)]);
        assert_eq!(hits("Straße", "STRASSE", false), vec![]);
        assert_eq!(hits("ÉCOLE", "école", false), vec![(0, 5)]);
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        assert_eq!(hits("anything", "", true), Vec::<(usize, usize)>::new());
        assert_eq!(hits("", "x", true), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn a_query_longer_than_the_line_does_not_match() {
        assert_eq!(hits("ab", "abc", true), Vec::<(usize, usize)>::new());
        assert_eq!(hits("ab", "abc", false), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn a_match_knows_whether_it_is_ahead_of_the_caret() {
        let hit = Match {
            line: 3,
            start: CharIdx(2),
            end: CharIdx(5),
        };
        assert!(hit.starts_at_or_after(3, CharIdx(2)));
        assert!(!hit.starts_at_or_after(3, CharIdx(3)));
        assert!(hit.starts_at_or_after(2, CharIdx(99)));
    }
}
