//! Background thread running long git jobs, replying with `JobId` results.
//!
//! ADR-030 put the status read on the UI thread and said the worker would
//! arrive with the operations that genuinely cannot block a frame. These are
//! those operations: `pull` and `push` are network-bound and `commit` runs the
//! user's hooks, so none of them has an upper bound a frame can wait for
//! (SPEC §34, §37).
//!
//! The shape is the one SPEC §37 asks for and nothing more: one `std::thread`,
//! one channel in, and the main loop's own `AppEvent` channel out. There is no
//! executor and no runtime — a handful of subprocesses is not a reason to take
//! on Tokio.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::models::GitError;
use super::GitService;
use crate::event::AppEvent;

/// Names one submitted job, so an answer can be matched to the request that
/// produced it even when several are outstanding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub u64);

/// What the worker can be asked to do (SPEC §31, §32, §34).
///
/// Staging is here with the network operations even though it is fast: it takes
/// the index lock, and a lock another git is holding is exactly the case that
/// turns a millisecond into ten seconds. One road to git for every command that
/// writes is also one place to reason about ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitJob {
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    StageAll,
    UnstageAll,
    Commit(String),
    Pull,
    Push,
    /// Moves `HEAD` to an existing branch (SPEC §33).
    Switch(String),
    /// Creates a branch at `HEAD` and switches to it.
    CreateBranch(String),
    /// Merges a branch into the current one (SPEC §35).
    Merge(String),
}

impl GitJob {
    /// What the panel title and the opening notification say while it runs —
    /// `Pushing…`, the line SPEC §34 asks for.
    pub fn progress(&self) -> &'static str {
        match self {
            Self::Stage(_) => "Staging…",
            Self::Unstage(_) => "Unstaging…",
            Self::StageAll => "Staging everything…",
            Self::UnstageAll => "Unstaging everything…",
            Self::Commit(_) => "Committing…",
            Self::Pull => "Pulling…",
            Self::Push => "Pushing…",
            Self::Switch(_) => "Switching…",
            Self::CreateBranch(_) => "Creating the branch…",
            Self::Merge(_) => "Merging…",
        }
    }

    /// What the status bar says when it worked and git itself said nothing.
    pub fn done(&self) -> &'static str {
        match self {
            Self::Stage(_) => "Staged",
            Self::Unstage(_) => "Unstaged",
            Self::StageAll => "Staged every change",
            Self::UnstageAll => "Unstaged every change",
            Self::Commit(_) => "Committed",
            Self::Pull => "Pulled",
            Self::Push => "Pushed",
            Self::Switch(_) => "Switched",
            Self::CreateBranch(_) => "Created the branch",
            Self::Merge(_) => "Merged",
        }
    }

    /// The noun a failure is reported under: `Push failed: …`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stage(_) => "Stage",
            Self::Unstage(_) => "Unstage",
            Self::StageAll => "Stage all",
            Self::UnstageAll => "Unstage all",
            Self::Commit(_) => "Commit",
            Self::Pull => "Pull",
            Self::Push => "Push",
            Self::Switch(_) => "Switch",
            Self::CreateBranch(_) => "New branch",
            Self::Merge(_) => "Merge",
        }
    }
}

/// A finished job, on its way back to the main loop.
///
/// The error is already a string. `App` only ever shows it, and the outcome
/// travels as a `Command` so that the worker feeds the same single mutation
/// path as every other producer (ARCHITECTURE invariant 3) — which requires
/// `Clone` and `Eq`, and `GitError` is neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutcome {
    pub id: JobId,
    pub job: GitJob,
    pub result: Result<String, String>,
}

/// The handle the editor keeps: a queue into the thread, and the counter that
/// names what goes into it.
#[derive(Debug)]
pub struct GitWorker {
    jobs: Sender<Task>,
    next_id: u64,
}

type Task = (JobId, GitJob, GitService);

impl GitWorker {
    /// Starts the thread. `events` is the main loop's own channel, so a
    /// finished job wakes it the same way a key press does — no polling and no
    /// timeout on the receive.
    pub fn spawn(events: Sender<AppEvent>) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("git".into())
            .spawn(move || worker_loop(&rx, &events))
            .expect("failed to spawn the git worker thread");
        Self {
            jobs: tx,
            next_id: 0,
        }
    }

    /// Queues a job, or reports that the thread is gone.
    ///
    /// The service travels with the job rather than living on the thread: the
    /// repository can be rediscovered under the editor (`git init` in the
    /// terminal next door), and a worker holding a stale root would keep
    /// answering about the old one.
    pub fn submit(&mut self, job: GitJob, service: GitService) -> Option<JobId> {
        let id = JobId(self.next_id);
        self.next_id += 1;
        match self.jobs.send((id, job, service)) {
            Ok(()) => Some(id),
            Err(err) => {
                log::error!("the git worker is gone: {err}");
                None
            }
        }
    }
}

