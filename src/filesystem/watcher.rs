//! Filesystem watcher: the answer to "something changed outside the editor".
//!
//! Until now the explorer, the git panel and the diff viewer all learned about
//! a `git checkout` in another terminal on the next save, file operation or
//! `F5`. This thread closes that gap (ADR-040): `notify` reports the change, a
//! filter decides whether it is worth a frame, and a coalescing window turns a
//! burst into one `AppEvent::FilesChanged`.
//!
//! Two things make it affordable. The filter drops everything git already
//! ignores and everything under `.git` except the handful of paths that decide
//! what the panel shows, so a `cargo build` writing ten thousand files into
//! `target/` costs nothing. The window then bounds what is left: a quiet period
//! before anything is sent, and a ceiling so a long-running writer still gets a
//! refresh rather than starving behind its own noise.
//!
//! Like the input thread this one is detached and owns its watcher. It ends
//! when the channel to the main loop closes, which it notices on the next
//! event; on the way out of the process that is never, and the thread dies
//! with everything else.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Event, RecursiveMode, Result as NotifyResult, Watcher};

use crate::event::AppEvent;

/// How long the filesystem has to be quiet before a burst is reported.
///
/// Long enough that one `git checkout` is one refresh, short enough that a
/// change made in another window is on screen before the user has looked back.
const QUIET: Duration = Duration::from_millis(250);

/// The longest a burst is held before it is reported anyway.
///
/// A writer that never goes quiet — a watch build, a long `git clone` — would
/// otherwise keep resetting the window and the panel would say nothing at all
/// until it finished.
const MAX_DELAY: Duration = Duration::from_secs(2);

/// The files under `.git` whose contents decide what the git panel shows.
///
/// Everything else in there is git's own bookkeeping: loose objects by the
/// thousand, `index.lock` held for a millisecond, log files nobody is reading.
/// Matching on the first component covers `refs/heads/topic` with one entry.
const GIT_PATHS: &[&str] = &[
    "HEAD",
    "index",
    "packed-refs",
    "refs",
    "MERGE_HEAD",
    "MERGE_MSG",
    "ORIG_HEAD",
    "REBASE_HEAD",
    "CHERRY_PICK_HEAD",
];

/// What a burst of filesystem events turned out to be about.
///
/// A change in the worktree is also a change to `git status`, so `worktree`
/// implies `repository`; the pair exists for the other direction, where staging
/// a file in another terminal must not cost a rebuild of the explorer's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FsChange {
    pub worktree: bool,
    pub repository: bool,
}

impl FsChange {
    fn worktree() -> Self {
        Self {
            worktree: true,
            repository: true,
        }
    }

    fn repository() -> Self {
        Self {
            worktree: false,
            repository: true,
        }
    }

    fn merge(&mut self, other: Self) {
        self.worktree |= other.worktree;
        self.repository |= other.repository;
    }
}

