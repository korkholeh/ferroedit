//! The git panel's state: the repository, its status, and the selection in it.
//!
//! Reading a status is a subprocess, so it happens when something has changed
//! — a save, a file operation, an explicit refresh — and never once a frame.
//! `App` owns one of these; `ui/git.rs` reads it and nothing else writes it.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::Sender;

use crate::event::AppEvent;
use crate::git::models::{FileEntry, GitError, RepoStatus};
use crate::git::{GitJob, GitService, GitWorker, JobId, JobOutcome};

/// What the panel has to say for itself.
///
/// The three failure states are kept apart because the panel says different
/// things about them and because only one of them is worth retrying: a
/// directory that is not a repository stays that way until the user makes one,
/// while a command that failed may well work on the next refresh (SPEC §28).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GitAvailability {
    /// Nothing has been looked at yet — the state before the first scan.
    #[default]
    Unknown,
    Repository,
    NotARepository,
    /// git is missing, or a command failed. The string is what the panel shows.
    Unavailable(String),
}

#[derive(Debug, Default)]
pub struct GitState {
    repo: Option<GitService>,
    /// The background thread the write operations run on (ADR-033). `None`
    /// until the main loop attaches one, which is also the state every headless
    /// test starts in — a `GitState` with no worker refuses jobs instead of
    /// spawning a thread nobody asked for.
    worker: Option<GitWorker>,
    /// Jobs submitted and not yet answered, oldest first. A queue rather than a
    /// single slot because the worker runs them in order: staging three files
    /// in three keystrokes must not be three refusals.
    running: VecDeque<(JobId, GitJob)>,
    pub availability: GitAvailability,
    pub status: RepoStatus,
    pub selected: usize,
    pub scroll: usize,
}

impl GitState {
    /// Looks for the repository the workspace is in and reads its status.
    ///
    /// Run at startup and on an explicit refresh. Everything that merely
    /// changes files calls `refresh` instead: `git rev-parse` on every save
    /// would be a subprocess spent re-answering a question whose answer is
    /// already known.
    pub fn discover(&mut self, root: &Path) {
        match GitService::discover(root) {
            Ok(service) => {
                log::info!("git status will run in {}", service.root().display());
                self.repo = Some(service);
                self.availability = GitAvailability::Repository;
                self.refresh();
            }
            Err(GitError::NotARepository) => self.set_unavailable(GitAvailability::NotARepository),
            Err(err) => {
                log::warn!("git is unavailable: {err}");
                self.set_unavailable(GitAvailability::Unavailable(err.to_string()));
            }
        }
    }

