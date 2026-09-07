//! Repository status, file entries, branches.
//!
//! The shapes here are what `parser.rs` produces and what `ui/git.rs` draws.
//! They know nothing about subprocesses or about ratatui, so the parser is
//! testable against recorded `git` output and the panel is testable against
//! hand-built status values.

use std::path::PathBuf;

use thiserror::Error;

/// One side of a porcelain-v2 `XY` pair.
///
/// The letters are git's own (`M`, `A`, `D`, `R`, `C`, `T`, `U`), with two
/// additions that are not letters in the pair itself: `Unmodified` is the `.`
/// git prints for "nothing happened on this side", and `Untracked` is the `?`
/// record, which has no pair at all and is folded in here so that a row on
/// screen is always two columns wide (SPEC §30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

impl Change {
    /// Reads one column of an `XY` pair. `None` is a code git does not define,
    /// which the parser treats as a malformed record rather than guessing.
    pub fn from_code(code: char) -> Option<Self> {
        Some(match code {
            '.' | ' ' => Self::Unmodified,
            'M' => Self::Modified,
            'A' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            'U' => Self::Unmerged,
            '?' => Self::Untracked,
            _ => return None,
        })
    }

    /// The character the panel shows. `Unmodified` is a blank rather than a
    /// dot: the column is there to be read at a glance, and `git status`
    /// itself leaves it empty.
    pub fn symbol(self) -> char {
        match self {
            Self::Unmodified => ' ',
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::TypeChanged => 'T',
            Self::Unmerged => 'U',
            Self::Untracked => '?',
        }
    }

    pub fn is_change(self) -> bool {
        self != Self::Unmodified
    }
}

/// One line of `git status`, as the two sides git reports it in.
///
/// `index` is what is staged and `worktree` is what is not — the same split as
/// the `XY` pair of `git status --short`. Phase 11 stages and unstages by
/// reading exactly these two fields, which is why the entry keeps both rather
/// than collapsing them into one "status" the way the Phase 1 mock did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Repository-relative, as git printed it.
    pub path: PathBuf,
    /// Where a renamed or copied entry came from.
    pub original_path: Option<PathBuf>,
    pub index: Change,
    pub worktree: Change,
}

impl FileEntry {
    /// The two-column `XY` field, index side first (SPEC §30).
    pub fn codes(&self) -> String {
        format!("{}{}", self.index.symbol(), self.worktree.symbol())
    }

    /// A file both sides changed, which git refuses to stage until it is
    /// resolved.
    pub fn is_conflicted(&self) -> bool {
        self.index == Change::Unmerged || self.worktree == Change::Unmerged
    }

    /// The change a row is coloured by: a conflict first, then whatever is not
    /// yet staged, and the staged side only when the worktree agrees with it.
    pub fn primary(&self) -> Change {
        if self.is_conflicted() {
            return Change::Unmerged;
        }
        if self.worktree.is_change() {
            return self.worktree;
        }
        self.index
    }
}

/// One entry of the branch picker (SPEC §33).
///
/// Remote-tracking branches are listed alongside local ones because checking
/// out a colleague's freshly pushed branch is the common reason to open a
/// picker at all. They are switched to by their short name — `git switch`
/// creates the local tracking branch for it — which is what `switch_target`
/// returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// As `git branch -a` prints it: `main`, or `origin/feature` for a
    /// remote-tracking one.
    pub name: String,
    /// The branch `HEAD` is on. Never true for a remote one.
    pub is_head: bool,
    pub remote: bool,
}

impl Branch {
    /// The name to hand `git switch`.
    ///
    /// `git switch origin/feature` would detach `HEAD`; `git switch feature`
    /// creates a local branch tracking it, which is what a person clicking
    /// `origin/feature` in a picker means.
    pub fn switch_target(&self) -> &str {
        if self.remote {
            // `origin/feature/x` -> `feature/x`: only the remote's own name is
            // dropped, and a branch name may contain slashes of its own.
            if let Some((_, rest)) = self.name.split_once('/') {
                return rest;
            }
        }
        &self.name
    }
}

/// What `# branch.head` said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    /// No branch: a checkout of a commit, a tag, or a rebase in progress.
    Detached,
}

/// The whole answer to one `git status --porcelain=v2 --branch`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// `None` until a status has been read; `Some(Head::Detached)` on a
    /// detached HEAD.
    pub head: Option<Head>,
    /// The short object name of `HEAD`, or `None` before the first commit.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub entries: Vec<FileEntry>,
    /// Whether `MAX_ENTRIES` cut the list short.
    pub truncated: bool,
    /// An operation git started and has not finished (SPEC §35).
    ///
    /// It is not the same as "there are conflicts": once every conflicted file
    /// has been staged the conflicts are gone and the operation is still
    /// waiting to be finished, which is the state a user is most likely to be
    /// lost in.
    pub operation: Option<Operation>,
}

/// Something git is in the middle of, recorded in the repository directory.
///
/// Phase 11 read only `MERGE_HEAD`, so a stopped rebase or cherry-pick left the
/// panel listing conflicted files under a title that said nothing about why
/// they were conflicted. All four are the same `exists()` on a path `discover`
/// already knows (ADR-041).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl Operation {
    /// What the panel title and the status bar call it, mid-sentence.
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "merging",
            Self::Rebase => "rebasing",
            Self::CherryPick => "cherry-picking",
            Self::Revert => "reverting",
        }
    }

    /// The noun for it, for a sentence that names it.
    pub fn noun(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }

    /// The `git` command that finishes it, for the sentence that says so.
    pub fn continue_command(self) -> &'static str {
        match self {
            Self::Merge => "git commit",
            Self::Rebase => "git rebase --continue",
            Self::CherryPick => "git cherry-pick --continue",
            Self::Revert => "git revert --continue",
        }
    }

    /// Whether the editor's own Commit finishes it.
    ///
    /// A merge is completed by a commit and the dialog is the way to write one.
    /// The other three are driven by `git <verb> --continue`, which reuses the
    /// recorded message and is not something an editor should imitate with a
    /// commit of its own.
    pub fn finished_by_commit(self) -> bool {
        self == Self::Merge
    }
}

impl RepoStatus {
    /// What the panel title and the status bar call the current head.
    pub fn head_label(&self) -> &str {
        match &self.head {
            Some(Head::Branch(name)) => name,
            Some(Head::Detached) => "detached",
            None => "no branch",
        }
    }

    pub fn is_clean(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn conflicts(&self) -> usize {
        self.entries.iter().filter(|e| e.is_conflicted()).count()
    }
}

/// Everything that can go wrong between the editor and the `git` binary.
///
/// `NotInstalled` and `NotARepository` are states rather than failures — the
/// panel says so and the editor carries on — which is why they are separate
/// variants instead of one string: `App` decides what to show from the variant
/// and never by matching on a message (SPEC §28, §45).
#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git is not installed")]
    NotInstalled,
    #[error("Not a Git repository")]
    NotARepository,
    #[error("git {command} failed: {message}")]
    Failed { command: String, message: String },
    #[error("git {command} did not finish in {seconds}s")]
    TimedOut { command: String, seconds: u64 },
    #[error("could not run git: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not read git output: {0}")]
    Parse(String),
    /// A merge that stopped on conflicts (SPEC §35). Not a failed command: git
    /// did what it was asked and the tree is now in a state the panel shows and
    /// the user has to finish.
    #[error("conflicts — resolve them in the panel, then commit")]
    Conflicted,
}
