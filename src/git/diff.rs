//! Unified diff: the side to show, and the lines `git diff` printed.
//!
//! The parsing is line-classification and nothing more (SPEC §36). A unified
//! diff is already the shape the viewer draws — one line per line, each one a
//! header, a hunk marker, an addition, a removal or context — so the work here
//! is to say *which* each line is, once, rather than to look at its first byte
//! again on every frame (ARCHITECTURE invariant 4).
//!
//! *First byte* is the part a conflicted file makes wrong. `git diff` of an
//! unmerged path is a **combined** diff: one marker column per parent, so a
//! two-parent merge writes two of them and ` +ours` is an addition whose first
//! byte is a space (ADR-045). The classifier therefore keeps two pieces of
//! state per file — how many columns there are, and whether a hunk has begun —
//! rather than reading one byte in isolation.

use unicode_segmentation::UnicodeSegmentation;

use crate::editor::coords::{line_width, DEFAULT_TAB_WIDTH};

/// The most lines a viewer holds. Past it the diff is cut and the title says
/// so, for the ADR-027 reason: a whole-file rewrite of a generated file is a
/// diff nobody reads and megabytes nobody asked to allocate.
pub const MAX_LINES: usize = 5000;

/// Which of the two diffs a file has is on screen (SPEC §36).
///
/// `git diff` and `git diff --cached` answer different questions — "what have I
/// not staged yet?" and "what would this commit contain?" — and a viewer that
/// showed one without saying which would be answering neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide {
    Worktree,
    Staged,
}

impl DiffSide {
    /// What the title calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::Staged => "staged",
        }
    }

    /// The sentence for a side that turned out to hold nothing.
    pub fn nothing(self) -> &'static str {
        match self {
            Self::Worktree => "No unstaged changes in",
            Self::Staged => "Nothing staged in",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Worktree => Self::Staged,
            Self::Staged => Self::Worktree,
        }
    }
}

/// What one line of the output is, which is what decides its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// `diff --git`, `index`, `---`, `+++`, and the mode and rename lines.
    Header,
    /// `@@ -1,7 +1,9 @@`.
    Hunk,
    Added,
    Removed,
    Context,
    /// `\ No newline at end of file`, and anything else git says about the
    /// file rather than about its contents.
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub text: String,
    pub kind: DiffLineKind,
    /// The bytes of `text` that this line does *not* share with the line it
    /// replaced, when it was paired with one (ADR-082).
    ///
    /// `None` for every line that has no partner and for every pair whose two
    /// lines have nothing in common: a mark on the whole line says the same
    /// thing the line's own colour already said, and a diff where everything is
    /// marked has marked nothing. The range includes neither the `+`/`-`
    /// prefix nor the common head and tail around the change.
    pub changed: Option<(usize, usize)>,
}

/// One file's diff, classified and counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub lines: Vec<DiffLine>,
    /// Whether `MAX_LINES` cut it short.
    pub truncated: bool,
    pub added: usize,
    pub removed: usize,
    /// The widest line in display cells, so horizontal scrolling has an end.
    pub width: usize,
    /// How many parents the widest hunk header claimed: one for an ordinary
    /// diff, two for a conflicted file in an ordinary merge, more for an
    /// octopus. Zero when the output held no hunk at all.
    pub parents: usize,
}

