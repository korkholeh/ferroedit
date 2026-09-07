//! The find/replace bar: its two fields, its options, and the hits it found.
//!
//! State only — the searching itself is `editor/search.rs`, and the commands
//! that drive it are in `commands/execute.rs`. What lives here is the answer to
//! "what is the bar showing right now", which is what both the renderer and the
//! next/previous commands need.
//!
//! The matches are a *cache*: recomputed once a frame when the query, the
//! options, the tab or the document have changed, and left alone otherwise
//! (`App::sync_search`). Recomputing eagerly at every mutation point would mean
//! remembering to do it in a dozen commands, and forgetting once.

use crate::app::input_field::InputField;
use crate::editor::coords::CharIdx;
use crate::editor::search::Match;

/// Hits past this are not collected.
///
/// `e` in a five-megabyte file is half a million of them: a vector nobody
/// reads, and a memory spike on a keystroke. Search still works — the bar says
/// `10000+` and the matches past the limit are simply not offered as
/// destinations.
pub const MAX_MATCHES: usize = 10_000;

/// Which of the bar's two fields the caret is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchField {
    #[default]
    Query,
    Replacement,
}

/// What the matches were computed from. Anything here changing means they have
/// to be found again.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Inputs {
    tab: usize,
    revision: u64,
    query: String,
    case_sensitive: bool,
}

#[derive(Debug, Default)]
pub struct SearchState {
    pub open: bool,
    /// Whether the replacement row is shown — `Ctrl+H` rather than `Ctrl+F`.
    pub replacing: bool,
    pub query: InputField,
    pub replacement: InputField,
    pub field: SearchField,
    pub case_sensitive: bool,
    pub matches: Vec<Match>,
    /// Whether `MAX_MATCHES` cut the list short.
    pub truncated: bool,
    /// Index into `matches` of the hit the caret is on.
    pub current: Option<usize>,
    /// Whether `current` was placed by the caret rather than stepped to.
    ///
    /// The difference is what the first `Enter` does: the bar has just pointed
    /// at a hit, so going *to* it is right and stepping past it would look like
    /// the search skipped one.
    anchored: bool,
    computed: Option<Inputs>,
}

impl SearchState {
    /// Opens the bar, optionally seeding the query with the editor's selection.
    ///
    /// Seeding is what makes "select a word, press `Ctrl+F`" do the obvious
    /// thing. An empty or multi-line selection seeds nothing: a query field
    /// holds one line, and clearing a query the user typed a moment ago would
    /// be worse than ignoring a selection they did not mean as one.
    pub fn open(&mut self, replacing: bool, seed: Option<String>) {
        self.open = true;
        self.replacing = replacing;
        self.field = SearchField::Query;
        if let Some(seed) = seed.filter(|s| !s.is_empty() && !s.contains('\n')) {
            self.query = InputField::new(&seed);
            self.invalidate();
        }
        self.query.end();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.matches.clear();
        self.current = None;
        self.truncated = false;
        self.computed = None;
    }

    /// Rows the bar occupies: none, the find row, or find and replace.
    pub fn height(&self) -> u16 {
        match (self.open, self.replacing) {
            (false, _) => 0,
            (true, false) => 1,
            (true, true) => 2,
        }
    }

    pub fn active_field_mut(&mut self) -> &mut InputField {
        match self.field {
            SearchField::Query => &mut self.query,
            SearchField::Replacement => &mut self.replacement,
        }
    }

    /// Moves the caret to the other field, if there is one.
    pub fn focus_field(&mut self, field: SearchField) {
        if field == SearchField::Replacement && !self.replacing {
            return;
        }
        self.field = field;
    }

    pub fn toggle_field(&mut self) {
        self.field = match (self.field, self.replacing) {
            (SearchField::Query, true) => SearchField::Replacement,
            _ => SearchField::Query,
        };
    }

    /// Forces the matches to be found again on the next sync — what an edit to
    /// the query or a flipped option does.
    pub fn invalidate(&mut self) {
        self.computed = None;
    }

