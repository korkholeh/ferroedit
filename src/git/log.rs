//! Commit history: what one commit looks like, and which history is being read.
//!
//! The three histories the editor offers — the repository's, one file's, and
//! one range of lines' — differ only in the arguments `git log` is given
//! (ADR-068). They therefore share one shape here and one viewer above, rather
//! than being three features that happen to look alike.
//!
//! Like `parser.rs`, this module knows nothing about subprocesses: it takes the
//! bytes `git log` wrote and produces commits, so it is testable against
//! recorded output.

use std::path::{Path, PathBuf};

use super::models::GitError;

/// The most commits one viewer holds.
///
/// The ADR-027 reason the diff has `MAX_LINES` and the status has
/// `MAX_ENTRIES`: a repository with a hundred thousand commits in it is a
/// list nobody scrolls to the end of and megabytes nobody asked to allocate.
/// The viewer says `(cut)` in its title when the limit was the thing that
/// ended the list, so a search that finds nothing has a visible reason.
pub const MAX_COMMITS: usize = 2000;

/// The record `git log --format` is asked for, and what `parse_log` reads.
///
/// The fields are separated by U+001F and the records by NUL (`-z`), because
/// both are bytes a commit message cannot contain — a subject with a `|` in it
/// is ordinary, and one with a newline in it is what `%s` already collapses.
/// The body is last on purpose: it is the one field that can contain anything,
/// including a `%x1f` somebody pasted into a commit message, so the reader
/// splits off the five fixed fields and keeps the whole remainder as the body.
pub const FORMAT: &str = "--format=%H%x1f%h%x1f%an%x1f%ad%x1f%s%x1f%b";

/// One line of the log viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The full object name, which is what every command here is given: an
    /// abbreviation is display, and one that has become ambiguous since the
    /// list was read would resolve to the wrong commit or to none.
    pub oid: String,
    /// The abbreviation git itself chose, for the column on screen.
    pub short: String,
    pub author: String,
    /// `--date=short`, so it sorts and aligns: `2026-09-10`.
    pub date: String,
    pub subject: String,
    /// Everything under the subject line, as the author wrote it — trailers,
    /// paragraphs, blank lines and all.
    ///
    /// Read with the list rather than on demand (ADR-080): the viewer's column
    /// shows a subject cut to whatever the pane had left, and the reader who
    /// wants the rest of it wants it *now*. One `git log` already ran; a second
    /// subprocess per keystroke would put a spinner on a question git has
    /// already answered.
    pub body: String,
}

impl Commit {
    /// Whether `needle` — already lowercase — appears anywhere in the row.
    ///
    /// The oid is matched on its full length as well as its abbreviation, so a
    /// hash pasted from somewhere else finds its commit.
    pub fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let hit = |field: &str| field.to_lowercase().contains(needle);
        hit(&self.subject) || hit(&self.author) || hit(&self.date) || self.oid.starts_with(needle)
    }
}

/// Which history a viewer is showing (ADR-068).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogScope {
    /// Everything reachable from `HEAD`.
    Repository,
    /// One file, followed across renames.
    File(PathBuf),
    /// One range of lines of one file, followed across the moves and the
    /// renames git can trace — `git log -L`.
    Lines {
        path: PathBuf,
        /// One-based and inclusive, as git counts them and as the editor shows
        /// them in its gutter.
        first: usize,
        last: usize,
    },
}

