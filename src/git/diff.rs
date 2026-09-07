//! Unified diff: the side to show, and the lines `git diff` printed.
//!
//! The parsing is line-classification and nothing more (SPEC §36). A unified
//! diff is already the shape the viewer draws — one line per line, each one a
//! header, a hunk marker, an addition, a removal or context — so the work here
//! is to say *which* each line is, once, rather than to look at its first byte
//! again on every frame (ARCHITECTURE invariant 4).

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
}

impl Diff {
    /// Classifies the output of one `git diff`.
    ///
    /// Order matters at the top: `--- a/file` and `+++ b/file` begin with the
    /// same bytes as a removal and an addition, and a viewer that painted the
    /// file header red and green would be colouring the one part of the output
    /// that is not a change.
    pub fn parse(text: &str) -> Self {
        let mut diff = Self::default();
        for line in text.lines() {
            if diff.lines.len() == MAX_LINES {
                diff.truncated = true;
                break;
            }
            let kind = classify(line);
            match kind {
                DiffLineKind::Added => diff.added += 1,
                DiffLineKind::Removed => diff.removed += 1,
                _ => {}
            }
            diff.width = diff.width.max(line_width(line, DEFAULT_TAB_WIDTH).0);
            diff.lines.push(DiffLine {
                text: line.to_string(),
                kind,
            });
        }
        diff
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

/// The prefixes git writes for the parts of the output that describe the file
/// rather than its contents.
const HEADERS: [&str; 10] = [
    "diff --git",
    "diff --cc",
    "diff --combined",
    "index ",
    "old mode ",
    "new mode ",
    "new file mode ",
    "deleted file mode ",
    "similarity index ",
    "dissimilarity index ",
];

fn classify(line: &str) -> DiffLineKind {
    if line.starts_with("--- ") || line.starts_with("+++ ") || line == "---" || line == "+++" {
        return DiffLineKind::Header;
    }
    if HEADERS.iter().any(|prefix| line.starts_with(prefix))
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("copy from ")
        || line.starts_with("copy to ")
        || line.starts_with("Binary files ")
    {
        return DiffLineKind::Header;
    }
    match line.chars().next() {
        Some('@') => DiffLineKind::Hunk,
        Some('+') => DiffLineKind::Added,
        Some('-') => DiffLineKind::Removed,
        Some('\\') => DiffLineKind::Meta,
        _ => DiffLineKind::Context,
    }
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

    #[test]
    fn a_side_names_itself_and_its_opposite() {
        assert_eq!(DiffSide::Worktree.label(), "worktree");
        assert_eq!(DiffSide::Worktree.other(), DiffSide::Staged);
        assert_eq!(DiffSide::Staged.other(), DiffSide::Worktree);
        assert!(DiffSide::Staged.nothing().starts_with("Nothing staged"));
    }
}