impl Diff {
    /// Classifies the output of one `git diff`.
    ///
    /// The classifier is stateful because the format is. `--- a/file` and
    /// `+++ b/file` begin with the same bytes as a removal and an addition,
    /// and a viewer that painted the file header red and green would be
    /// colouring the one part of the output that is not a change — but in a
    /// two-column combined diff `--- x` is also exactly how a line removed
    /// from both parents is printed. What separates them is not the bytes, it
    /// is where they are: a file header comes before the first hunk of its
    /// file, and everything after one is contents.
    pub fn parse(text: &str) -> Self {
        let mut diff = Self::default();
        let mut classifier = Classifier::default();
        for line in text.lines() {
            if diff.lines.len() == MAX_LINES {
                diff.truncated = true;
                break;
            }
            let kind = classifier.classify(line);
            match kind {
                DiffLineKind::Added => diff.added += 1,
                DiffLineKind::Removed => diff.removed += 1,
                DiffLineKind::Hunk => diff.parents = diff.parents.max(classifier.columns),
                _ => {}
            }
            diff.width = diff.width.max(line_width(line, DEFAULT_TAB_WIDTH).0);
            diff.lines.push(DiffLine {
                text: line.to_string(),
                kind,
                changed: None,
            });
        }
        diff.mark_changed_words();
        diff
    }

    /// Finds, for each replaced line, the part of it that actually changed
    /// (ADR-082).
    ///
    /// A hunk is a run of removals followed by a run of additions, so the two
    /// runs are paired off in order — but only when they are the same length.
    /// An unequal pair is not a line-for-line replacement: three lines becoming
    /// one is a rewrite, and pairing the first of each would mark two lines
    /// that have nothing to do with each other.
    fn mark_changed_words(&mut self) {
        let mut index = 0;
        while index < self.lines.len() {
            let removed = run_of(&self.lines, index, DiffLineKind::Removed);
            if removed == 0 {
                index += 1;
                continue;
            }
            let added = run_of(&self.lines, index + removed, DiffLineKind::Added);
            index += removed + added;
            if removed != added || removed > MAX_PAIRED_RUN {
                continue;
            }
            let first_added = index - added;
            for offset in 0..removed {
                let (before, after) = (
                    self.lines[first_added - removed + offset].text.clone(),
                    self.lines[first_added + offset].text.clone(),
                );
                let Some((old_range, new_range)) = changed_words(&before, &after) else {
                    continue;
                };
                self.lines[first_added - removed + offset].changed = Some(old_range);
                self.lines[first_added + offset].changed = Some(new_range);
            }
        }
    }