impl LogScope {
    /// The file this history is about, when it is about one. The commit diff
    /// is narrowed to it, so a commit that touched forty files shows the one
    /// the user was reading.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Repository => None,
            Self::File(path) | Self::Lines { path, .. } => Some(path),
        }
    }

    /// What the tab strip calls it: short, because a tab is a dozen cells wide.
    pub fn name(&self) -> String {
        match self {
            Self::Repository => "Log".to_string(),
            Self::File(path) => format!("Log: {}", file_name(path)),
            Self::Lines { path, first, last } => {
                format!("Log: {}:{first}-{last}", file_name(path))
            }
        }
    }

    /// What the viewer's own title calls it, in full.
    pub fn label(&self) -> String {
        match self {
            Self::Repository => "the repository".to_string(),
            Self::File(path) => path.display().to_string(),
            Self::Lines { path, first, last } => {
                format!("{}:{first}-{last}", path.display())
            }
        }
    }

    /// The arguments that select this history, after the format and the limit.
    ///
    /// `--follow` is on the file scope because a file that was renamed has a
    /// history on both sides of the rename and a viewer that stopped at it
    /// would be answering the wrong question. It is *not* on the line scope:
    /// `git log -L` refuses it, and traces the lines through renames itself.
    pub fn args(&self) -> Vec<std::ffi::OsString> {
        use std::ffi::OsString;
        match self {
            Self::Repository => Vec::new(),
            Self::File(path) => vec![
                OsString::from("--follow"),
                OsString::from("--"),
                path.as_os_str().to_os_string(),
            ],
            Self::Lines { path, first, last } => {
                // `-L first,last:path` is one argument, so the path cannot be
                // separated from the range by a `--` and a file called `-x`
                // cannot be reached this way at all. It is git's own syntax;
                // the range in front of the colon is what keeps the argument
                // from ever starting with a dash.
                let mut spec = OsString::from(format!("{first},{last}:"));
                spec.push(path.as_os_str());
                vec![
                    OsString::from("-L"),
                    spec,
                    // `-L` implies `-p`, and the list wants commits and not
                    // patches: the diff is read when a commit is chosen.
                    OsString::from("--no-patch"),
                ]
            }
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Reads what `FORMAT` and `-z` produced.
///
/// Lossy on purpose, like the diff: an author name in another encoding must
/// not cost the user the rest of the history.
pub fn parse_log(output: &[u8]) -> Result<Vec<Commit>, GitError> {
    let text = String::from_utf8_lossy(output);
    let mut commits = Vec::new();
    for record in text.split('\0') {
        // `-z` terminates rather than separates, so the last split is empty.
        // A record can also carry the newline `-L` writes between its own
        // output and the next commit, which is not part of any field.
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        // Six splits at most: the sixth is the body, which keeps every
        // separator inside it rather than being cut at the first one.
        let mut fields = record.splitn(6, '\u{1f}');
        let (Some(oid), Some(short), Some(author), Some(date), Some(subject)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(GitError::Parse(format!("log record {record:?}")));
        };
        let body = fields.next().unwrap_or_default();
        commits.push(Commit {
            oid: oid.to_string(),
            short: short.to_string(),
            author: author.to_string(),
            date: date.to_string(),
            // `-L` prints its patch after the subject when `--no-patch` is not
            // honoured by an older git; everything past the first line of the
            // subject field is not the subject.
            subject: subject.lines().next().unwrap_or_default().to_string(),
            // `%b` ends with the newline git puts after every commit message,
            // and a body of trailing blank lines is rows of an empty box.
            body: body.trim_end().to_string(),
        });
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(oid: &str, subject: &str) -> String {
        format!(
            "{oid}\u{1f}{short}\u{1f}Ada\u{1f}2026-09-10\u{1f}{subject}\u{1f}\0",
            short = &oid[..7]
        )
    }

    #[test]
    fn a_log_is_read_field_by_field() {
        let text = format!(
            "{}{}",
            record("0123456789abcdef", "first"),
            record("fedcba9876543210", "second")
        );
        let commits = parse_log(text.as_bytes()).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short, "0123456");
        assert_eq!(commits[0].subject, "first");
        assert_eq!(commits[1].oid, "fedcba9876543210");
        assert_eq!(commits[1].author, "Ada");
    }

    /// `git log -L` writes a newline between its records even under `-z`.
    #[test]
    fn the_newline_a_line_log_writes_between_records_is_not_a_field() {
        let text = format!(
            "{}\n{}",
            record("0123456789abcdef", "first"),
            record("fedcba9876543210", "second")
        );
        let commits = parse_log(text.as_bytes()).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[1].subject, "second");
    }

    #[test]
    fn an_empty_log_is_no_commits_and_not_a_failure() {
        assert!(parse_log(b"").unwrap().is_empty());
    }

    #[test]
    fn a_record_missing_a_field_is_a_parse_error() {
        let err = parse_log("abc\u{1f}abc\0".as_bytes()).unwrap_err();
        assert!(matches!(err, GitError::Parse(_)), "{err}");
    }

    #[test]
    fn a_filter_matches_the_subject_the_author_and_the_hash() {
        let commits = parse_log(record("0123456789abcdef", "Fix the thing").as_bytes()).unwrap();
        let commit = &commits[0];
        assert!(commit.matches(""), "an empty filter keeps everything");
        assert!(commit.matches("fix the"));
        assert!(commit.matches("ada"));
        assert!(commit.matches("2026-09"));
        assert!(commit.matches("0123456"), "the abbreviation");
        assert!(commit.matches("0123456789ab"), "and past it");
        assert!(!commit.matches("nothing here"));
    }

    #[test]
    fn a_scope_names_itself_for_the_tab_strip_and_for_the_title() {
        let file = LogScope::File(PathBuf::from("src/git/log.rs"));
        assert_eq!(file.name(), "Log: log.rs");
        assert_eq!(file.label(), "src/git/log.rs");
        let lines = LogScope::Lines {
            path: PathBuf::from("src/git/log.rs"),
            first: 10,
            last: 24,
        };
        assert_eq!(lines.name(), "Log: log.rs:10-24");
        assert_eq!(lines.label(), "src/git/log.rs:10-24");
        assert_eq!(LogScope::Repository.name(), "Log");
    }

    #[test]
    fn the_line_scope_asks_for_a_range_and_not_for_a_pathspec() {
        let lines = LogScope::Lines {
            path: PathBuf::from("a b.rs"),
            first: 3,
            last: 4,
        };
        let args: Vec<String> = lines
            .args()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["-L", "3,4:a b.rs", "--no-patch"]);
    }
}
