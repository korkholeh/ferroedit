//! `git status --porcelain=v2 -z --branch` parser.
//!
//! Machine-readable output only (SPEC §30): the human form is localised,
//! reflowed and quoted, and none of that is a contract. With `-z` the records
//! are NUL-terminated and paths are printed raw, which is what makes a file
//! name with a space, a quote or a newline in it survive the trip.
//!
//! The parser works on bytes rather than on a `String` because a path is bytes
//! on the platforms FerroEdit targets. Only the fixed prefix of each record —
//! the codes, the modes and the object names — is required to be ASCII.

use std::path::PathBuf;

use super::models::GitError;
use super::models::{Change, FileEntry, Head, RepoStatus};

/// How many changed files the panel will hold.
///
/// A tree with more changes than this is a `git checkout` of something huge or
/// a mass rewrite; the list stops being browsable long before the limit, and
/// the cap is what keeps one status from allocating a hundred megabytes of
/// paths. The status says it was cut short rather than lying about a clean
/// tail (the `MAX_MATCHES` argument of ADR-027).
pub const MAX_ENTRIES: usize = 5_000;

/// Parses the whole output of one status invocation.
pub fn parse_status(output: &[u8]) -> Result<RepoStatus, GitError> {
    let mut status = RepoStatus::default();
    // `-z` terminates every record, so the split ends with an empty tail.
    let mut records = output.split(|byte| *byte == 0).filter(|r| !r.is_empty());

    while let Some(record) = records.next() {
        match record.first() {
            Some(b'#') => header(&mut status, record)?,
            Some(b'1') => {
                let entry = ordinary(record)?;
                push(&mut status, entry);
            }
            Some(b'2') => {
                // The original path of a rename is its own NUL-terminated
                // field, which is the only record that reads two.
                let original = records
                    .next()
                    .ok_or_else(|| malformed("a rename with no original path", record))?;
                let mut entry = ordinary_with_score(record)?;
                entry.original_path = Some(path_from(original));
                push(&mut status, entry);
            }
            Some(b'u') => {
                let entry = unmerged(record)?;
                push(&mut status, entry);
            }
            Some(b'?') => {
                let (_, path) = split_after(record, 1)
                    .ok_or_else(|| malformed("an untracked record with no path", record))?;
                push(
                    &mut status,
                    FileEntry {
                        path: path_from(path),
                        original_path: None,
                        index: Change::Unmodified,
                        worktree: Change::Untracked,
                    },
                );
            }
            // Ignored files are not asked for, and anything else is a record
            // type added to git after this was written: neither is a reason to
            // refuse the rest of the status.
            _ => log::debug!("skipping git status record {:?}", lossy(record)),
        }
    }

    Ok(status)
}

/// Appends an entry until the cap, and remembers that the cap was reached.
fn push(status: &mut RepoStatus, entry: FileEntry) {
    if status.entries.len() >= MAX_ENTRIES {
        status.truncated = true;
        return;
    }
    status.entries.push(entry);
}

/// `# branch.oid`, `# branch.head`, `# branch.upstream`, `# branch.ab`.
fn header(status: &mut RepoStatus, record: &[u8]) -> Result<(), GitError> {
    let text =
        std::str::from_utf8(record).map_err(|_| malformed("a header that is not UTF-8", record))?;
    let mut parts = text.splitn(3, ' ');
    // "#" itself.
    parts.next();
    let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
        // `# branch.oid` with no value cannot happen, and a header we do not
        // know about is not worth failing over.
        return Ok(());
    };
    match key {
        // A repository with no commits yet reports `(initial)`, which is not
        // an object name and must not be shown as one.
        "branch.oid" => status.oid = (value != "(initial)").then(|| short_oid(value)),
        "branch.head" => {
            status.head = Some(if value == "(detached)" {
                Head::Detached
            } else {
                Head::Branch(value.to_string())
            });
        }
        "branch.upstream" => status.upstream = Some(value.to_string()),
        "branch.ab" => {
            let mut counts = value.split(' ');
            status.ahead = signed(counts.next(), '+');
            status.behind = signed(counts.next(), '-');
        }
        _ => {}
    }
    Ok(())
}

/// `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
fn ordinary(record: &[u8]) -> Result<FileEntry, GitError> {
    entry_after(record, 8)
}

