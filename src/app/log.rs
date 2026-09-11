//! The log viewer's state: which history, the commits in it, and the search
//! over them.
//!
//! Read-only, like the diff viewer beside it, and a tab for the same reason
//! (ADR-068): a history is something a user opens, keeps, switches away from
//! and closes. What it adds is a search field, because a list of two thousand
//! commits that could only be scrolled would not be a history anybody reads.
//!
//! Nothing here runs git. The commits arrive already read, so this module is
//! testable against hand-built lists.

use crate::app::input_field::InputField;
use crate::git::log::MAX_COMMITS;
use crate::git::{Commit, LogScope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogState {
    pub scope: LogScope,
    /// Everything the last read produced, newest first.
    commits: Vec<Commit>,
    /// Indices into `commits`, in display order: what the field's text leaves.
    visible: Vec<usize>,
    /// The search field. It holds its text whether or not it has the caret, so
    /// closing it and reopening it does not lose what was typed.
    pub field: InputField,
    /// Whether the field has the caret. The pane's keys are a pager's while it
    /// is closed and a field's while it is open, which is the same arrangement
    /// the find bar has (SPEC §22).
    pub searching: bool,
    /// The pattern git itself was asked for, when this list came from a search
    /// rather than from the whole history.
    pub query: Option<String>,
    /// Whether `MAX_COMMITS` is what ended the list.
    pub truncated: bool,
    /// Index into `visible`.
    selected: usize,
    pub scroll: usize,
    /// Whether the three columns in front of the subject are drawn (ADR-080).
    ///
    /// On by default — a history without its dates is a list of sentences —
    /// and off is what a reader turns to when the pane is narrow enough that
    /// the columns are costing them the end of every subject.
    pub show_columns: bool,
}

impl LogState {
    pub fn new(scope: LogScope, commits: Vec<Commit>, query: Option<String>) -> Self {
        let truncated = commits.len() >= MAX_COMMITS;
        let visible = (0..commits.len()).collect();
        Self {
            scope,
            commits,
            visible,
            field: InputField::default(),
            searching: false,
            query,
            truncated,
            selected: 0,
            scroll: 0,
            show_columns: true,
        }
    }

    /// Puts a freshly read list in the same viewer — a refresh, or the answer
    /// to a search.
    ///
    /// The selection goes back to the top rather than trying to follow the
    /// commit it was on: a new list is a different question's answer, and a
    /// viewer that silently scrolled to the middle of it would be showing the
    /// user somewhere they did not ask to be.
    pub fn replace(&mut self, commits: Vec<Commit>, query: Option<String>) {
        self.truncated = commits.len() >= MAX_COMMITS;
        self.commits = commits;
        self.query = query;
        self.selected = 0;
        self.scroll = 0;
        self.rebuild_visible();
    }

    /// The commits to draw, in display order.
    pub fn rows(&self) -> impl Iterator<Item = &Commit> {
        self.visible.iter().filter_map(|i| self.commits.get(*i))
    }

    /// How many rows are shown — what the selection is clamped against.
    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    /// Whether the read itself produced nothing — not the same question as a
    /// field that currently matches nothing.
    pub fn has_no_commits(&self) -> bool {
        self.commits.is_empty()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        self.visible
            .get(self.selected)
            .and_then(|i| self.commits.get(*i))
    }

    /// Opens the search field, putting the caret at the end of whatever it
    /// already holds.
    pub fn open_search(&mut self) {
        self.field.end();
        self.searching = true;
    }

    /// Closes the field and drops what it was narrowing by.
    ///
    /// `Esc` leaves the search rather than only leaving the field: a list still
    /// filtered by text the user can no longer see is a viewer that appears to
    /// have lost half its commits.
    pub fn close_search(&mut self, height: usize) {
        self.searching = false;
        self.field = InputField::default();
        self.refilter(height);
    }

    /// Re-applies the field's text to the loaded commits.
    ///
    /// Called after every keystroke in the field. It narrows what was *read*
    /// and does not ask git anything, which is what makes it instant; `Enter`
    /// is what hands the text to git, and reaches the whole message and the
    /// commits past the cap (ADR-068).
    pub fn refilter(&mut self, height: usize) {
        let previous = self.selected_commit().map(|commit| commit.oid.clone());
        self.rebuild_visible();
        self.selected = previous
            .and_then(|oid| {
                self.visible
                    .iter()
                    .position(|i| self.commits[*i].oid == oid)
            })
            .unwrap_or(0);
        self.follow_selection(height);
    }

    fn rebuild_visible(&mut self) {
        let needle = self.field.value.trim().to_lowercase();
        self.visible = self
            .commits
            .iter()
            .enumerate()
            .filter(|(_, commit)| commit.matches(&needle))
            .map(|(index, _)| index)
            .collect();
    }

    /// Moves the selection, clamped at both ends.
    pub fn step(&mut self, delta: isize, height: usize) {
        if self.visible.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        let last = self.visible.len() - 1;
        let target = self.selected as isize + delta;
        self.selected = target.clamp(0, last as isize) as usize;
        self.follow_selection(height);
    }

    /// Selects the row at `row` cells down the pane — a click.
    pub fn select_row(&mut self, row: usize, height: usize) {
        if self.visible.is_empty() {
            return;
        }
        self.selected = (self.scroll + row).min(self.visible.len() - 1);
        self.follow_selection(height);
    }

    pub fn home(&mut self, height: usize) {
        self.selected = 0;
        self.follow_selection(height);
    }

    pub fn end(&mut self, height: usize) {
        self.selected = self.visible.len().saturating_sub(1);
        self.follow_selection(height);
    }

    /// Keeps the selection on a row and the row on screen — the rule the
    /// explorer and the git panel both follow.
    pub fn follow_selection(&mut self, height: usize) {
        if self.visible.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(self.visible.len() - 1);
        if height == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        self.scroll = self.scroll.min(self.visible.len().saturating_sub(height));
    }

    /// What the tab strip calls it.
    pub fn name(&self) -> String {
        self.scope.name()
    }

    /// ` Log — src/main.rs — 42 commits, matching "fix" `.
    ///
    /// The count is of what is *shown*, because that is the list being
    /// scrolled; `(cut)` says the limit ended it, so a search that seems to
    /// have missed something has a visible reason (ADR-068).
    pub fn title(&self) -> String {
        let cut = if self.truncated { " (cut)" } else { "" };
        let matching = match &self.query {
            Some(query) => format!(", matching \"{query}\""),
            None => String::new(),
        };
        let filtered = if self.field.value.trim().is_empty() {
            String::new()
        } else {
            format!(" of {}", self.commits.len())
        };
        let commits = match self.visible.len() {
            1 => "1 commit".to_string(),
            count => format!("{count} commits"),
        };
        format!(
            " Log — {} — {commits}{filtered}{matching}{cut} ",
            self.scope.label()
        )
    }

    /// `12/340`, the bottom-right readout: which row the selection is on, out
    /// of how many are shown.
    pub fn position(&self) -> String {
        let at = if self.visible.is_empty() {
            0
        } else {
            self.selected + 1
        };
        format!(" {at}/{} ", self.visible.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn commit(oid: &str, subject: &str) -> Commit {
        Commit {
            oid: format!("{oid:0<40}"),
            short: oid.to_string(),
            author: "Ada".into(),
            date: "2026-09-10".into(),
            subject: subject.into(),
            body: String::new(),
        }
    }

    fn viewer() -> LogState {
        LogState::new(
            LogScope::Repository,
            vec![
                commit("aaaaaaa", "feat: the newest thing"),
                commit("bbbbbbb", "fix: a bug"),
                commit("ccccccc", "docs: a note"),
            ],
            None,
        )
    }

    #[test]
    fn the_field_narrows_what_was_read_without_asking_git() {
        let mut log = viewer();
        assert_eq!(log.len(), 3);
        log.open_search();
        log.field.insert_str("fix");
        log.refilter(10);
        assert_eq!(log.len(), 1);
        assert_eq!(log.selected_commit().unwrap().short, "bbbbbbb");
        assert!(
            log.title().contains("1 commit of 3"),
            "the title says what was hidden: {}",
            log.title()
        );
    }

    /// Leaving the field puts the whole list back: a viewer still filtered by
    /// text nobody can see has apparently lost half its commits.
    #[test]
    fn closing_the_field_drops_the_filter_with_it() {
        let mut log = viewer();
        log.open_search();
        log.field.insert_str("fix");
        log.refilter(10);
        log.close_search(10);
        assert_eq!(log.len(), 3);
        assert!(!log.searching);
        assert!(log.field.value.is_empty());
    }

    #[test]
    fn a_filter_keeps_the_selection_on_the_commit_it_was_on() {
        let mut log = viewer();
        log.step(1, 10);
        assert_eq!(log.selected_commit().unwrap().short, "bbbbbbb");
        log.field.insert_str("a bug");
        log.refilter(10);
        assert_eq!(
            log.selected_commit().unwrap().short,
            "bbbbbbb",
            "the row it was on is still shown"
        );
    }

    #[test]
    fn a_filter_that_hides_the_selected_row_lands_on_the_first_match() {
        let mut log = viewer();
        log.end(10);
        assert_eq!(log.selected_commit().unwrap().short, "ccccccc");
        log.field.insert_str("f");
        log.refilter(10);
        assert_eq!(log.selected_commit().unwrap().short, "aaaaaaa");
    }

    #[test]
    fn the_selection_stops_at_both_ends() {
        let mut log = viewer();
        log.step(-5, 10);
        assert_eq!(log.selected(), 0);
        log.step(100, 10);
        assert_eq!(log.selected(), 2);
        assert_eq!(log.position(), " 3/3 ");
    }

    #[test]
    fn the_view_follows_the_selection_down_a_list_taller_than_the_pane() {
        let commits = (0..20)
            .map(|i| commit(&format!("{i:07}"), "x"))
            .collect::<Vec<_>>();
        let mut log = LogState::new(LogScope::Repository, commits, None);
        log.step(15, 5);
        assert_eq!(log.scroll, 11, "the selected row is the last one shown");
        log.home(5);
        assert_eq!((log.selected(), log.scroll), (0, 0));
    }

    #[test]
    fn an_empty_history_has_nothing_selected_and_does_not_panic() {
        let mut log = LogState::new(LogScope::Repository, Vec::new(), None);
        assert!(log.has_no_commits());
        log.step(3, 5);
        log.end(5);
        assert_eq!(log.position(), " 0/0 ");
        assert!(log.selected_commit().is_none());
    }

    #[test]
    fn a_search_says_so_in_the_title_and_a_new_list_starts_at_the_top() {
        let mut log = viewer();
        log.end(10);
        log.replace(vec![commit("ddddddd", "fix: another")], Some("fix".into()));
        assert_eq!(log.selected(), 0);
        assert_eq!(log.scroll, 0);
        assert!(log.title().contains("matching \"fix\""), "{}", log.title());
    }

    #[test]
    fn a_scoped_viewer_names_its_file_in_the_title_and_in_the_strip() {
        let log = LogState::new(
            LogScope::Lines {
                path: PathBuf::from("src/app/log.rs"),
                first: 4,
                last: 9,
            },
            vec![commit("aaaaaaa", "x")],
            None,
        );
        assert_eq!(log.name(), "Log: log.rs:4-9");
        assert!(
            log.title()
                .starts_with(" Log — src/app/log.rs:4-9 — 1 commit"),
            "{}",
            log.title()
        );
    }
}