    /// Whether this is a combined diff — the shape `git diff` takes for an
    /// unmerged path, with one marker column per parent instead of one.
    ///
    /// The viewer says so in its title: two columns of `+` and `-` mean
    /// something different from one, and a reader who has not been told which
    /// they are looking at will read `+ theirs` as an ordinary addition.
    pub fn is_combined(&self) -> bool {
        self.parents > 1
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// `+12 −3`, for the title. The minus is U+2212 rather than a hyphen so it
    /// reads as a pair with the plus at any font size.
    pub fn summary(&self) -> String {
        format!("+{} −{}", self.added, self.removed)
    }
}

/// The longest run of replaced lines whose words are compared.
///
/// Beyond it the pair is left with the whole-line colouring it had before
/// (ADR-082). The comparison is linear in the length of a line and this bounds
/// how many of them one hunk can ask for, which is what keeps a diff of a
/// generated file — thousands of lines replaced at once — from spending a
/// visible fraction of the read on marking changes nobody is going to look at
/// one by one.
const MAX_PAIRED_RUN: usize = 200;

/// How much of the shorter of two lines has to be common to them before the
/// difference between them is worth marking, as a reciprocal: four is a
/// quarter (ADR-082).
const MIN_SHARED_FRACTION: usize = 4;

/// The longest line whose words are compared.
///
/// A minified bundle is one line of half a megabyte, and the common prefix of
/// two of them is most of that: the walk is cheap per cluster but there is no
/// reading such a mark anyway, so past this the line keeps its whole-line
/// colour.
const MAX_PAIRED_LINE: usize = 2000;

/// How many lines of `kind` there are in a row, starting at `from`.
fn run_of(lines: &[DiffLine], from: usize, kind: DiffLineKind) -> usize {
    lines[from.min(lines.len())..]
        .iter()
        .take_while(|line| line.kind == kind)
        .count()
}

/// The middles of two lines that replaced one another: what is left of each
/// once the head and the tail they share have been taken off.
///
/// Grapheme clusters and not bytes, so a mark can never start or end inside a
/// character — a combining accent left on the wrong side of the boundary would
/// be drawn as its own glyph. The returned ranges are byte ranges into the
/// lines *including* their `+`/`-` prefix, because that is what the renderer
/// has; the prefix itself is never in one, since the two lines differ there and
/// the common-prefix walk therefore starts after it.
///
/// `None` when there is nothing useful to say: the lines share no head and no
/// tail, or one of them is longer than `MAX_PAIRED_LINE`.
fn changed_words(before: &str, after: &str) -> Option<((usize, usize), (usize, usize))> {
    if before.len() > MAX_PAIRED_LINE || after.len() > MAX_PAIRED_LINE {
        return None;
    }
    // The marker column is not part of the text: `-x` and `+x` are the same
    // line, and comparing the columns would find every pair different at
    // byte 0 and every pair identical after it.
    let (old_body, new_body) = (skip_marker(before), skip_marker(after));
    let old_text = &before[old_body..];
    let new_text = &after[new_body..];
    if old_text == new_text {
        return None;
    }

    let head = common_prefix(old_text, new_text);
    let tail = common_suffix(&old_text[head..], &new_text[head..]);
    // Two lines that share almost nothing are two different lines, not one
    // line edited: `alpha` and `omega` have a final `a` in common, and marking
    // `alph` and `omeg` says nothing the two colours did not already say. A
    // quarter of the shorter line is the line between "this was edited" and
    // "these happen to share a letter".
    let shared = head + tail;
    let shortest = old_text.len().min(new_text.len());
    if shared * MIN_SHARED_FRACTION < shortest {
        return None;
    }
    Some((
        (old_body + head, before.len() - tail),
        (new_body + head, after.len() - tail),
    ))
}

/// How many bytes of the marker column a diff line begins with.
///
/// One for an ordinary diff and one per parent for a combined one — but the
/// count is taken from the line itself rather than from `parents`, because a
/// pair is compared line by line and this is the only thing that has to agree
/// between the two of them.
fn skip_marker(line: &str) -> usize {
    line.bytes()
        .take_while(|byte| matches!(byte, b'+' | b'-' | b' '))
        .count()
        .min(1)
}

/// The bytes two strings share at the front, on a cluster boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    let mut shared = 0;
    for ((at, left), (_, right)) in a.grapheme_indices(true).zip(b.grapheme_indices(true)) {
        if left != right {
            return at;
        }
        shared = at + left.len();
    }
    shared
}

/// The bytes two strings share at the back, on a cluster boundary.
fn common_suffix(a: &str, b: &str) -> usize {
    let left: Vec<&str> = a.graphemes(true).collect();
    let right: Vec<&str> = b.graphemes(true).collect();
    let mut shared = 0;
    for (x, y) in left.iter().rev().zip(right.iter().rev()) {
        if x != y {
            break;
        }
        shared += x.len();
    }
    shared
}

/// The three spellings of the line that opens a file's diff. Each one resets
/// the classifier: a diff of several files is several state machines in a row.
const FILE_HEADERS: [&str; 3] = ["diff --git", "diff --cc", "diff --combined"];

/// The prefixes git writes, before the first hunk, for the parts of the output
/// that describe the file rather than its contents.
const PREAMBLE: [&str; 12] = [
    "index ",
    "old mode ",
    "new mode ",
    "new file mode ",
    "deleted file mode ",
    "similarity index ",
    "dissimilarity index ",
    "rename from ",
    "rename to ",
    "copy from ",
    "copy to ",
    "Binary files ",
];