/// `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>` — one field more
/// than an ordinary record, and a second record holding the original path.
fn ordinary_with_score(record: &[u8]) -> Result<FileEntry, GitError> {
    entry_after(record, 9)
}

/// `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`
fn unmerged(record: &[u8]) -> Result<FileEntry, GitError> {
    entry_after(record, 10)
}

/// The three changed-file records differ only in how many fields stand between
/// the `XY` pair and the path, so one function reads all of them.
fn entry_after(record: &[u8], fields: usize) -> Result<FileEntry, GitError> {
    let (head, path) =
        split_after(record, fields).ok_or_else(|| malformed("a truncated record", record))?;
    let head = std::str::from_utf8(head)
        .map_err(|_| malformed("a record prefix that is not UTF-8", record))?;
    let codes = head
        .split(' ')
        .nth(1)
        .ok_or_else(|| malformed("a record with no XY field", record))?;
    let (index, worktree) = pair(codes).ok_or_else(|| malformed("an unknown XY pair", record))?;
    Ok(FileEntry {
        path: path_from(path),
        original_path: None,
        index,
        worktree,
    })
}

fn pair(codes: &str) -> Option<(Change, Change)> {
    let mut chars = codes.chars();
    let (x, y) = (chars.next()?, chars.next()?);
    if chars.next().is_some() {
        return None;
    }
    Some((Change::from_code(x)?, Change::from_code(y)?))
}

/// Splits a record just after its `count`-th space, giving the fixed prefix and
/// the raw path bytes that follow it.
///
/// The path is whatever is left, spaces included: it is the last field of every
/// record, so it needs no escaping and gets none.
fn split_after(record: &[u8], count: usize) -> Option<(&[u8], &[u8])> {
    let mut seen = 0;
    for (index, byte) in record.iter().enumerate() {
        if *byte != b' ' {
            continue;
        }
        seen += 1;
        if seen == count {
            return Some((&record[..index], &record[index + 1..]));
        }
    }
    None
}

/// A path exactly as git printed it.
///
/// On unix the bytes are the name, so they are used as one; elsewhere there is
/// no such guarantee and the lossy conversion is the honest answer.
fn path_from(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// `+3` / `-0`, as `# branch.ab` writes them.
fn signed(field: Option<&str>, sign: char) -> usize {
    field
        .and_then(|value| value.strip_prefix(sign))
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0)
}

/// Object names are shown at git's own default length.
fn short_oid(oid: &str) -> String {
    oid.chars().take(7).collect()
}

fn lossy(record: &[u8]) -> String {
    String::from_utf8_lossy(record).into_owned()
}