/// One job at a time, in the order they were submitted.
///
/// Serial rather than parallel on purpose: these commands take the index lock,
/// and two of them racing for it would produce a failure the user did not cause
/// — a commit queued behind the staging it depends on has to see that staging
/// finish.
fn worker_loop(jobs: &Receiver<Task>, events: &Sender<AppEvent>) {
    while let Ok((id, job, service)) = jobs.recv() {
        log::info!("git job {id:?}: {}", job.label());
        let result = run_job(&service, &job).map_err(reason);
        let outcome = JobOutcome { id, job, result };
        if events.send(AppEvent::GitJob(outcome)).is_err() {
            // The editor has shut down; there is nobody left to answer.
            break;
        }
    }
    log::debug!("the git worker is stopping");
}

/// The half of a failure worth showing under the job's own name.
///
/// `App` reports a failure as `Push failed: …`, so git's own `git push failed:`
/// prefix would say it twice — and it would say `git reset failed` for what the
/// user asked for as an unstage. The variant's *reason* is the half that is
/// about what went wrong; the half about which subprocess ran is already in the
/// log.
fn reason(err: GitError) -> String {
    match err {
        GitError::Failed { message, .. } => message,
        GitError::TimedOut { seconds, .. } => format!("it did not finish in {seconds}s"),
        other => other.to_string(),
    }
}

fn run_job(service: &GitService, job: &GitJob) -> Result<String, GitError> {
    let said = match job {
        GitJob::Stage(paths) => service.stage(paths)?,
        GitJob::Unstage(paths) => service.unstage(paths)?,
        GitJob::StageAll => service.stage_all()?,
        GitJob::UnstageAll => service.unstage_all()?,
        GitJob::Commit(message) => service.commit(message)?,
        GitJob::Pull => service.pull()?,
        GitJob::Push => service.push()?,
        GitJob::Switch(branch) => service.switch_to(branch)?,
        GitJob::CreateBranch(name) => service.create_branch(name)?,
        GitJob::Merge(branch) => service.merge(branch)?,
    };
    if !said.is_empty() {
        // git's own sentence when it wrote one — `[main 5944bbe] message`,
        // `Already up to date.`
        return Ok(said);
    }
    // Ours when it did not, naming the branch: `Switched` alone leaves the one
    // question the user has — to what? — unanswered.
    Ok(match job {
        GitJob::Switch(branch) | GitJob::CreateBranch(branch) | GitJob::Merge(branch) => {
            format!("{} {branch}", job.done())
        }
        _ => job.done().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testing::TestRepo;

    /// Drains one outcome, failing rather than hanging if the worker never
    /// answers.
    fn outcome(rx: &Receiver<AppEvent>) -> JobOutcome {
        match rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the git worker answers")
        {
            AppEvent::GitJob(outcome) => outcome,
            other => panic!("expected a git outcome, got {other:?}"),
        }
    }

    #[test]
    fn a_job_runs_off_the_calling_thread_and_answers_on_the_event_channel() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        let service = GitService::discover(repo.path()).unwrap();

        let (tx, rx) = mpsc::channel();
        let mut worker = GitWorker::spawn(tx);
        let id = worker.submit(GitJob::StageAll, service).unwrap();

        let answer = outcome(&rx);
        assert_eq!(answer.id, id);
        assert_eq!(answer.job, GitJob::StageAll);
        assert_eq!(answer.result, Ok("Staged every change".into()));
        assert_eq!(repo.short_status(), vec!["A  a.txt"]);
    }

    #[test]
    fn jobs_are_answered_in_the_order_they_were_submitted() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        let service = GitService::discover(repo.path()).unwrap();

        let (tx, rx) = mpsc::channel();
        let mut worker = GitWorker::spawn(tx);
        let first = worker.submit(GitJob::StageAll, service.clone()).unwrap();
        let second = worker.submit(GitJob::UnstageAll, service).unwrap();

        assert_eq!(outcome(&rx).id, first);
        assert_eq!(outcome(&rx).id, second);
        // The second undid the first, which is only true if they ran in order.
        assert_eq!(repo.short_status(), vec!["?? a.txt"]);
    }

    #[test]
    fn a_failure_comes_back_as_gits_own_first_line() {
        let repo = TestRepo::new();
        let service = GitService::discover(repo.path()).unwrap();

        let (tx, rx) = mpsc::channel();
        let mut worker = GitWorker::spawn(tx);
        worker.submit(GitJob::Push, service).unwrap();

        let answer = outcome(&rx);
        let message = answer.result.unwrap_err();
        assert!(message.contains("destination"), "{message}");
        // The reason only: `App` puts `Push failed: ` in front of it, and
        // git's own `git push failed:` would then be there twice.
        assert!(!message.contains("failed"), "{message}");
    }

    #[test]
    fn every_job_has_a_progress_line_a_completion_and_a_noun() {
        for job in [
            GitJob::Stage(vec![PathBuf::from("a")]),
            GitJob::Unstage(vec![PathBuf::from("a")]),
            GitJob::StageAll,
            GitJob::UnstageAll,
            GitJob::Commit("m".into()),
            GitJob::Pull,
            GitJob::Push,
            GitJob::Switch("main".into()),
            GitJob::CreateBranch("topic".into()),
            GitJob::Merge("other".into()),
        ] {
            assert!(job.progress().ends_with('…'), "{job:?}");
            assert!(!job.done().is_empty(), "{job:?}");
            assert!(!job.label().is_empty(), "{job:?}");
        }
    }
}
