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

use super::diff::{Diff, DiffSide};
use super::models::{Branch, GitError, Operation, RepoStatus};
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
    /// The repository's own directory, absolute. Held so that the states git
    /// records as files in it — `MERGE_HEAD` above all — can be read with a
    /// `stat` rather than with another subprocess on every status.
    git_dir: PathBuf,
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
        // Both paths in one invocation, both absolute: `--git-dir` on its own
        // is printed relative to the working directory, which is `dir` and not
        // necessarily the root.
        let output = match run(dir, &["rev-parse", "--show-toplevel", "--absolute-git-dir"]) {
            Ok(output) => output,
            Err(GitError::Failed { message, .. }) => {
                log::debug!("{} is not a repository: {message}", dir.display());
                return Err(GitError::NotARepository);
            }
            Err(other) => return Err(other),
        };
        let text = String::from_utf8_lossy(&output);
        let mut lines = text.lines().map(str::trim_end);
        let (Some(root), Some(git_dir)) = (lines.next(), lines.next()) else {
            return Err(GitError::NotARepository);
        };
        if root.is_empty() || git_dir.is_empty() {
            return Err(GitError::NotARepository);
        }
        log::info!("git repository at {root} (git dir {git_dir})");
        Ok(Self {
            root: PathBuf::from(root),
            git_dir: PathBuf::from(git_dir),
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
        let mut status = parse_status(&output)?;
        // A `stat`, not a subprocess: git records an unfinished operation as a
        // file in the repository directory, and the panel has to say so even
        // after every conflicted file has been staged (SPEC §35).
        status.operation = self.operation();
        Ok(status)
    }

    /// What git is in the middle of, if anything (ADR-041).
    ///
    /// Four `stat`s on paths under a directory `discover` already found. A
    /// rebase is recognised by its state *directory* rather than by
    /// `REBASE_HEAD`, which is what git's own status does: the directory is
    /// there for the whole rebase, and `REBASE_HEAD` only once one has stopped.
    /// Merge is checked first because it is the one the editor can finish.
    fn operation(&self) -> Option<Operation> {
        let exists = |name: &str| self.git_dir.join(name).exists();
        if exists("MERGE_HEAD") {
            return Some(Operation::Merge);
        }
        if exists("rebase-merge") || exists("rebase-apply") {
            return Some(Operation::Rebase);
        }
        if exists("CHERRY_PICK_HEAD") {
            return Some(Operation::CherryPick);
        }
        exists("REVERT_HEAD").then_some(Operation::Revert)
    }

    /// Every local branch, and every remote-tracking branch (SPEC §33).
    ///
    /// One `for-each-ref` rather than `git branch -a`: the format is ours, so
    /// nothing has to be recovered from a display form that changes with the
    /// terminal width and with the user's `color.branch`.
    pub fn branches(&self) -> Result<Vec<Branch>, GitError> {
        let output = run(
            &self.root,
            &[
                "for-each-ref",
                "--format=%(HEAD)%00%(refname:short)%00%(refname)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;
        let text = String::from_utf8_lossy(&output);
        let mut branches = Vec::new();
        for line in text.lines() {
            let mut fields = line.split('\0');
            let (Some(head), Some(name), Some(refname)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(GitError::Parse(format!("branch record {line:?}")));
            };
            // `refs/remotes/origin/HEAD` is a symbolic ref to the remote's
            // default branch, not a branch of its own; listing it would offer
            // the same branch twice under two names.
            if refname.ends_with("/HEAD") {
                continue;
            }
            branches.push(Branch {
                name: name.to_string(),
                is_head: head == "*",
                remote: refname.starts_with("refs/remotes/"),
            });
        }
        log::debug!("{} branches", branches.len());
        Ok(branches)
    }

    /// `git switch <name>` (SPEC §33).
    ///
    /// `switch` and not `checkout`: it only ever moves `HEAD`, so a branch name
    /// that is also a path cannot be read as a request to discard that file's
    /// changes.
    pub fn switch_to(&self, branch: &str) -> Result<String, GitError> {
        run(&self.root, &["switch", branch])?;
        Ok(String::new())
    }

    /// Creates a branch at `HEAD` and switches to it — what "New Branch" means
    /// when it is reached from a picker of branches to be on.
    pub fn create_branch(&self, name: &str) -> Result<String, GitError> {
        run(&self.root, &["switch", "-c", name])?;
        Ok(String::new())
    }

    /// `git merge <branch>` (SPEC §35).
    ///
    /// A merge that stops on conflicts is reported as `Conflicted` and not as a
    /// failed command: git wrote its complaint to *stdout*, exited non-zero,
    /// and left the tree in exactly the state the panel is there to show. The
    /// status is what decides which of the two happened, because it is the same
    /// question the panel will ask a moment later anyway.
    pub fn merge(&self, branch: &str) -> Result<String, GitError> {
        match run(&self.root, &["merge", "--no-edit", branch]) {
            Ok(output) => Ok(first_line_of(&output)),
            Err(GitError::Failed { command, message }) => {
                if self.status().is_ok_and(|status| status.conflicts() > 0) {
                    return Err(GitError::Conflicted);
                }
                Err(GitError::Failed { command, message })
            }
            Err(other) => Err(other),
        }
    }

    /// The unified diff of one file, worktree or staged (SPEC §36).
    ///
    /// `--no-color` because the viewer colours the lines itself from what they
    /// are, and a user with `color.ui = always` would otherwise get escape
    /// sequences drawn as text. `--no-ext-diff` because `diff.external` is
    /// somebody else's program writing somebody else's format, and the parser
    /// here reads git's. Both are passed rather than inherited, for the reason
    /// `--untracked-files` is: a configuration file must not change what the
    /// editor shows.
    ///
    /// The path goes in as an `OsString` pathspec after `--`, so a file called
    /// `-x` is a file and not an option (ADR-032).
    pub fn diff(&self, path: &Path, side: DiffSide) -> Result<Diff, GitError> {
        let mut args: Vec<OsString> = ["diff", "--no-color", "--no-ext-diff"]
            .iter()
            .map(OsString::from)
            .collect();
        if side == DiffSide::Staged {
            args.push(OsString::from("--cached"));
        }
        args.push(OsString::from("--"));
        args.push(path.as_os_str().to_os_string());
        let output = run_os(&self.root, &args, TIMEOUT)?;
        // Lossy on purpose: a diff is text to look at, and one line of a file
        // in another encoding must not cost the user the rest of the hunk.
        Ok(Diff::parse(&String::from_utf8_lossy(&output)))
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

    /// `git pull --no-edit` (SPEC §34).
    ///
    /// No `--ff-only` any more and no `--rebase` either way: ADR-034 refused a
    /// merging pull only until there was somewhere for a conflict to be shown,
    /// and `merge` above is now that somewhere (ADR-036). Neither reconciliation
    /// strategy is forced, so `pull.rebase` decides — and a divergence with no
    /// configuration is git's own clear complaint about exactly that, which is
    /// better advice than anything this editor could substitute for it.
    pub fn pull(&self) -> Result<String, GitError> {
        match run_with(&self.root, &["pull", "--no-edit"], NETWORK_TIMEOUT) {
            Ok(output) => Ok(first_line_of(&output)),
            Err(GitError::Failed { command, message }) => {
                if self.status().is_ok_and(|status| status.conflicts() > 0) {
                    return Err(GitError::Conflicted);
                }
                Err(GitError::Failed { command, message })
            }
            Err(other) => Err(other),
        }
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

/// The one line of git's complaint worth showing on a one-line status bar; the
/// rest is progress and hints, and is logged instead.
///
/// It is *not* simply the first line. A failed `git pull` reports the fetch it
/// managed first (`From /srv/repo`), then a dozen `hint:` lines, and only then
/// says what actually went wrong — so a line git marked as the failure wins
/// over position, and position is only the fallback for the commands that mark
/// nothing.
fn first_line(stderr: &[u8]) -> String {
    const MARKERS: [&str; 2] = ["fatal: ", "error: "];
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "no output".to_string();
    }
    log::error!("git said: {trimmed}");
    let marked = trimmed
        .lines()
        .find(|line| MARKERS.iter().any(|marker| line.starts_with(marker)));
    let line = marked.or_else(|| trimmed.lines().next()).unwrap_or(trimmed);
    MARKERS
        .iter()
        .fold(line, |line, marker| line.trim_start_matches(marker))
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
    use crate::git::models::{Branch, Change, Head};
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

    /// A repository with a `main` and an `other` whose changes conflict.
    fn conflicting_repo() -> TestRepo {
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
        repo
    }

    #[test]
    fn the_branch_list_names_every_branch_and_marks_the_one_head_is_on() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["branch", "topic"]);

        let branches = GitService::discover(repo.path())
            .unwrap()
            .branches()
            .unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "topic"]);
        assert!(branches[0].is_head, "{branches:?}");
        assert!(!branches[0].remote);
        assert!(!branches[1].is_head);
    }

    /// A remote branch is worth listing — checking out a colleague's is the
    /// common reason to open a picker — and `origin/HEAD` is not, because it is
    /// a symbolic ref to a branch already in the list.
    #[test]
    fn remote_branches_are_listed_and_the_remotes_head_symref_is_not() {
        let (repo, _remote) = repo_with_remote();
        repo.run(&["remote", "set-head", "origin", "main"]);

        let branches = GitService::discover(repo.path())
            .unwrap()
            .branches()
            .unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "origin/main"]);
        assert!(branches[1].remote);
        assert!(!branches[1].is_head, "a remote branch is never HEAD");
        // `git switch origin/main` would detach HEAD; `git switch main` is what
        // clicking that row has to run.
        assert_eq!(branches[1].switch_target(), "main");
    }

    #[test]
    fn a_remote_branch_keeps_the_slashes_in_its_own_name() {
        let branch = Branch {
            name: "origin/feature/nested".into(),
            is_head: false,
            remote: true,
        };
        assert_eq!(branch.switch_target(), "feature/nested");
    }

    #[test]
    fn switching_moves_head_and_creating_makes_a_branch_at_it() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        let service = GitService::discover(repo.path()).unwrap();

        service.create_branch("topic").unwrap();
        assert_eq!(
            service.status().unwrap().head,
            Some(Head::Branch("topic".into()))
        );
        service.switch_to("main").unwrap();
        assert_eq!(
            service.status().unwrap().head,
            Some(Head::Branch("main".into()))
        );
    }

    #[test]
    fn switching_to_a_branch_that_does_not_exist_says_so() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let err = GitService::discover(repo.path())
            .unwrap()
            .switch_to("nope")
            .unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
    }

    #[test]
    fn a_clean_merge_reports_what_git_said_about_it() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.run(&["checkout", "-q", "-b", "other"]);
        repo.write("b.txt", "b\n");
        repo.run(&["add", "."]);
        repo.commit("other");
        repo.run(&["checkout", "-q", "main"]);

        let service = GitService::discover(repo.path()).unwrap();
        let said = service.merge("other").unwrap();
        assert!(!said.is_empty(), "git says what it did");
        assert!(repo.path().join("b.txt").exists());
        assert!(service.status().unwrap().is_clean());
    }

    /// SPEC §35: a merge that conflicts is not a failed command. git wrote its
    /// complaint to stdout, exited non-zero, and left a tree the panel shows.
    #[test]
    fn a_merge_that_conflicts_is_reported_as_conflicts_and_not_as_a_failure() {
        let repo = conflicting_repo();
        let service = GitService::discover(repo.path()).unwrap();

        let err = service.merge("other").unwrap_err();
        assert!(matches!(err, GitError::Conflicted), "{err:?}");

        let status = service.status().unwrap();
        assert_eq!(status.conflicts(), 1, "{:?}", status.entries);
        assert_eq!(
            status.operation,
            Some(Operation::Merge),
            "MERGE_HEAD is what says the merge is unfinished"
        );
    }

    /// The half of `operation` that `conflicts()` cannot answer: once every
    /// conflicted file is staged there are no conflicts left and the merge is
    /// still waiting for its commit.
    #[test]
    fn a_resolved_merge_is_still_a_merge_until_it_is_committed() {
        let repo = conflicting_repo();
        let service = GitService::discover(repo.path()).unwrap();
        let _ = service.merge("other");

        repo.write("c.txt", "resolved\n");
        service.stage(&[PathBuf::from("c.txt")]).unwrap();
        let status = service.status().unwrap();
        assert_eq!(status.conflicts(), 0);
        assert_eq!(status.operation, Some(Operation::Merge), "{status:?}");

        service.commit("merged").unwrap();
        assert_eq!(service.status().unwrap().operation, None);
    }

    /// Phase 11 read only `MERGE_HEAD`, so these two left the panel listing
    /// conflicted files with nothing above them saying why (ADR-041).
    #[test]
    fn a_stopped_rebase_and_a_stopped_cherry_pick_are_named() {
        let repo = conflicting_repo();
        let service = GitService::discover(repo.path()).unwrap();

        // `main` and `other` changed the same line, so replaying either onto
        // the other stops.
        let rebase = repo.try_run(&["rebase", "other"]);
        assert!(!rebase.status.success(), "the rebase was meant to stop");
        let status = service.status().unwrap();
        assert_eq!(status.operation, Some(Operation::Rebase), "{status:?}");
        assert_eq!(status.conflicts(), 1);
        repo.run(&["rebase", "--abort"]);
        assert_eq!(service.status().unwrap().operation, None);

        let pick = repo.try_run(&["cherry-pick", "other"]);
        assert!(!pick.status.success(), "the cherry-pick was meant to stop");
        assert_eq!(
            service.status().unwrap().operation,
            Some(Operation::CherryPick)
        );
        repo.run(&["cherry-pick", "--abort"]);
        assert_eq!(service.status().unwrap().operation, None);
    }

    #[test]
    fn merging_a_branch_that_does_not_exist_is_an_ordinary_failure() {
        let repo = TestRepo::new();
        repo.write("a.txt", "a\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let err = GitService::discover(repo.path())
            .unwrap()
            .merge("nope")
            .unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
    }

    /// ADR-036: `--ff-only` is gone, so a pull that has to merge does.
    ///
    /// `pull.rebase` is written into the repository's own config rather than
    /// left to the machine's: the point of not passing `--no-rebase` is that
    /// the user's configuration decides, so the test has to be a user with one.
    #[test]
    fn a_pull_that_has_to_merge_merges() {
        let (repo, remote) = repo_with_remote();
        repo.run(&["config", "pull.rebase", "false"]);
        // A second clone pushes a commit the first one has not seen.
        let other = tempfile::tempdir().unwrap();
        let clone = TestRepo::at(other.path());
        clone.run(&["clone", "-q", remote.path().to_str().unwrap(), "."]);
        clone.write("theirs.txt", "theirs\n");
        clone.run(&["add", "."]);
        clone.commit("theirs");
        clone.run(&["push", "-q"]);

        // And the first one has a commit of its own, so the pull cannot
        // fast-forward.
        repo.write("ours.txt", "ours\n");
        repo.run(&["add", "."]);
        repo.commit("ours");

        let service = GitService::discover(repo.path()).unwrap();
        service.pull().unwrap();
        assert!(
            repo.path().join("theirs.txt").exists(),
            "their commit arrived"
        );
        assert!(service.status().unwrap().is_clean());
    }

    /// A failed `git pull` writes the fetch it managed, then a dozen hints, and
    /// only then the sentence that says what went wrong.
    #[test]
    fn the_line_git_marked_as_the_failure_beats_the_first_one() {
        let stderr = b"From /srv/repo\n   abc..def  main -> origin/main\n\
                       hint: You have divergent branches.\n\
                       fatal: Need to specify how to reconcile divergent branches.\n";
        assert_eq!(
            first_line(stderr),
            "Need to specify how to reconcile divergent branches."
        );
        // And position is still the fallback for the commands that mark nothing.
        assert_eq!(
            first_line(b"something went wrong\nand then more\n"),
            "something went wrong"
        );
        assert_eq!(first_line(b"   \n"), "no output");
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

    /// SPEC §36: the two sides answer different questions, and the service has
    /// to be able to ask each of them.
    #[test]
    fn the_worktree_and_the_staged_diff_of_a_file_are_different_answers() {
        let repo = TestRepo::new();
        repo.write("a.txt", "one\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.write("a.txt", "two\n");
        repo.run(&["add", "a.txt"]);
        repo.write("a.txt", "three\n");

        let service = GitService::discover(repo.path()).unwrap();
        let path = Path::new("a.txt");

        let worktree = service.diff(path, DiffSide::Worktree).unwrap();
        let staged = service.diff(path, DiffSide::Staged).unwrap();
        let text = |diff: &Diff| {
            diff.lines
                .iter()
                .map(|line| line.text.clone())
                .collect::<Vec<_>>()
        };
        assert!(text(&worktree).contains(&"+three".to_string()));
        assert!(text(&worktree).contains(&"-two".to_string()));
        assert!(text(&staged).contains(&"+two".to_string()));
        assert!(text(&staged).contains(&"-one".to_string()));
        assert_eq!((worktree.added, worktree.removed), (1, 1));
    }

    #[test]
    fn a_file_with_nothing_to_show_diffs_as_nothing() {
        let repo = TestRepo::new();
        repo.write("a.txt", "one\n");
        repo.run(&["add", "."]);
        repo.commit("init");

        let service = GitService::discover(repo.path()).unwrap();
        assert!(service
            .diff(Path::new("a.txt"), DiffSide::Worktree)
            .unwrap()
            .is_empty());
    }

    /// A pathspec that begins with a dash is a file name, not an option
    /// (ADR-032) — and `--` in front of it is what makes that true.
    #[test]
    fn a_file_whose_name_looks_like_an_option_is_still_a_file() {
        let repo = TestRepo::new();
        repo.write("-x.txt", "one\n");
        repo.run(&["add", "--", "-x.txt"]);
        repo.commit("init");
        repo.write("-x.txt", "two\n");

        let service = GitService::discover(repo.path()).unwrap();
        let diff = service
            .diff(Path::new("-x.txt"), DiffSide::Worktree)
            .unwrap();
        assert_eq!((diff.added, diff.removed), (1, 1));
    }

    /// The diff runs from the repository root, so a path git printed in a
    /// status is the path that can be handed straight back to it.
    #[test]
    fn a_path_in_a_subdirectory_is_relative_to_the_root() {
        let repo = TestRepo::new();
        repo.write("src/a.txt", "one\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.write("src/a.txt", "two\n");

        let service = GitService::discover(&repo.path().join("src")).unwrap();
        let diff = service
            .diff(Path::new("src/a.txt"), DiffSide::Worktree)
            .unwrap();
        assert_eq!((diff.added, diff.removed), (1, 1));
    }
}
