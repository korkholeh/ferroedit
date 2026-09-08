//! The Open dialog's directory browser (SPEC §6).
//!
//! The Open prompt used to be one text field: a path was typed, or it was not
//! opened. That is the right tool for a path already on the clipboard and the
//! wrong one for "somewhere over there" — the user has to know the answer
//! before they can ask the question (ADR-051).
//!
//! This is the listing behind the new body: one directory at a time, read on
//! demand, with a filter field over it. It owns no I/O policy beyond
//! `read_dir` — where the chosen path goes is `commands::execute`'s business.

use std::path::{Path, PathBuf};

use crate::app::input_field::InputField;

/// One row of the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserEntry {
    /// What the row shows: `..`, or the entry's own file name.
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// The `..` row. It is a directory like any other, but it is never hidden
    /// by the filter and never sorted among the names.
    pub parent: bool,
}

/// A directory, what is in it, and where the selection is.
///
/// `entries` is everything `read_dir` returned; `visible` is the indices that
/// survive the filter, in display order. Keeping both means a filter that is
/// cleared costs no syscall, and the selection can be carried across a filter
/// edit by path rather than by index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    dir: PathBuf,
    entries: Vec<BrowserEntry>,
    visible: Vec<usize>,
    /// Narrows the listing, and is also the path field the old dialog was: an
    /// answer that resolves to a real path wins over the selection
    /// (`commands::execute::browser_target`).
    pub filter: InputField,
    /// Index into `visible`.
    selected: usize,
    scroll: usize,
    /// Why the listing is empty, when it is empty because of an error rather
    /// than because the directory is.
    error: Option<String>,
}

