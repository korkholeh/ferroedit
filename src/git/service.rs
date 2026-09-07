//! Subprocess wrapper around `git` with the safety env vars applied.
//!
//! The system binary, never a library (ADR-001): the user's credential helper,
//! SSH configuration and signing keys keep working because it is the same git
//! they run in a shell. Arguments are always passed as arguments — nothing here
//! builds a command line for a shell to re-split, so a file called
//! `; rm -rf ~` is a file name and not a surprise (SPEC §29).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::models::{GitError, RepoStatus};
use super::parser::parse_status;

/// How long any one git invocation may take before it is killed.
///
/// Status is local and finishes in milliseconds; the limit is here for the
/// invocation that never finishes — a lock held by another git, or a helper
/// waiting on a terminal that a TUI is not going to give it. A killed child is
/// an error the panel can show, and a wedged one is an editor that never draws
/// another frame (ARCHITECTURE §8).
const TIMEOUT: Duration = Duration::from_secs(10);

/// How often the child is checked while it runs. Small enough that a status
/// costs no visible time, large enough not to spin a core.
const POLL: Duration = Duration::from_millis(2);

/// A repository, and the way to ask git about it.
///
/// Holding the root rather than the workspace directory means every command
/// runs from the same place, so a status taken from a subdirectory lists the
/// same paths as one taken from the top.
#[derive(Debug, Clone)]
pub struct GitService {
    root: PathBuf,
}

impl GitService {
    /// Finds the repository `dir` is in (SPEC §28).
    ///
    /// Every failure of `rev-parse` is reported as "not a repository": that is
    /// what it means in all but pathological cases, the real message is
    /// logged, and a panel that said "permission denied" would be describing a
    /// state the user cannot act on from inside the editor anyway.
    pub fn discover(dir: &Path) -> Result<Self, GitError> {
        // A path that is not a directory would make the spawn itself fail with
        // `NotFound`, which is the same error a missing `git` produces — and
        // reporting "Git is not installed" because a directory was deleted
        // would be a lie.
        if !dir.is_dir() {
            return Err(GitError::NotARepository);
        }
        let output = match run(dir, &["rev-parse", "--show-toplevel"]) {
            Ok(output) => output,
            Err(GitError::Failed { message, .. }) => {
                log::debug!("{} is not a repository: {message}", dir.display());
                return Err(GitError::NotARepository);
            }
            Err(other) => return Err(other),
        };
        let root = String::from_utf8_lossy(&output).trim_end().to_string();
        if root.is_empty() {
            return Err(GitError::NotARepository);
        }
        log::info!("git repository at {root}");
        Ok(Self {
            root: PathBuf::from(root),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The changed files and the current branch, in one invocation (SPEC §30).
    ///
    /// `--untracked-files=normal` is passed rather than inherited: a user whose
    /// config turns untracked files off would otherwise get a panel that
    /// silently omits new files, and "the file I just created is not there" is
    /// not a state worth reproducing.
    pub fn status(&self) -> Result<RepoStatus, GitError> {
        let output = run(
            &self.root,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=normal",
                "-z",
            ],
        )?;
        parse_status(&output)
    }
}

/// Runs one git command and returns its standard output.
fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let started = Instant::now();
    let mut command = Command::new("git");
    command
        // A pager attached to a TUI's stdout would be a second program drawing
        // on the same screen.
        .arg("-c")
        .arg("core.pager=cat")
        .args(args)
        .current_dir(dir)
        // Without this a credential or passphrase prompt waits forever on a
        // terminal that belongs to the editor; with it, git fails and says so,
        // which is an error the user can act on (ARCHITECTURE §8).
        .env("GIT_TERMINAL_PROMPT", "0")
        // Reading status must not take the index lock: the user may well have a
        // git running in the terminal next door.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            GitError::NotInstalled
        } else {
            GitError::Io(err)
        }
    })?;

    let name = args.first().copied().unwrap_or("git").to_string();
    let (status, stdout, stderr) = wait_with_timeout(child, &name)?;
    log::debug!(
        "git {name} finished in {:?} ({} bytes)",
        started.elapsed(),
        stdout.len()
    );
    if !status.success() {
        return Err(GitError::Failed {
            command: name,
            message: first_line(&stderr),
        });
    }
    Ok(stdout)
}

/// Waits for the child, killing it if it outlives `TIMEOUT`.
///
/// Both pipes are drained on threads of their own rather than after the wait:
/// a status large enough to fill the pipe buffer would otherwise block the
/// child on a write while this function waits for it to exit.
fn wait_with_timeout(
    mut child: Child,
    name: &str,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), GitError> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_reader = thread::spawn(move || read_all(stdout));
    let err_reader = thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                log::error!("git {name} timed out; killing it");
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitError::TimedOut {
                    command: name.to_string(),
                    seconds: TIMEOUT.as_secs(),
                });
            }
            None => thread::sleep(POLL),
        }
    };

    // The pipes are closed by now, so neither join can block for long. A reader
    // that panicked costs its stream and nothing else.
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok((status, stdout, stderr))
}

fn read_all<R: Read>(stream: Option<R>) -> Vec<u8> {
    let mut buffer = Vec::new();
    if let Some(mut stream) = stream {
        if let Err(err) = stream.read_to_end(&mut buffer) {
            log::error!("could not read git output: {err}");
        }
    }
    buffer
}

