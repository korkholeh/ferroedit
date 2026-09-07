//! A real repository in a temporary directory, for the tests that need one.
//!
//! Everything git integration does goes through the real binary (ADR-001), so
//! the tests do too: a fake that answered like git would only ever be as
//! correct as our idea of git. The repositories built here are isolated from
//! the machine's configuration — no global config, no system config, no
//! signing — so a developer with `commit.gpgsign = true` gets the same results
//! as CI.
//!
//! That isolation has to be written into the *repository* and not only into
//! the environment these helpers pass, because `GitService` is the other thing
//! running git here and it is production code: it passes the environment the
//! editor passes, and reads whatever config the machine has (ADR-048). An
//! identity in `.git/config` is what a repository a user works in already has,
//! and it is the one place both callers look.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct TestRepo {
    dir: Handle,
}

/// A repository this handle owns the directory of, or one it only points at.
#[derive(Debug)]
enum Handle {
    Owned(TempDir),
    Borrowed(std::path::PathBuf),
}

impl Handle {
    fn path(&self) -> &Path {
        match self {
            Self::Owned(dir) => dir.path(),
            Self::Borrowed(path) => path,
        }
    }
}

impl TestRepo {
    /// A repository with no commits, on `main`.
    pub fn new() -> Self {
        let repo = Self {
            dir: Handle::Owned(tempfile::tempdir().expect("a temporary directory")),
        };
        repo.run(&["-c", "init.defaultBranch=main", "init", "-q"]);
        repo.identify();
        repo
    }

    /// An empty handle over a directory that is about to become a repository —
    /// a clone into it, which `new`'s `git init` would get in the way of. The
    /// `TempDir` stays the caller's, so it outlives this.
    ///
    /// Use `clone_from` rather than running the clone by hand: a repository
    /// with no identity in it is the thing this module exists to prevent.
    pub fn at(dir: &Path) -> Self {
        Self {
            dir: Handle::Borrowed(dir.to_path_buf()),
        }
    }

    /// Clones `remote` into this handle's directory and gives it an identity.
    pub fn clone_from(&self, remote: &Path) {
        self.run(&[
            "clone",
            "-q",
            remote.to_str().expect("a temporary path is UTF-8"),
            ".",
        ]);
        self.identify();
    }

    /// Writes the identity and the signing setting into `.git/config`.
    ///
    /// The environment variables `try_run` passes cover the commands *these*
    /// helpers run and nothing else. `GitService` is the other caller and it
    /// is the code under test: it passes the editor's own environment, so on a
    /// machine with no `user.email` configured anywhere — a CI runner, which
    /// is exactly where this was found — every merge, pull and commit it makes
    /// fails with `empty ident name`. Repository config is what both of them
    /// read, and it beats a global `commit.gpgsign = true` as well (ADR-048).
    fn identify(&self) {
        self.run(&["config", "user.name", "FerroEdit Test"]);
        self.run(&["config", "user.email", "test@example.invalid"]);
        self.run(&["config", "commit.gpgsign", "false"]);
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn write(&self, name: &str, contents: &str) {
        let path = self.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a parent directory");
        }
        std::fs::write(path, contents).expect("a writable temporary directory");
    }

    /// Runs git and fails the test if it does.
    pub fn run(&self, args: &[&str]) -> String {
        let output = self.try_run(args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// The same, for the commands whose failure is the point — a merge that
    /// conflicts, for instance.
    pub fn try_run(&self, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "FerroEdit Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "FerroEdit Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git is installed on a machine running these tests")
    }

    pub fn commit(&self, message: &str) {
        // No `-c commit.gpgsign=false` here any more: `identify` wrote it into
        // the repository, where `GitService`'s own commits see it too.
        self.run(&["commit", "-qam", message]);
    }

    /// `git status --short`, one entry per line — the human-facing form the
    /// panel is checked against.
    pub fn short_status(&self) -> Vec<String> {
        self.run(&["status", "--short"])
            .lines()
            .map(|line| line.trim_end().to_string())
            .filter(|line| !line.is_empty())
            .collect()
    }
}