impl Browser {
    /// Lists `dir`. A directory that cannot be read is not a failure to open
    /// the dialog: the box appears with the reason in it, and `..` is still
    /// there to get out with.
    pub fn new(dir: &Path) -> Self {
        let mut browser = Self {
            dir: dir.to_path_buf(),
            entries: Vec::new(),
            visible: Vec::new(),
            filter: InputField::default(),
            selected: 0,
            scroll: 0,
            error: None,
        };
        browser.read();
        browser
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The rows to draw, in display order.
    pub fn rows(&self) -> impl Iterator<Item = &BrowserEntry> {
        self.visible.iter().filter_map(|i| self.entries.get(*i))
    }

    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// The row the buttons act on.
    pub fn selected_entry(&self) -> Option<&BrowserEntry> {
        self.visible
            .get(self.selected)
            .and_then(|i| self.entries.get(*i))
    }

    /// The directory `Open Folder` means: the selected one when a directory is
    /// selected, and otherwise the one being listed.
    ///
    /// `..` is not one of those directories. It is the way out of the listing,
    /// not a thing in it — and it is what the selection rests on the moment you
    /// walk into a folder, which is exactly when Open Folder is most likely to
    /// be pressed. Counting it opened the *parent* of the folder that had just
    /// been chosen.
    pub fn folder_target(&self) -> PathBuf {
        match self.selected_entry() {
            Some(entry) if entry.is_dir && !entry.parent => entry.path.clone(),
            _ => self.dir.clone(),
        }
    }

    /// Lists another directory, from the top and with the filter cleared.
    ///
    /// The filter is cleared because it described the listing that is being
    /// left: carrying `car` into a directory with no `Cargo.toml` in it would
    /// show an empty box and no reason for it.
    pub fn open_dir(&mut self, dir: &Path) {
        self.dir = dir.to_path_buf();
        self.filter = InputField::default();
        self.read();
    }

    /// Re-applies the filter after the field has been edited, keeping the
    /// selection on the same entry when that entry is still shown.
    ///
    /// `..` is the exception: it survives every filter, so a selection resting
    /// on it would never move and Enter after typing a name would walk *up* a
    /// directory. Typing is how you aim at something, so the selection lands on
    /// the first row that is not the way out.
    pub fn refilter(&mut self, height: usize) {
        let previous = self
            .selected_entry()
            .filter(|entry| !entry.parent)
            .map(|entry| entry.path.clone());
        self.rebuild_visible();
        self.selected = previous
            .and_then(|path| {
                self.visible
                    .iter()
                    .position(|i| self.entries[*i].path == path)
            })
            .or_else(|| self.visible.iter().position(|i| !self.entries[*i].parent))
            .unwrap_or(0);
        self.follow_selection(height);
    }

    /// Moves the selection, clamped at both ends like the branch picker's.
    pub fn step(&mut self, delta: i16, height: usize) {
        if self.visible.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        let last = self.visible.len() - 1;
        self.selected = if delta < 0 {
            self.selected.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (self.selected + delta as usize).min(last)
        };
        self.follow_selection(height);
    }

    /// Selects a row by its place in the drawn window; the scroll is added
    /// here for the same reason the branch picker adds it here (ADR-035).
    pub fn select_visible_row(&mut self, row: usize, height: usize) {
        if self.visible.is_empty() {
            return;
        }
        self.selected = (self.scroll + row).min(self.visible.len() - 1);
        self.follow_selection(height);
    }

    fn follow_selection(&mut self, height: usize) {
        let height = height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        self.scroll = self.scroll.min(self.visible.len().saturating_sub(height));
    }

    /// Reads the directory and rebuilds everything derived from it.
    fn read(&mut self) {
        self.entries.clear();
        self.error = None;
        if let Some(parent) = self.dir.parent() {
            self.entries.push(BrowserEntry {
                name: "..".into(),
                path: parent.to_path_buf(),
                is_dir: true,
                parent: true,
            });
        }
        match std::fs::read_dir(&self.dir) {
            Ok(reader) => {
                let mut found: Vec<BrowserEntry> = reader
                    .filter_map(Result::ok)
                    .map(|entry| {
                        // `file_type` is one `lstat` and `is_dir` on the path is
                        // another; the second also follows symlinks, which is
                        // what makes a link to a directory behave like the
                        // directory it points at.
                        let path = entry.path();
                        BrowserEntry {
                            name: entry.file_name().to_string_lossy().into_owned(),
                            is_dir: path.is_dir(),
                            path,
                            parent: false,
                        }
                    })
                    .collect();
                // Directories first, then names, case-insensitively: the order
                // the explorer already uses, so the same tree looks the same in
                // both panes.
                found.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                self.entries.extend(found);
            }
            Err(err) => {
                log::warn!("could not list {}: {err}", self.dir.display());
                self.error = Some(err.to_string());
            }
        }
        self.selected = 0;
        self.scroll = 0;
        self.rebuild_visible();
    }

    /// Applies the filter: a case-insensitive substring of the name.
    ///
    /// Dotfiles are left out until the filter starts with a dot, which is the
    /// same bargain the explorer strikes with `show_hidden` — a listing whose
    /// first forty rows are `.DS_Store` and friends is not a listing anybody
    /// browses — except that here the way back in is a single keystroke.
    ///
    /// A filter that looks like a path (it has a separator in it) matches
    /// nothing and is meant to: it is being typed as an answer rather than as a
    /// search, and `browser_target` reads it as one.
    fn rebuild_visible(&mut self) {
        let query = self.filter.value.trim().to_lowercase();
        let hidden = query.starts_with('.');
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.parent
                    || ((hidden || !entry.name.starts_with('.'))
                        && (query.is_empty() || entry.name.to_lowercase().contains(&query)))
            })
            .map(|(index, _)| index)
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        dir
    }

    fn names(browser: &Browser) -> Vec<String> {
        browser.rows().map(|entry| entry.name.clone()).collect()
    }

    #[test]
    fn a_listing_puts_the_parent_first_then_directories_then_files() {
        let dir = fixture();
        let browser = Browser::new(dir.path());
        assert_eq!(
            names(&browser),
            vec!["..", "docs", "src", "Cargo.toml", "README.md"],
            "and the dotfile is not in it"
        );
    }

    #[test]
    fn typing_a_filter_aims_the_selection_at_the_first_real_match() {
        let dir = fixture();
        let mut browser = Browser::new(dir.path());
        assert_eq!(browser.selected_entry().unwrap().name, "..");

        browser.filter = InputField::new("read");
        browser.refilter(10);
        assert_eq!(
            browser.selected_entry().unwrap().name,
            "README.md",
            "otherwise Enter after typing a name would walk up a directory"
        );
    }

    #[test]
    fn the_filter_narrows_the_listing_but_never_hides_the_way_out() {
        let dir = fixture();
        let mut browser = Browser::new(dir.path());
        browser.filter = InputField::new("RE");
        browser.refilter(10);
        assert_eq!(
            names(&browser),
            vec!["..", "README.md"],
            "matching is case-insensitive and `..` is always reachable"
        );
    }

    #[test]
    fn a_filter_that_starts_with_a_dot_is_how_hidden_files_are_reached() {
        let dir = fixture();
        let mut browser = Browser::new(dir.path());
        browser.filter = InputField::new(".h");
        browser.refilter(10);
        assert_eq!(names(&browser), vec!["..", ".hidden"]);
    }

    #[test]
    fn the_selection_stays_on_the_same_entry_across_a_filter_edit() {
        let dir = fixture();
        let mut browser = Browser::new(dir.path());
        browser.step(2, 10);
        assert_eq!(browser.selected_entry().unwrap().name, "src");

        browser.filter = InputField::new("s");
        browser.refilter(10);
        assert_eq!(
            browser.selected_entry().unwrap().name,
            "src",
            "the row moved, and the selection went with it"
        );
    }

    #[test]
    fn a_filter_that_matches_nothing_leaves_the_selection_on_the_parent_row() {
        let dir = fixture();
        let mut browser = Browser::new(dir.path());
        browser.step(3, 10);
        browser.filter = InputField::new("zzz");
        browser.refilter(10);
        assert_eq!(names(&browser), vec![".."]);
        assert_eq!(browser.selected(), 0);
    }

    #[test]
    fn opening_a_directory_clears_the_filter_and_starts_at_the_top() {
        let dir = fixture();
        let mut browser = Browser::new(dir.path());
        browser.filter = InputField::new("src");
        browser.refilter(10);
        browser.step(1, 10);

        let src = dir.path().join("src");
        browser.open_dir(&src);
        assert_eq!(browser.dir(), src);
        assert_eq!(browser.filter.value, "");
        assert_eq!(browser.selected(), 0);
        assert_eq!(names(&browser), vec![".."], "an empty directory, plus `..`");
    }

    #[test]
    fn open_folder_means_the_selected_directory_and_otherwise_the_listed_one() {
        let dir = fixture();
        let mut browser = Browser::new(dir.path());
        browser.step(2, 10);
        assert_eq!(browser.folder_target(), dir.path().join("src"));

        // `Cargo.toml`: a file is not a folder, so the listed directory is.
        browser.step(1, 10);
        assert_eq!(browser.folder_target(), dir.path());
    }

    /// Walking into a folder leaves the selection on `..`, which is when Open
    /// Folder is most likely to be pressed — and counting `..` opened the
    /// parent of the folder that had just been chosen.
    #[test]
    fn open_folder_on_the_way_out_row_still_means_the_folder_you_are_in() {
        let dir = fixture();
        let mut browser = Browser::new(&dir.path().join("src"));
        assert_eq!(browser.selected_entry().unwrap().name, "..");
        assert_eq!(browser.folder_target(), dir.path().join("src"));

        browser.step(-1, 10);
        assert_eq!(browser.folder_target(), dir.path().join("src"));
    }

    #[test]
    fn the_selection_is_clamped_at_both_ends_and_scrolls_into_view() {
        let dir = fixture();
        let mut browser = Browser::new(dir.path());
        browser.step(-1, 2);
        assert_eq!(browser.selected(), 0, "clamped at the top");

        browser.step(99, 2);
        assert_eq!(browser.selected(), browser.len() - 1);
        assert_eq!(
            browser.scroll(),
            browser.len() - 2,
            "the last row is on screen in a two-row window"
        );
    }

    #[test]
    fn a_directory_that_cannot_be_read_says_so_and_still_has_a_way_out() {
        let missing = std::path::Path::new("/no/such/directory/at/all");
        let browser = Browser::new(missing);
        assert!(browser.error().is_some());
        assert_eq!(names(&browser), vec![".."]);
    }
}
