//! Subprocess wrapper around `git` with the safety env vars applied.
//!
//! The system binary, never a library (ADR-001): the user's credential helper,
//! SSH configuration and signing keys keep working because it is the same git
//! they run in a shell. Arguments are always passed as arguments — nothing here
//! builds a command line for a shell to re-split, so a file called
//! `; rm -rf ~` is a file name and not a surprise (SPEC §29).

use std::ffi::OsString;
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

/// The same for the three commands that talk to a remote (SPEC §37).
///
/// A fetch over a slow link is not a wedged process, and killing an honest
/// push after ten seconds would be worse than the pause it was meant to
/// prevent — the pause is not on the UI thread at all, because these run on
/// the worker (ADR-033). Two minutes is past any transfer a person waits for
/// at a keyboard and still short of a run that will never end.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

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

    /// `git add -- <paths>` (SPEC §31).
    ///
    /// One invocation for the whole list, and the paths go in as `OsStr`: they
    /// are the bytes git printed in the status, so a name that is not UTF-8
    /// goes back exactly as it came (ADR-032).
    pub fn stage(&self, paths: &[PathBuf]) -> Result<String, GitError> {
        self.with_paths(&["add", "--"], paths)
    }

    /// `git reset -q -- <paths>`.
    ///
    /// `git restore --staged` is the modern spelling and the one SPEC §31
    /// sketches, but it resolves `HEAD` and therefore fails outright in a
    /// repository that has no commits yet — which is precisely the repository
    /// where a file staged by mistake is most likely. `reset` does the same
    /// job and does it on an unborn branch too.
    pub fn unstage(&self, paths: &[PathBuf]) -> Result<String, GitError> {
        self.with_paths(&["reset", "-q", "--"], paths)
    }

    /// Everything the status lists, in one go — new files included.
    pub fn stage_all(&self) -> Result<String, GitError> {
        run(&self.root, &["add", "-A"])?;
        Ok(String::new())
    }

    pub fn unstage_all(&self) -> Result<String, GitError> {
        run(&self.root, &["reset", "-q", "--"])?;
        Ok(String::new())
    }

    /// Commits what is staged, with the message the dialog collected.
    ///
    /// Nothing about the user's configuration is overridden: their hooks, their
    /// signing key and their identity are the ones a commit from a shell would
    /// use (SPEC §32). A signing key that wants a passphrase from a terminal is
    /// the case `GIT_TERMINAL_PROMPT=0` turns into an error message instead of
    /// a hang.
    pub fn commit(&self, message: &str) -> Result<String, GitError> {
        let output = run(&self.root, &["commit", "-m", message])?;
        Ok(first_line_of(&output))
    }

    /// `git pull --ff-only` (SPEC §34).
    ///
    /// Fast-forward only, deliberately: a pull that merges can conflict, and a
    /// conflict needs the resolution UI that Phase 12 owns. Refusing with
    /// git's own "Not possible to fast-forward" is a state the user can act on;
    /// a surprise merge commit made by an editor is not (ADR-034).
    pub fn pull(&self) -> Result<String, GitError> {
        let output = run_with(&self.root, &["pull", "--ff-only"], NETWORK_TIMEOUT)?;
        Ok(first_line_of(&output))
    }

    /// `git push`, with whatever `push.default` and the branch's upstream say.
    pub fn push(&self) -> Result<String, GitError> {
        let output = run_with(&self.root, &["push"], NETWORK_TIMEOUT)?;
        Ok(first_line_of(&output))
    }

    /// A fixed head followed by a pathspec list. An empty list runs nothing:
    /// `git add --` with no paths is `git add` with no paths, which is not what
    /// any caller here means.
    fn with_paths(&self, head: &[&str], paths: &[PathBuf]) -> Result<String, GitError> {
        if paths.is_empty() {
            return Ok(String::new());
        }
        let mut args: Vec<OsString> = head.iter().map(OsString::from).collect();
        args.extend(paths.iter().map(|path| path.as_os_str().to_os_string()));
        run_os(&self.root, &args, TIMEOUT)?;
        Ok(String::new())
    }
}

/// Runs one git command and returns its standard output.
fn run(dir: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    run_with(dir, args, TIMEOUT)
}

fn run_with(dir: &Path, args: &[&str], timeout: Duration) -> Result<Vec<u8>, GitError> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    run_os(dir, &args, timeout)
}