    /// Re-reads the status of a repository already found. Does nothing at all
    /// outside a repository, which is what makes it safe to call after every
    /// save and every file operation.
    pub fn refresh(&mut self) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match repo.status() {
            Ok(status) => {
                self.status = status;
                self.availability = GitAvailability::Repository;
                self.clamp();
            }
            Err(err) => {
                log::error!("git status failed: {err}");
                self.set_unavailable(GitAvailability::Unavailable(err.to_string()));
            }
        }
    }

    /// The worker is deliberately left alone: it is a thread, not repository
    /// state, and a job already in flight still has an answer to deliver.
    fn set_unavailable(&mut self, availability: GitAvailability) {
        self.repo = None;
        self.status = RepoStatus::default();
        self.availability = availability;
        self.clamp();
    }

    /// Gives the panel a worker thread to run its write operations on.
    ///
    /// Called once, from the run loop, with the loop's own event channel — the
    /// finished job has to arrive where the key presses do (ADR-033).
    pub fn attach_worker(&mut self, events: Sender<AppEvent>) {
        self.worker = Some(GitWorker::spawn(events));
    }

    /// Queues a job, returning the line to show while it runs.
    ///
    /// The `Err` is a sentence for the status bar, not a failure to handle:
    /// there are exactly two, and both mean "this cannot be started", not
    /// "this went wrong".
    pub fn start(&mut self, job: GitJob) -> Result<&'static str, String> {
        let Some(repo) = self.repo.clone() else {
            return Err(self.summary());
        };
        let Some(worker) = self.worker.as_mut() else {
            return Err("Git operations need the worker thread".to_string());
        };
        let progress = job.progress();
        match worker.submit(job.clone(), repo) {
            Some(id) => {
                self.running.push_back((id, job));
                Ok(progress)
            }
            None => Err("The git worker has stopped".to_string()),
        }
    }

    /// Takes a finished job off the queue.
    ///
    /// An id that is not there is not an error: the repository can be
    /// rediscovered while a job is in flight, and the answer to a job from
    /// before that is still worth showing even though nothing is waiting for it.
    pub fn finish(&mut self, outcome: &JobOutcome) {
        self.running.retain(|(id, _)| *id != outcome.id);
    }

    /// What the panel title says while something is running — the oldest job,
    /// because that is the one actually in git's hands.
    pub fn busy(&self) -> Option<&'static str> {
        self.running.front().map(|(_, job)| job.progress())
    }

    /// The repository root, for turning a status path into one to open.
    pub fn root(&self) -> Option<&Path> {
        self.repo.as_ref().map(GitService::root)
    }

    /// The entry the panel's selection is on.
    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.status.entries.get(self.selected)
    }

    /// How many files have something staged — what a commit would include, and
    /// what decides whether asking for a message is worth doing at all.
    pub fn staged_count(&self) -> usize {
        self.status
            .entries
            .iter()
            .filter(|entry| !entry.is_conflicted() && entry.index.is_change())
            .count()
    }

    pub fn is_repository(&self) -> bool {
        self.availability == GitAvailability::Repository
    }

    pub fn entries(&self) -> &[FileEntry] {
        &self.status.entries
    }

    /// The branch as the status bar names it (SPEC §38).
    pub fn branch_label(&self) -> &str {
        if self.is_repository() {
            self.status.head_label()
        } else {
            "no repository"
        }
    }

    /// The line an explicit refresh puts on the status bar.
    pub fn summary(&self) -> String {
        match &self.availability {
            GitAvailability::Unknown => "Git has not been read yet".to_string(),
            GitAvailability::NotARepository => "Not a Git repository".to_string(),
            GitAvailability::Unavailable(why) => why.clone(),
            GitAvailability::Repository => {
                let head = self.status.head_label();
                let conflicts = match self.status.conflicts() {
                    0 => String::new(),
                    1 => ", 1 conflict".to_string(),
                    count => format!(", {count} conflicts"),
                };
                match self.status.entries.len() {
                    0 => format!("{head} — working tree clean"),
                    1 => format!("{head} — 1 change{conflicts}"),
                    count => format!("{head} — {count} changes{conflicts}"),
                }
            }
        }
    }

    /// Keeps the selection on an entry and the entry on screen.
    ///
    /// The same rule the explorer has, and here for the same reason: a refresh
    /// can shorten the list under a selection that was valid a moment ago.
    pub fn follow_selection(&mut self, height: usize) {
        let len = self.status.entries.len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(len - 1);
        if height == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        self.scroll = self.scroll.min(len.saturating_sub(height));
    }

    /// Clamps after the list itself changed, without a window to scroll into:
    /// the next `follow_selection` has the height and finishes the job.
    fn clamp(&mut self) {
        let len = self.status.entries.len();
        self.selected = self.selected.min(len.saturating_sub(1));
        self.scroll = self.scroll.min(len.saturating_sub(1));
    }
}

/// Test fixture: a repository with one file in each of the states the panel
/// draws differently, built in memory so that no test of the *rendering* needs
/// a repository on disk. The tests of the reading itself use a real one.
#[cfg(test)]
impl GitState {
    pub fn fixture() -> Self {
        use crate::git::models::{Change, Head};
        use std::path::PathBuf;

        let entry = |path: &str, index: Change, worktree: Change| FileEntry {
            path: PathBuf::from(path),
            original_path: None,
            index,
            worktree,
        };
        Self {
            repo: None,
            worker: None,
            running: VecDeque::new(),
            availability: GitAvailability::Repository,
            status: RepoStatus {
                head: Some(Head::Branch("main".into())),
                oid: Some("5944bbe".into()),
                entries: vec![
                    entry("src/main.rs", Change::Unmodified, Change::Modified),
                    entry("src/ui/theme.rs", Change::Added, Change::Unmodified),
                    entry("src/old.rs", Change::Unmodified, Change::Deleted),
                    entry("src/new.rs", Change::Unmodified, Change::Untracked),
                ],
                ..RepoStatus::default()
            },
            selected: 0,
            scroll: 0,
        }
    }