    /// Whether the hits on hand are still the answer for this tab and buffer.
    pub fn is_current(&self, tab: usize, revision: u64) -> bool {
        self.computed.as_ref().is_some_and(|inputs| {
            inputs.tab == tab
                && inputs.revision == revision
                && inputs.query == self.query.value
                && inputs.case_sensitive == self.case_sensitive
        })
    }

    /// Takes a fresh set of hits and puts the current one on the caret.
    pub fn set_matches(
        &mut self,
        tab: usize,
        revision: u64,
        matches: Vec<Match>,
        truncated: bool,
        caret: (usize, CharIdx),
    ) {
        self.matches = matches;
        self.truncated = truncated;
        self.computed = Some(Inputs {
            tab,
            revision,
            query: self.query.value.clone(),
            case_sensitive: self.case_sensitive,
        });
        self.current = self.first_at_or_after(caret);
        self.anchored = self.current.is_some();
    }

    /// The first hit at or after a caret position, wrapping to the top.
    ///
    /// This is what "current" means after the list is rebuilt: pressing Enter
    /// then goes forward from where the user is, not from wherever the previous
    /// search happened to leave off.
    fn first_at_or_after(&self, (line, column): (usize, CharIdx)) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        Some(
            self.matches
                .iter()
                .position(|hit| hit.starts_at_or_after(line, column))
                .unwrap_or(0),
        )
    }

    /// Steps the current hit by one, wrapping at both ends. `None` when there
    /// is nothing to step to.
    pub fn step(&mut self, delta: isize) -> Option<Match> {
        let len = self.matches.len();
        if len == 0 {
            return None;
        }
        let next = match (self.current, self.anchored) {
            (None, _) => 0,
            // Going forward from a hit the caret was put on lands on that hit;
            // going back from it lands on the one before, since the anchored
            // hit is the first at or *after* the caret either way.
            (Some(current), true) if delta > 0 => current,
            (Some(current), _) if delta > 0 => (current + 1) % len,
            (Some(current), _) => (current + len - 1) % len,
        };
        self.current = Some(next);
        self.anchored = false;
        self.matches.get(next).copied()
    }

    /// The hit the caret is on, if any.
    pub fn current_match(&self) -> Option<Match> {
        self.current.and_then(|i| self.matches.get(i)).copied()
    }

    /// `3/17` for the bar's right-hand side, or the empty string when there is
    /// no query to count.
    pub fn count_label(&self) -> String {
        if self.query.value.is_empty() {
            return String::new();
        }
        let total = self.matches.len();
        let more = if self.truncated { "+" } else { "" };
        match self.current {
            Some(index) => format!("{}/{total}{more}", index + 1),
            None => format!("0/{total}{more}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(spans: &[(usize, usize, usize)]) -> Vec<Match> {
        spans
            .iter()
            .map(|(line, start, end)| Match {
                line: *line,
                start: CharIdx(*start),
                end: CharIdx(*end),
            })
            .collect()
    }

    fn state(spans: &[(usize, usize, usize)], caret: (usize, usize)) -> SearchState {
        let mut search = SearchState::default();
        search.open(false, Some("x".into()));
        search.set_matches(0, 1, hits(spans), false, (caret.0, CharIdx(caret.1)));
        search
    }

    #[test]
    fn the_bar_takes_one_row_to_find_and_two_to_replace() {
        let mut search = SearchState::default();
        assert_eq!(search.height(), 0);
        search.open(false, None);
        assert_eq!(search.height(), 1);
        search.open(true, None);
        assert_eq!(search.height(), 2);
        search.close();
        assert_eq!(search.height(), 0);
    }

    #[test]
    fn opening_seeds_the_query_from_a_one_line_selection() {
        let mut search = SearchState::default();
        search.open(false, Some("needle".into()));
        assert_eq!(search.query.value, "needle");
        assert_eq!(search.query.cursor, CharIdx(6), "caret after the seed");

        // A multi-line selection is not a query, and must not clear one.
        search.open(false, Some("two\nlines".into()));
        assert_eq!(search.query.value, "needle");
        search.open(false, None);
        assert_eq!(search.query.value, "needle");
    }

    #[test]
    fn the_current_hit_starts_at_the_caret() {
        let search = state(&[(0, 0, 1), (5, 2, 3), (9, 0, 1)], (5, 0));
        assert_eq!(search.current, Some(1), "the hit on line 6");
        assert_eq!(search.count_label(), "2/3");
    }

    #[test]
    fn a_caret_past_the_last_hit_wraps_to_the_first() {
        let search = state(&[(0, 0, 1), (2, 0, 1)], (40, 0));
        assert_eq!(search.current, Some(0));
    }

    #[test]
    fn the_first_step_forward_lands_on_the_hit_the_caret_is_at() {
        // Otherwise typing a query and pressing Enter skips the match the bar
        // has just pointed at, which reads as a missed hit.
        let mut search = state(&[(3, 0, 1), (7, 0, 1)], (3, 0));
        assert_eq!(search.step(1).map(|m| m.line), Some(3));
        assert_eq!(search.step(1).map(|m| m.line), Some(7));
    }

    #[test]
    fn the_first_step_backward_lands_on_the_hit_before_the_caret() {
        let mut search = state(&[(3, 0, 1), (7, 0, 1)], (3, 0));
        assert_eq!(search.step(-1).map(|m| m.line), Some(7), "wrapping upwards");
    }

    #[test]
    fn next_and_previous_wrap_in_both_directions() {
        let mut search = state(&[(0, 0, 1), (2, 0, 1), (4, 0, 1)], (0, 0));
        assert_eq!(search.current, Some(0));
        assert_eq!(search.step(1).map(|m| m.line), Some(0), "the anchored hit");
        assert_eq!(search.step(1).map(|m| m.line), Some(2));
        assert_eq!(search.step(1).map(|m| m.line), Some(4));
        assert_eq!(search.step(1).map(|m| m.line), Some(0), "wraps to the top");
        assert_eq!(
            search.step(-1).map(|m| m.line),
            Some(4),
            "and to the bottom"
        );
    }

    #[test]
    fn stepping_with_no_hits_does_nothing() {
        let mut search = state(&[], (0, 0));
        assert_eq!(search.step(1), None);
        assert_eq!(search.current, None);
        assert_eq!(search.count_label(), "0/0");
    }

    #[test]
    fn the_count_says_when_the_list_was_cut_short() {
        let mut search = SearchState::default();
        search.open(false, Some("e".into()));
        search.set_matches(0, 1, hits(&[(0, 0, 1)]), true, (0, CharIdx(0)));
        assert_eq!(search.count_label(), "1/1+");
    }

    #[test]
    fn an_empty_query_counts_nothing_at_all() {
        let search = SearchState::default();
        assert_eq!(search.count_label(), "");
    }

    #[test]
    fn the_cache_notices_every_input_that_could_change_the_answer() {
        let mut search = state(&[(0, 0, 1)], (0, 0));
        assert!(search.is_current(0, 1));
        assert!(!search.is_current(1, 1), "another tab");
        assert!(!search.is_current(0, 2), "an edit");

        search.case_sensitive = true;
        assert!(!search.is_current(0, 1));
        search.case_sensitive = false;
        search.query.insert('y');
        assert!(!search.is_current(0, 1));
    }

    #[test]
    fn the_replacement_field_is_only_reachable_while_replacing() {
        let mut search = SearchState::default();
        search.open(false, None);
        search.toggle_field();
        assert_eq!(search.field, SearchField::Query);
        search.focus_field(SearchField::Replacement);
        assert_eq!(search.field, SearchField::Query);

        search.open(true, None);
        search.toggle_field();
        assert_eq!(search.field, SearchField::Replacement);
        search.toggle_field();
        assert_eq!(search.field, SearchField::Query);
    }
}