/// The one place a git subprocess is spawned.
///
/// Arguments are `OsString` rather than `&str` because a pathspec is a path,
/// and a path is bytes: transcoding one through `String` on the way to `git
/// add` would stage a different file than the status listed (ADR-032).
fn run_os(dir: &Path, args: &[OsString], timeout: Duration) -> Result<Vec<u8>, GitError> {
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
        // A commit or a merge that wants a message opens `core.editor`, which
        // would be a second full-screen program on the terminal the TUI is
        // drawing to. `true` exits 0 with an empty file, so git falls back to
        // whatever message it was already given — and the commands here always
        // give it one.
        .env("GIT_EDITOR", "true")
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

    let name = args.first().map_or_else(
        || "git".to_string(),
        |arg| arg.to_string_lossy().into_owned(),
    );
    let (status, stdout, stderr) = wait_with_timeout(child, &name, timeout)?;
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
    timeout: Duration,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), GitError> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_reader = thread::spawn(move || read_all(stdout));
    let err_reader = thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                log::error!("git {name} timed out; killing it");
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitError::TimedOut {
                    command: name.to_string(),
                    seconds: timeout.as_secs(),
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

/// The first non-empty line git wrote to standard output, which is the
/// sentence worth putting on the status bar: `[main 5944bbe] message` for a
/// commit, `Already up to date.` for a pull. An empty answer is normal — a
/// plain push says nothing on stdout — and the caller supplies the wording for
/// that case.
fn first_line_of(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
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

    /// A helper the action tests share: a repository with one commit and a
    /// bare remote it is set up to push to. No network is involved — a path is
    /// a perfectly good git remote, and it exercises the same code.
    fn repo_with_remote() -> (TestRepo, tempfile::TempDir) {
        let repo = TestRepo::new();
        let remote = tempfile::tempdir().unwrap();
        repo.run(&[
            "-c",
            "init.defaultBranch=main",
            "init",
            "-q",
            "--bare",
            remote.path().to_str().unwrap(),
        ]);
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
        repo.run(&["push", "-q", "-u", "origin", "main"]);
        (repo, remote)
    }

    #[test]
    fn staging_and_unstaging_move_a_file_between_the_two_columns() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.write("a.txt", "b\n");
        let service = GitService::discover(repo.path()).unwrap();

        service.stage(&[PathBuf::from("a.txt")]).unwrap();
        assert_eq!(repo.short_status(), vec!["M  a.txt"]);
        service.unstage(&[PathBuf::from("a.txt")]).unwrap();
        assert_eq!(repo.short_status(), vec![" M a.txt"]);
    }

    /// SPEC §31's four operations, and the one that has to work before there is
    /// a `HEAD` to reset to.
    #[test]
    fn stage_all_and_unstage_all_work_before_the_first_commit() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.write("b.txt", "b\n");
        let service = GitService::discover(repo.path()).unwrap();

        service.stage_all().unwrap();
        assert_eq!(repo.short_status(), vec!["A  a.txt", "A  b.txt"]);
        service.unstage_all().unwrap();
        assert_eq!(repo.short_status(), vec!["?? a.txt", "?? b.txt"]);
    }

    #[test]
    fn a_deletion_is_staged_like_any_other_change() {
        let repo = TestRepo::new();
        repo.write("gone.txt", "x\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        std::fs::remove_file(repo.path().join("gone.txt")).unwrap();

        GitService::discover(repo.path())
            .unwrap()
            .stage(&[PathBuf::from("gone.txt")])
            .unwrap();
        assert_eq!(repo.short_status(), vec!["D  gone.txt"]);
    }

    /// ADR-032's promise, exercised end to end: a name that is not UTF-8 is
    /// staged as the bytes git printed, not as a lossy transcription of them.
    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_staged_by_its_bytes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let repo = TestRepo::new();
        let name = PathBuf::from(OsStr::from_bytes(b"caf\xe9.txt"));
        // APFS and HFS+ reject a name that is not valid UTF-8 at the syscall,
        // so on macOS there is no such file to stage and nothing to check. The
        // ADR is about the platforms where one can exist.
        if std::fs::write(repo.path().join(&name), "x\n").is_err() {
            return;
        }

        let service = GitService::discover(repo.path()).unwrap();
        let untracked = service.status().unwrap().entries[0].path.clone();
        assert_eq!(untracked, name);
        service.stage(&[untracked]).unwrap();

        let staged = service.status().unwrap();
        assert_eq!(staged.entries.len(), 1);
        assert_eq!(staged.entries[0].index, Change::Added);
    }

    #[test]
    fn a_pathspec_list_that_is_empty_runs_nothing() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        let service = GitService::discover(repo.path()).unwrap();
        service.stage(&[]).unwrap();
        assert_eq!(repo.short_status(), vec!["?? a.txt"], "still untracked");
    }

    #[test]
    fn a_commit_reports_the_line_git_printed_for_it() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);

        let said = GitService::discover(repo.path())
            .unwrap()
            .commit("first")
            .unwrap();
        assert!(said.contains("first"), "{said:?}");
        assert!(repo.short_status().is_empty(), "nothing left to commit");
    }

    #[test]
    fn committing_with_nothing_staged_fails_with_gits_own_words() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let err = GitService::discover(repo.path())
            .unwrap()
            .commit("empty")
            .unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
    }

    /// The Phase 11 acceptance, minus the thread: a real push against a real
    /// remote, and the ahead count it clears.
    #[test]
    fn a_push_sends_the_local_commits_and_clears_the_ahead_count() {
        let (repo, _remote) = repo_with_remote();
        repo.write("a.txt", "b\n");
        repo.commit("second");

        let service = GitService::discover(repo.path()).unwrap();
        assert_eq!(service.status().unwrap().ahead, 1);
        service.push().unwrap();
        assert_eq!(service.status().unwrap().ahead, 0);
    }

    #[test]
    fn a_pull_with_nothing_to_fetch_says_so() {
        let (repo, _remote) = repo_with_remote();
        let said = GitService::discover(repo.path()).unwrap().pull().unwrap();
        assert!(said.contains("up to date"), "{said:?}");
    }

    /// `GIT_TERMINAL_PROMPT=0` turns the question git would otherwise ask into
    /// a sentence the status bar can show — the Phase 11 acceptance for
    /// failures.
    #[test]
    fn a_push_with_no_remote_fails_with_an_actionable_message() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let err = GitService::discover(repo.path())
            .unwrap()
            .push()
            .unwrap_err();
        match err {
            GitError::Failed { command, message } => {
                assert_eq!(command, "push");
                assert!(message.contains("destination"), "{message}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
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