    /// Puts a job on the queue with no worker behind it, so the in-progress
    /// title can be drawn and asserted without starting a thread.
    pub fn pretend_running(&mut self, job: GitJob) {
        self.running.push_back((JobId(0), job));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::models::{Change, Head};
    use crate::git::testing::TestRepo;
    use std::path::PathBuf;

    fn entry(path: &str, index: Change, worktree: Change) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            original_path: None,
            index,
            worktree,
        }
    }

    #[test]
    fn a_directory_outside_a_repository_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut git = GitState::default();
        git.discover(dir.path());
        assert_eq!(git.availability, GitAvailability::NotARepository);
        assert_eq!(git.summary(), "Not a Git repository");
        assert_eq!(git.branch_label(), "no repository");
    }

    #[test]
    fn discovery_reads_the_branch_and_the_changes_in_one_go() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.write("a.txt", "b\n");

        let mut git = GitState::default();
        git.discover(repo.path());
        assert!(git.is_repository());
        assert_eq!(git.status.head, Some(Head::Branch("main".into())));
        assert_eq!(git.entries().len(), 1);
        assert_eq!(git.summary(), "main — 1 change");
    }

    #[test]
    fn a_refresh_outside_a_repository_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut git = GitState::default();
        git.discover(dir.path());
        git.refresh();
        assert_eq!(git.availability, GitAvailability::NotARepository);
    }

    #[test]
    fn a_refresh_picks_up_a_file_written_since_the_last_one() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let mut git = GitState::default();
        git.discover(repo.path());
        assert!(git.entries().is_empty());

        repo.write("new.txt", "new\n");
        git.refresh();
        assert_eq!(git.entries().len(), 1);
        assert_eq!(git.entries()[0].worktree, Change::Untracked);
    }

    #[test]
    fn a_shorter_list_cannot_leave_the_selection_past_its_end() {
        let mut git = GitState {
            status: RepoStatus {
                entries: vec![
                    entry("a", Change::Unmodified, Change::Modified),
                    entry("b", Change::Unmodified, Change::Modified),
                    entry("c", Change::Unmodified, Change::Modified),
                ],
                ..RepoStatus::default()
            },
            selected: 2,
            scroll: 2,
            ..GitState::default()
        };
        git.status.entries.truncate(1);
        git.clamp();
        assert_eq!(git.selected, 0);
        assert_eq!(git.scroll, 0);
    }

    #[test]
    fn the_view_follows_the_selection_down_a_list_taller_than_the_panel() {
        let mut git = GitState {
            status: RepoStatus {
                entries: (0..10)
                    .map(|i| entry(&format!("f{i}"), Change::Unmodified, Change::Modified))
                    .collect(),
                ..RepoStatus::default()
            },
            ..GitState::default()
        };
        git.selected = 7;
        git.follow_selection(4);
        assert_eq!(git.scroll, 4, "the selected row is the last one shown");
        git.selected = 1;
        git.follow_selection(4);
        assert_eq!(git.scroll, 1);
    }

    #[test]
    fn a_job_cannot_be_started_outside_a_repository_or_without_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        let mut git = GitState::default();
        git.discover(dir.path());
        assert_eq!(
            git.start(GitJob::StageAll),
            Err("Not a Git repository".to_string())
        );

        let repo = TestRepo::new();
        let mut git = GitState::default();
        git.discover(repo.path());
        assert_eq!(
            git.start(GitJob::StageAll),
            Err("Git operations need the worker thread".to_string()),
            "a panel with no worker refuses rather than spawning one"
        );
        assert_eq!(git.busy(), None);
    }

    #[test]
    fn the_oldest_running_job_is_the_one_the_title_names() {
        let mut git = GitState::default();
        assert_eq!(git.busy(), None);
        git.pretend_running(GitJob::Push);
        git.pretend_running(GitJob::Pull);
        assert_eq!(git.busy(), Some("Pushing…"), "the one git actually has");
    }

    #[test]
    fn finishing_a_job_that_is_not_on_the_queue_is_not_an_error() {
        let mut git = GitState::default();
        git.pretend_running(GitJob::Push);
        // A rediscovery can drop the queue under an outcome already in flight.
        git.finish(&JobOutcome {
            id: JobId(99),
            job: GitJob::Pull,
            result: Ok("Pulled".into()),
        });
        assert_eq!(git.busy(), Some("Pushing…"));
        git.finish(&JobOutcome {
            id: JobId(0),
            job: GitJob::Push,
            result: Ok("Pushed".into()),
        });
        assert_eq!(git.busy(), None);
    }

    #[test]
    fn only_the_staged_side_of_a_row_counts_towards_a_commit() {
        let mut git = GitState::fixture();
        // The fixture holds one added file, one modified, one deleted and one
        // untracked; only the added one has anything in the index.
        assert_eq!(git.staged_count(), 1);
        git.status
            .entries
            .push(entry("conflict.rs", Change::Unmerged, Change::Unmerged));
        assert_eq!(git.staged_count(), 1, "a conflict is not staged work");
    }

    #[test]
    fn an_empty_list_has_nothing_selected_and_nothing_scrolled() {
        let mut git = GitState::default();
        git.follow_selection(5);
        assert_eq!((git.selected, git.scroll), (0, 0));
        assert!(git.entries().is_empty());
    }
}