/// The first line of git's complaint, which is the sentence worth showing on a
/// one-line status bar; the rest is hints and is logged instead.
fn first_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "no output".to_string();
    }
    log::error!("git said: {trimmed}");
    trimmed
        .lines()
        .next()
        .unwrap_or(trimmed)
        .trim_start_matches("fatal: ")
        .trim_start_matches("error: ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::models::{Change, Head};
    use crate::git::testing::TestRepo;

    #[test]
    fn a_directory_outside_a_repository_is_reported_as_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            GitService::discover(dir.path()),
            Err(GitError::NotARepository)
        ));
    }

    #[test]
    fn a_path_that_is_not_a_directory_is_not_a_missing_git() {
        assert!(matches!(
            GitService::discover(Path::new("/no/such/directory")),
            Err(GitError::NotARepository)
        ));
    }

    #[test]
    fn discovery_finds_the_root_from_a_subdirectory() {
        let repo = TestRepo::new();
        std::fs::create_dir(repo.path().join("src")).unwrap();
        let service = GitService::discover(&repo.path().join("src")).unwrap();
        assert_eq!(
            service.root(),
            std::fs::canonicalize(repo.path()).unwrap().as_path()
        );
    }

    #[test]
    fn a_fresh_repository_is_clean_and_on_its_default_branch() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let status = GitService::discover(repo.path()).unwrap().status().unwrap();
        assert_eq!(status.head, Some(Head::Branch("main".into())));
        assert!(status.is_clean(), "nothing changed since the commit");
        assert!(status.oid.is_some(), "there is a commit to name");
    }

    #[test]
    fn every_status_of_spec_30_reaches_the_panel() {
        let repo = TestRepo::new();
        repo.write("modified.txt", "one\n");
        repo.write("deleted.txt", "two\n");
        repo.write("renamed.txt", "a stable body that will not be rewritten\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        repo.write("modified.txt", "one\ntwo\n");
        std::fs::remove_file(repo.path().join("deleted.txt")).unwrap();
        repo.run(&["mv", "renamed.txt", "moved.txt"]);
        repo.write("added.txt", "new\n");
        repo.run(&["add", "added.txt"]);
        repo.write("untracked.txt", "loose\n");

        let status = GitService::discover(repo.path()).unwrap().status().unwrap();
        let find = |name: &str| {
            status
                .entries
                .iter()
                .find(|entry| entry.path.ends_with(name))
                .unwrap_or_else(|| panic!("{name} is missing from {:?}", status.entries))
        };
        assert_eq!(find("modified.txt").worktree, Change::Modified);
        assert_eq!(find("deleted.txt").worktree, Change::Deleted);
        assert_eq!(find("added.txt").index, Change::Added);
        assert_eq!(find("untracked.txt").worktree, Change::Untracked);
        let renamed = find("moved.txt");
        assert_eq!(renamed.index, Change::Renamed);
        assert_eq!(
            renamed.original_path.as_deref(),
            Some(Path::new("renamed.txt"))
        );
    }

    /// The Phase 10 acceptance, as a test: what the panel shows is what
    /// `git status` says, code for code.
    #[test]
    fn the_codes_match_what_git_status_prints_itself() {
        let repo = TestRepo::new();
        repo.write("staged.txt", "one\n");
        repo.write("both.txt", "two\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        repo.write("staged.txt", "changed\n");
        repo.run(&["add", "staged.txt"]);
        repo.write("both.txt", "changed\n");
        repo.run(&["add", "both.txt"]);
        repo.write("both.txt", "changed again\n");
        repo.write("untracked.txt", "loose\n");

        let ours = GitService::discover(repo.path()).unwrap().status().unwrap();
        let theirs = repo.short_status();
        let mine: Vec<String> = ours
            .entries
            .iter()
            .map(|entry| format!("{} {}", entry.codes(), entry.path.display()))
            .collect();
        assert_eq!(mine.len(), theirs.len(), "{mine:?} vs {theirs:?}");
        for line in &theirs {
            // `git status --short` writes untracked files as `??`; the panel
            // uses the porcelain-v2 form, where the index column of an
            // untracked file is blank because there is nothing in the index.
            let expected = line.replace("?? ", " ? ");
            assert!(mine.contains(&expected), "{expected:?} is not in {mine:?}");
        }
    }

    #[test]
    fn a_conflict_is_reported_as_unmerged() {
        let repo = TestRepo::new();
        repo.write("c.txt", "base\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["checkout", "-q", "-b", "other"]);
        repo.write("c.txt", "theirs\n");
        repo.commit("theirs");
        repo.run(&["checkout", "-q", "main"]);
        repo.write("c.txt", "ours\n");
        repo.commit("ours");
        // The merge fails on purpose, so its exit status is not checked.
        let _ = repo.try_run(&["merge", "other"]);

        let status = GitService::discover(repo.path()).unwrap().status().unwrap();
        assert_eq!(status.conflicts(), 1, "{:?}", status.entries);
        assert!(status.entries[0].is_conflicted());
    }

    #[test]
    fn a_branch_with_an_upstream_reports_how_far_it_has_diverged() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        // A second repository is not needed: a branch is a valid upstream, and
        // `branch.ab` counts commits either way.
        repo.run(&["branch", "upstream"]);
        repo.run(&["branch", "--set-upstream-to=upstream", "main"]);
        repo.write("a.txt", "b\n");
        repo.commit("ahead");

        let status = GitService::discover(repo.path()).unwrap().status().unwrap();
        assert_eq!(status.upstream.as_deref(), Some("upstream"));
        assert_eq!((status.ahead, status.behind), (1, 0));
    }

    #[test]
    fn a_failing_command_carries_gits_own_first_line() {
        let repo = TestRepo::new();
        let err = run(repo.path(), &["rev-parse", "--verify", "no-such-ref"]).unwrap_err();
        match err {
            GitError::Failed { command, message } => {
                assert_eq!(command, "rev-parse");
                assert!(!message.is_empty());
                assert!(!message.starts_with("fatal: "), "{message}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }
}