/// One `git diff`'s worth of state: enough to tell a file header from a line
/// of a file, which the bytes alone cannot (see `Diff::parse`).
#[derive(Debug)]
struct Classifier {
    /// Marker columns in the hunks of the file being read: one for an ordinary
    /// diff, one per parent for a combined one. Taken from the hunk header,
    /// which spells it out — `@@@ … @@@` is two.
    columns: usize,
    /// Whether a hunk of this file has begun. Before the first one every line
    /// describes the file; after it every line is contents.
    in_hunk: bool,
    /// Whether the output opened with a commit describing itself (ADR-069).
    ///
    /// `git diff` opens with `diff --git`; `git show` opens with `commit
    /// <oid>` and then says who wrote it, when, and the whole message — and a
    /// message is prose, so a paragraph beginning `- ` is a bullet and not a
    /// removed line. Everything from there to the first file header is
    /// therefore the commit rather than a change. Only the *first* line can
    /// turn this on, which is what keeps a diff whose contents happen to
    /// contain the word from being read as one.
    commit_header: bool,
    /// Whether the line about to be classified is the first of the output.
    first_line: bool,
}

impl Default for Classifier {
    fn default() -> Self {
        Self {
            columns: 1,
            in_hunk: false,
            commit_header: false,
            first_line: true,
        }
    }
}

impl Classifier {
    fn classify(&mut self, line: &str) -> DiffLineKind {
        let first_line = std::mem::take(&mut self.first_line);
        if FILE_HEADERS.iter().any(|prefix| line.starts_with(prefix)) {
            *self = Self {
                first_line: false,
                ..Self::default()
            };
            return DiffLineKind::Header;
        }
        if first_line && line.starts_with("commit ") {
            self.commit_header = true;
            return DiffLineKind::Header;
        }
        if self.commit_header {
            return DiffLineKind::Header;
        }
        if let Some(columns) = hunk_columns(line) {
            self.columns = columns;
            self.in_hunk = true;
            return DiffLineKind::Hunk;
        }
        if !self.in_hunk {
            // `--- ` and `+++ ` are the file's two halves being named here and
            // a pair of removals in a combined hunk once one has started, so
            // they are only headers on this side of the first `@@`.
            if line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line == "---"
                || line == "+++"
            {
                return DiffLineKind::Header;
            }
            if PREAMBLE.iter().any(|prefix| line.starts_with(prefix)) {
                return DiffLineKind::Header;
            }
            // `git diff --cached` of an unmerged path prints this instead of a
            // diff: there is no single staged blob to diff against.
            if line.starts_with("* Unmerged path ") {
                return DiffLineKind::Meta;
            }
        }
        if line.starts_with('\\') {
            return DiffLineKind::Meta;
        }
        markers(line, self.columns)
    }
}

/// The parent count a hunk header claims, as a column count.
///
/// `@@ … @@` is one column, `@@@ … @@@` two, and an octopus adds one `@` per
/// parent. Nothing else in the output starts with `@`: inside a hunk every
/// line begins with its marker columns, so an `@` at column zero is a hunk
/// header or it is not a body line at all.
fn hunk_columns(line: &str) -> Option<usize> {
    let ats = line.bytes().take_while(|&b| b == b'@').count();
    (ats >= 2).then(|| ats - 1)
}