/// Starts watching `root`, reporting into `tx`.
///
/// An error here is not fatal and must not be: inotify watches are a per-user
/// resource on Linux and a large tree can exhaust them. The editor runs without
/// a watcher exactly as it did before this existed, and `F5` is still there.
pub fn spawn(root: &Path, tx: Sender<AppEvent>) -> notify::Result<()> {
    let (fs_tx, fs_rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        // A send failure means the loop below has gone; there is nothing to do
        // about it here, and the watcher is dropped with it.
        let _ = fs_tx.send(event);
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;

    let filter = Filter::new(root);
    thread::Builder::new()
        .name("watcher".into())
        .spawn(move || {
            // Moved in so it lives as long as the loop that reads it: dropping
            // a `notify` watcher stops the watch.
            let _watcher = watcher;
            watch_loop(&fs_rx, &tx, &filter);
        })
        .expect("failed to spawn the watcher thread");
    Ok(())
}

fn watch_loop(rx: &Receiver<NotifyResult<Event>>, tx: &Sender<AppEvent>, filter: &Filter) {
    loop {
        // Block until something worth reporting happens. Everything filtered
        // out here costs one comparison and no frame.
        let mut pending = loop {
            match rx.recv() {
                Ok(Ok(event)) => match filter.classify(&event) {
                    Some(change) => break change,
                    None => continue,
                },
                // `notify` reports its own failures — a watch that could not be
                // added to a new subdirectory, a dropped event queue — through
                // the same channel. Neither is worth a notification, and both
                // are worth a log line.
                Ok(Err(err)) => log::warn!("watcher: {err}"),
                Err(_) => return,
            }
        };

        if !coalesce(rx, filter, &mut pending) {
            return;
        }
        log::debug!("filesystem changed: {pending:?}");
        if tx.send(AppEvent::FilesChanged(pending)).is_err() {
            return;
        }
    }
}

/// Waits out the burst `pending` started, merging what arrives into it.
///
/// Returns false when the watcher is gone, which is the loop's only exit that
/// is not the main loop's.
///
/// Only an event that survives the filter extends the quiet window. Without
/// that, a build writing to an ignored directory would hold the window open
/// with events nobody wants and the refresh would be paced by `MAX_DELAY`
/// rather than by the change that earned it.
fn coalesce(rx: &Receiver<NotifyResult<Event>>, filter: &Filter, pending: &mut FsChange) -> bool {
    let started = Instant::now();
    let mut last = started;
    loop {
        let quiet_left = QUIET.saturating_sub(last.elapsed());
        let total_left = MAX_DELAY.saturating_sub(started.elapsed());
        if quiet_left.is_zero() || total_left.is_zero() {
            return true;
        }
        match rx.recv_timeout(quiet_left.min(total_left)) {
            Ok(Ok(event)) => {
                if let Some(change) = filter.classify(&event) {
                    pending.merge(change);
                    last = Instant::now();
                }
            }
            Ok(Err(err)) => log::warn!("watcher: {err}"),
            // The deadline that expired is re-derived at the top of the loop.
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return true,
        }
    }
}

/// Decides which paths are worth waking the editor for.
struct Filter {
    /// `root/.git`, whether or not it exists — a workspace that is not a
    /// repository simply never matches it.
    git_dir: PathBuf,
    ignore: Gitignore,
}

impl Filter {
    fn new(root: &Path) -> Self {
        Self {
            git_dir: root.join(".git"),
            ignore: build_ignore(root),
        }
    }

    /// The change one event describes, or `None` when every path in it was
    /// filtered out.
    fn classify(&self, event: &Event) -> Option<FsChange> {
        let mut change = FsChange::default();
        for path in &event.paths {
            if let Some(one) = self.classify_path(path) {
                change.merge(one);
            }
        }
        (change != FsChange::default()).then_some(change)
    }

    fn classify_path(&self, path: &Path) -> Option<FsChange> {
        if let Ok(tail) = path.strip_prefix(&self.git_dir) {
            return self.classify_git_path(tail);
        }
        // `.git` as a *file* is a worktree or a submodule pointing elsewhere;
        // its content changing is a repository change like any other.
        if path == self.git_dir {
            return Some(FsChange::repository());
        }
        // What git ignores, the panel and the tree do not show, so a change to
        // it changes nothing on screen.
        if self
            .ignore
            .matched_path_or_any_parents(path, path.is_dir())
            .is_ignore()
        {
            return None;
        }
        Some(FsChange::worktree())
    }

    fn classify_git_path(&self, tail: &Path) -> Option<FsChange> {
        let first = tail.components().next()?.as_os_str().to_str()?;
        // git writes `index.lock` and `HEAD.lock` next to the real thing and
        // renames over it; the rename is the event that matters, and reporting
        // the lock as well would double every refresh.
        let name = first.strip_suffix(".lock").unwrap_or(first);
        GIT_PATHS.contains(&name).then(FsChange::repository)
    }
}

/// The root's ignore rules, best effort.
///
/// Only the root `.gitignore` and `.git/info/exclude`: `ignore` matches a path
/// against a set of patterns, and honouring a `.gitignore` in every
/// subdirectory would mean walking for them. The cost of missing one is an
/// extra refresh, not a wrong screen — and the pattern that matters, a build
/// directory at the root, is the one this catches.
fn build_ignore(root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    for file in [root.join(".gitignore"), root.join(".git/info/exclude")] {
        if let Some(err) = builder.add(&file) {
            log::debug!("watcher ignore rules: {} not read: {err}", file.display());
        }
    }
    builder.build().unwrap_or_else(|err| {
        log::warn!("watcher ignore rules could not be built: {err}");
        Gitignore::empty()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, EventKind};
    use std::sync::mpsc;

    fn root() -> PathBuf {
        PathBuf::from("/tmp/ws")
    }

    fn filter() -> Filter {
        Filter {
            git_dir: root().join(".git"),
            ignore: Gitignore::empty(),
        }
    }

    fn event(paths: &[&str]) -> Event {
        Event {
            kind: EventKind::Create(CreateKind::File),
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    #[test]
    fn a_source_file_is_a_worktree_change_and_so_a_status_change() {
        assert_eq!(
            filter().classify(&event(&["/tmp/ws/src/main.rs"])),
            Some(FsChange {
                worktree: true,
                repository: true
            })
        );
    }

    /// Staging in another terminal must not cost a rebuild of the tree's rows.
    #[test]
    fn the_index_is_a_repository_change_only() {
        assert_eq!(
            filter().classify(&event(&["/tmp/ws/.git/index"])),
            Some(FsChange {
                worktree: false,
                repository: true
            })
        );
    }

    #[test]
    fn a_branch_switch_is_seen_through_head_and_refs() {
        for path in ["/tmp/ws/.git/HEAD", "/tmp/ws/.git/refs/heads/topic"] {
            assert_eq!(
                filter().classify(&event(&[path])),
                Some(FsChange::repository()),
                "{path}"
            );
        }
    }

    /// The whole reason the filter exists: git's own churn is most of what a
    /// recursive watch on a repository reports.
    #[test]
    fn gits_bookkeeping_is_dropped() {
        for path in [
            "/tmp/ws/.git/objects/ab/cdef",
            "/tmp/ws/.git/logs/HEAD",
            "/tmp/ws/.git/COMMIT_EDITMSG",
            "/tmp/ws/.git/hooks/pre-commit",
        ] {
            assert_eq!(filter().classify(&event(&[path])), None, "{path}");
        }
    }

    #[test]
    fn a_lock_file_reports_as_the_thing_it_locks() {
        assert_eq!(
            filter().classify(&event(&["/tmp/ws/.git/index.lock"])),
            Some(FsChange::repository())
        );
    }

    #[test]
    fn an_ignored_path_is_not_worth_a_frame() {
        let mut builder = GitignoreBuilder::new(root());
        builder.add_line(None, "target/").unwrap();
        let filter = Filter {
            git_dir: root().join(".git"),
            ignore: builder.build().unwrap(),
        };
        assert_eq!(
            filter.classify(&event(&["/tmp/ws/target/debug/ferroedit"])),
            None
        );
        assert!(filter.classify(&event(&["/tmp/ws/src/main.rs"])).is_some());
    }

    /// A rename arrives as one event with both paths on it.
    #[test]
    fn an_event_is_the_union_of_its_paths() {
        assert_eq!(
            filter().classify(&event(&["/tmp/ws/.git/index", "/tmp/ws/src/main.rs"])),
            Some(FsChange {
                worktree: true,
                repository: true
            })
        );
        assert_eq!(
            filter().classify(&event(&["/tmp/ws/.git/objects/ab/cd"])),
            None
        );
    }

    /// A burst is one report, and it does not wait for the writer to stop
    /// forever.
    #[test]
    fn a_burst_coalesces_into_one_change() {
        let (tx, rx) = mpsc::channel();
        for _ in 0..50 {
            tx.send(Ok(event(&["/tmp/ws/src/main.rs"]))).unwrap();
        }
        tx.send(Ok(event(&["/tmp/ws/.git/index"]))).unwrap();
        drop(tx);

        let mut pending = FsChange::worktree();
        assert!(coalesce(&rx, &filter(), &mut pending));
        assert_eq!(
            pending,
            FsChange {
                worktree: true,
                repository: true
            }
        );
        assert!(rx.try_recv().is_err(), "the burst was drained");
    }

    /// The real thing, end to end: a file written on disk reaches the main
    /// loop's channel as one event.
    #[test]
    fn a_write_on_disk_reaches_the_main_loop() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        spawn(dir.path(), tx).expect("a watcher over a temp directory");

        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let event = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the write was reported");
        match event {
            AppEvent::FilesChanged(change) => assert!(change.worktree && change.repository),
            other => panic!("expected a filesystem change, got {other:?}"),
        }
    }
}