fn malformed(what: &str, record: &[u8]) -> GitError {
    GitError::Parse(format!("{what}: {:?}", lossy(record)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records are NUL-terminated in the real output; the tests write them with
    /// `\n` for legibility and this turns them into what git actually emits.
    fn zero(text: &str) -> Vec<u8> {
        text.lines()
            .flat_map(|line| {
                let mut bytes = line.as_bytes().to_vec();
                bytes.push(0);
                bytes
            })
            .collect()
    }

    #[test]
    fn the_branch_headers_name_the_head_and_its_upstream() {
        let raw = zero(
            "# branch.oid 5944bbedf5ce7e6cb6b2967c2f012e36e905cd57\n\
             # branch.head main\n\
             # branch.upstream origin/main\n\
             # branch.ab +2 -3",
        );
        let status = parse_status(&raw).unwrap();
        assert_eq!(status.head, Some(Head::Branch("main".into())));
        assert_eq!(status.head_label(), "main");
        assert_eq!(status.oid.as_deref(), Some("5944bbe"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 3));
        assert!(status.is_clean());
    }

    #[test]
    fn a_detached_head_is_not_a_branch_name() {
        let status = parse_status(&zero("# branch.head (detached)")).unwrap();
        assert_eq!(status.head, Some(Head::Detached));
        assert_eq!(status.head_label(), "detached");
    }

    #[test]
    fn a_repository_with_no_commits_has_no_object_name() {
        let status = parse_status(&zero("# branch.oid (initial)\n# branch.head main")).unwrap();
        assert_eq!(status.oid, None);
    }

    #[test]
    fn the_xy_pair_is_read_as_index_then_worktree() {
        let raw = zero(
            "1 D. N... 100644 000000 000000 7898192 0000000 a.txt\n\
             1 .M N... 100644 100644 100644 7898192 7898192 b.txt\n\
             1 MM N... 100644 100644 100644 7898192 7898192 c.txt",
        );
        let entries = parse_status(&raw).unwrap().entries;
        assert_eq!(entries[0].codes(), "D ");
        assert_eq!(entries[0].index, Change::Deleted);
        assert!(entries[0].index.is_change(), "the deletion is staged");
        assert_eq!(entries[1].codes(), " M");
        assert!(!entries[1].index.is_change(), "only the worktree changed");
        assert_eq!(entries[2].codes(), "MM");
        assert_eq!(entries[2].primary(), Change::Modified);
    }

    #[test]
    fn a_path_with_spaces_survives_because_it_is_the_last_field() {
        let raw = zero("1 .D N... 100644 100644 000000 6178079 6178079 b file.txt");
        let entries = parse_status(&raw).unwrap().entries;
        assert_eq!(entries[0].path, PathBuf::from("b file.txt"));
    }

    #[test]
    fn a_rename_reads_the_next_record_as_its_original_path() {
        let raw = zero(
            "2 R. N... 100644 100644 100644 b7b2b6b b7b2b6b R100 new name.txt\n\
             old.txt\n\
             ? after.txt",
        );
        let entries = parse_status(&raw).unwrap().entries;
        assert_eq!(
            entries.len(),
            2,
            "the original path is not an entry of its own"
        );
        assert_eq!(entries[0].path, PathBuf::from("new name.txt"));
        assert_eq!(entries[0].original_path, Some(PathBuf::from("old.txt")));
        assert_eq!(entries[0].index, Change::Renamed);
        assert_eq!(entries[1].path, PathBuf::from("after.txt"));
    }

    #[test]
    fn an_unmerged_record_is_a_conflict() {
        let raw = zero("u UU N... 100644 100644 100644 100644 df967b9 b19a1e9 950b81b c.txt");
        let status = parse_status(&raw).unwrap();
        let entry = &status.entries[0];
        assert_eq!(entry.codes(), "UU");
        assert!(entry.is_conflicted());
        assert_eq!(entry.primary(), Change::Unmerged);
        assert_eq!(status.conflicts(), 1);
    }

    #[test]
    fn untracked_takes_the_worktree_column_and_ignored_is_skipped() {
        let raw = zero("? new.rs\n! target/");
        let entries = parse_status(&raw).unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].codes(), " ?");
        assert_eq!(entries[0].worktree, Change::Untracked);
    }

    #[test]
    fn a_path_that_is_not_utf8_is_kept_as_it_was_printed() {
        let mut raw = b"? caf\xe9.txt".to_vec();
        raw.push(0);
        let entries = parse_status(&raw).unwrap().entries;
        assert_eq!(entries.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert_eq!(entries[0].path.as_os_str().as_bytes(), b"caf\xe9.txt");
        }
    }

    #[test]
    fn a_truncated_record_is_an_error_rather_than_a_guess() {
        assert!(matches!(
            parse_status(&zero("1 .M N...")),
            Err(GitError::Parse(_))
        ));
        assert!(matches!(
            parse_status(&zero(
                "1 XY N... 100644 100644 100644 7898192 7898192 a.txt"
            )),
            Err(GitError::Parse(_))
        ));
        assert!(matches!(
            parse_status(&zero(
                "2 R. N... 100644 100644 100644 b7b2b6b b7b2b6b R100 only.txt"
            )),
            Err(GitError::Parse(_)),
        ));
    }

    #[test]
    fn the_entry_list_stops_at_the_cap_and_says_so() {
        let mut raw = Vec::new();
        for index in 0..MAX_ENTRIES + 10 {
            raw.extend_from_slice(format!("? file{index}.txt").as_bytes());
            raw.push(0);
        }
        let status = parse_status(&raw).unwrap();
        assert_eq!(status.entries.len(), MAX_ENTRIES);
        assert!(status.truncated);
    }

    #[test]
    fn empty_output_is_a_clean_tree_and_not_an_error() {
        let status = parse_status(b"").unwrap();
        assert!(status.is_clean());
        assert_eq!(status.head, None);
    }
}