/// Reads the marker columns of a body line.
///
/// A combined diff puts one column per parent, so an addition can begin with a
/// space: ` +ours` is "unchanged against the first parent, added against the
/// second". A row cannot be both — the lines a `-` marks are the ones missing
/// from the result, and a `+` row is in it — so the two are read in one pass
/// and `+` is answered first.
fn markers(line: &str, columns: usize) -> DiffLineKind {
    let mut kind = DiffLineKind::Context;
    for c in line.chars().take(columns) {
        match c {
            '+' => return DiffLineKind::Added,
            '-' => kind = DiffLineKind::Removed,
            _ => {}
        }
    }
    kind
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/a.txt b/a.txt
index 7898192..6178079 100644
--- a/a.txt
+++ b/a.txt
@@ -1 +1,2 @@
 a
+b
-c
\\ No newline at end of file
";

    fn kinds(text: &str) -> Vec<DiffLineKind> {
        Diff::parse(text)
            .lines
            .into_iter()
            .map(|l| l.kind)
            .collect()
    }

    /// `git show` opens with the commit rather than with a file, and none of
    /// what it says about itself is a change (ADR-069).
    #[test]
    fn the_header_git_show_writes_above_a_patch_is_not_read_for_changes() {
        use DiffLineKind::{Added, Header, Hunk};
        let text = "\
commit 0123456789abcdef0123456789abcdef01234567
Author: Ada <ada@example.com>
Date:   2026-09-10

    subject

    -1 was the old way and +1 is the new one

diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
+b
";
        let diff = Diff::parse(text);
        let kinds: Vec<DiffLineKind> = diff.lines.iter().map(|l| l.kind).collect();
        assert!(
            kinds[..8].iter().all(|kind| *kind == Header),
            "the commit describes itself: {kinds:?}"
        );
        assert_eq!(kinds[11], Hunk);
        assert_eq!(kinds[12], Added);
        assert_eq!(
            (diff.added, diff.removed),
            (1, 0),
            "a message that mentions -1 and +1 is not a change"
        );
    }

    #[test]
    fn every_line_of_a_diff_is_classified() {
        use DiffLineKind::{Added, Context, Header, Hunk, Meta, Removed};
        assert_eq!(
            kinds(SAMPLE),
            vec![Header, Header, Header, Header, Hunk, Context, Added, Removed, Meta]
        );
    }

    #[test]
    fn the_file_header_is_not_an_addition_or_a_removal() {
        let diff = Diff::parse(SAMPLE);
        // `--- a/a.txt` and `+++ b/a.txt` begin like a change and are not one.
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert_eq!(diff.summary(), "+1 −1");
    }

    #[test]
    fn a_diff_of_nothing_is_empty_rather_than_one_blank_line() {
        let diff = Diff::parse("");
        assert!(diff.is_empty());
        assert_eq!(diff.len(), 0);
        assert_eq!(diff.summary(), "+0 −0");
    }

    #[test]
    fn a_rename_and_a_binary_file_are_headers() {
        use DiffLineKind::Header;
        let text = "\
diff --git a/old.txt b/new.txt
similarity index 100%
rename from old.txt
rename to new.txt
Binary files a/x.png and b/x.png differ
";
        assert_eq!(kinds(text), vec![Header; 5]);
    }

    #[test]
    fn a_long_diff_stops_at_the_cap_and_says_so() {
        let text: String = (0..MAX_LINES + 100).map(|i| format!("+{i}\n")).collect();
        let diff = Diff::parse(&text);
        assert_eq!(diff.len(), MAX_LINES);
        assert!(diff.truncated);
        assert_eq!(diff.added, MAX_LINES, "only what was kept is counted");
    }

    #[test]
    fn the_width_is_in_cells_with_tabs_expanded() {
        let diff = Diff::parse("+\tx\n+ab\n");
        // The `+` is one cell, the tab advances from column one to the next
        // stop, and `x` is the cell after it.
        assert_eq!(diff.width, DEFAULT_TAB_WIDTH + 1);
    }

    /// `git diff` of a conflicted file, exactly as git prints it: two marker
    /// columns, one per parent of the stopped merge.
    const COMBINED: &str = "\
diff --cc f.txt
index daf31e1,594dc4f..0000000
--- a/f.txt
+++ b/f.txt
@@@ -1,3 -1,3 +1,7 @@@
  one
++<<<<<<< HEAD
 +OURS
++=======
+ THEIRS
++>>>>>>> other
  three
";

    #[test]
    fn both_marker_columns_of_a_combined_diff_are_read() {
        use DiffLineKind::{Added, Context, Header, Hunk};
        assert_eq!(
            kinds(COMBINED),
            vec![
                Header, Header, Header, Header, Hunk,
                Context, // `  one` — unchanged against both parents
                Added,   // `++<<<<<<< HEAD`
                Added,   // ` +OURS` — added against the second parent only
                Added,   // `++=======`
                Added,   // `+ THEIRS` — added against the first parent only
                Added,   // `++>>>>>>> other`
                Context, // `  three`
            ]
        );
        let diff = Diff::parse(COMBINED);
        assert_eq!((diff.added, diff.removed), (5, 0));
    }

    #[test]
    fn a_combined_diff_says_how_many_parents_it_has() {
        let diff = Diff::parse(COMBINED);
        assert_eq!(diff.parents, 2);
        assert!(diff.is_combined());

        let ordinary = Diff::parse(SAMPLE);
        assert_eq!(ordinary.parents, 1);
        assert!(!ordinary.is_combined());

        // Nothing to read is not a merge.
        assert!(!Diff::parse("").is_combined());
    }

    /// An octopus writes one column per parent, and the hunk header counts
    /// them out in `@`s.
    #[test]
    fn three_columns_are_read_as_three() {
        use DiffLineKind::{Added, Context, Hunk, Removed};
        let text = "\
@@@@ -1,2 -1,2 -1,2 +1,2 @@@@
   kept
+++ added against all three
  - gone from the third
";
        assert_eq!(kinds(text), vec![Hunk, Context, Added, Removed]);
        assert_eq!(Diff::parse(text).parents, 3);
    }

    /// The one case the bytes alone cannot decide: in a two-column diff a line
    /// removed from both parents is printed `--` followed by its own text, and
    /// a line whose text begins `- ` makes that `--- …` — the file header's
    /// spelling. What tells them apart is the hunk that has begun by then.
    #[test]
    fn a_removal_that_looks_like_a_file_header_is_still_a_removal() {
        use DiffLineKind::{Added, Header, Hunk, Removed};
        let text = "\
diff --cc list.md
--- a/list.md
+++ b/list.md
@@@ -1,2 -1,2 +1,2 @@@
--- a bullet both parents dropped
+++ b line added against both
";
        assert_eq!(
            kinds(text),
            vec![Header, Header, Header, Hunk, Removed, Added]
        );
    }

    #[test]
    fn an_unmerged_path_has_nothing_staged_to_diff_and_says_so() {
        // What `git diff --cached` prints for a conflicted file. It is git
        // talking about the file, not a line of it, so it is not context.
        assert_eq!(kinds("* Unmerged path f.txt\n"), vec![DiffLineKind::Meta]);
    }

    #[test]
    fn each_file_of_a_multi_file_diff_gets_its_own_columns() {
        use DiffLineKind::{Added, Context, Header, Hunk};
        let text = "\
diff --cc a.txt
@@@ -1,1 -1,1 +1,1 @@@
 +ours
diff --git b.txt b.txt
@@ -1,1 +1,1 @@
 context
";
        assert_eq!(
            kinds(text),
            vec![Header, Hunk, Added, Header, Hunk, Context],
            "` +ours` is an addition in the two-column file and ` context` is \
             context in the one-column file that follows it"
        );
    }

    #[test]
    fn a_side_names_itself_and_its_opposite() {
        assert_eq!(DiffSide::Worktree.label(), "worktree");
        assert_eq!(DiffSide::Worktree.other(), DiffSide::Staged);
        assert_eq!(DiffSide::Staged.other(), DiffSide::Worktree);
        assert!(DiffSide::Staged.nothing().starts_with("Nothing staged"));
    }
    /// The example from the task: one call gains `.saturating_sub(GAP)`, and
    /// the mark is that and nothing else (ADR-082).
    #[test]
    fn a_small_change_inside_a_long_line_is_marked_on_its_own() {
        let diff = Diff::parse(concat!(
            "@@ -1,3 +1,3 @@\n",
            " fn budget(width: u16, left: u16) -> usize {\n",
            "-    width.saturating_sub(left.max(MIN_LEFT)) as usize\n",
            "+    width.saturating_sub(left.max(MIN_LEFT)).saturating_sub(GAP) as usize\n",
            " }\n",
        ));
        let removed = diff
            .lines
            .iter()
            .find(|line| line.kind == DiffLineKind::Removed)
            .unwrap();
        let added = diff
            .lines
            .iter()
            .find(|line| line.kind == DiffLineKind::Added)
            .unwrap();
        let (from, to) = added.changed.expect("the added line is marked");
        assert_eq!(&added.text[from..to], ".saturating_sub(GAP)");
        // Nothing was taken out of the removed line, so its own range is
        // empty — and it sits exactly where the insertion went in.
        let (from, to) = removed.changed.expect("its partner is marked too");
        assert_eq!(&removed.text[from..to], "");
        assert_eq!(
            &removed.text[..from],
            "-    width.saturating_sub(left.max(MIN_LEFT))"
        );
    }

    /// A run of removals and a run of additions of different lengths is a
    /// rewrite and not a line-for-line replacement: nothing is paired.
    #[test]
    fn an_unbalanced_run_is_left_with_its_whole_line_colour() {
        let diff = Diff::parse(concat!(
            "@@ -1,3 +1,1 @@\n",
            "-one\n",
            "-two\n",
            "-three\n",
            "+one two three\n",
        ));
        assert!(diff.lines.iter().all(|line| line.changed.is_none()));
    }

    /// Two lines with nothing in common at either end are two different lines,
    /// and marking all of both would say what the colours already say.
    #[test]
    fn a_pair_with_nothing_in_common_is_not_marked() {
        let diff = Diff::parse("@@ -1 +1 @@\n-alpha\n+omega\n");
        assert!(diff.lines.iter().all(|line| line.changed.is_none()));
    }

    /// Every line of a balanced run is paired with the line at the same place
    /// in the other run, not with the first one.
    #[test]
    fn a_balanced_run_is_paired_off_in_order() {
        let diff = Diff::parse(concat!(
            "@@ -1,2 +1,2 @@\n",
            "-let a = 1;\n",
            "-let b = 2;\n",
            "+let a = 11;\n",
            "+let b = 22;\n",
        ));
        let marked: Vec<&str> = diff
            .lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .map(|line| {
                let (from, to) = line.changed.expect("marked");
                &line.text[from..to]
            })
            .collect();
        assert_eq!(marked, vec!["1", "2"]);
    }

    /// A mark never starts or ends inside a character: the walk is over
    /// grapheme clusters, so a combining accent cannot be split off its letter.
    #[test]
    fn a_mark_falls_on_character_boundaries() {
        let diff = Diff::parse("@@ -1 +1 @@\n-日本語 text\n+日本 text\n");
        for line in &diff.lines {
            let Some((from, to)) = line.changed else {
                continue;
            };
            assert!(line.text.is_char_boundary(from), "{}", line.text);
            assert!(line.text.is_char_boundary(to), "{}", line.text);
            // Slicing it must not panic, which is the point of the assertions
            // above.
            let _ = &line.text[from..to];
        }
    }

    /// A line long enough to be a bundle keeps its whole-line colour: the
    /// comparison is bounded so a generated file cannot make a diff slow to
    /// read (ADR-082).
    #[test]
    fn a_very_long_line_is_not_compared() {
        let long = "x".repeat(MAX_PAIRED_LINE + 10);
        let diff = Diff::parse(&format!("@@ -1 +1 @@\n-{long}a\n+{long}b\n"));
        assert!(diff.lines.iter().all(|line| line.changed.is_none()));
    }

    /// And so does a replacement of more lines at once than anybody reads word
    /// by word.
    #[test]
    fn a_very_long_run_is_not_compared() {
        let mut text = String::from("@@ -1 +1 @@\n");
        let lines = MAX_PAIRED_RUN + 1;
        for i in 0..lines {
            text.push_str(&format!("-line {i} old\n"));
        }
        for i in 0..lines {
            text.push_str(&format!("+line {i} new\n"));
        }
        let diff = Diff::parse(&text);
        assert!(diff.lines.iter().all(|line| line.changed.is_none()));
    }
}
