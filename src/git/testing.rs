//! A real repository in a temporary directory, for the tests that need one.
//!
//! Everything git integration does goes through the real binary (ADR-001), so
//! the tests do too: a fake that answered like git would only ever be as
//! correct as our idea of git. The repositories built here are isolated from
//! the machine's configuration — no global config, no system config, no
//! signing — so a developer with `commit.gpgsign = true` gets the same results
//! as CI.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    /// A repository with no commits, on `main`.
    pub fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().expect("a temporary directory"),
        };
        repo.run(&["-c", "init.defaultBranch=main", "init", "-q"]);
        repo
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
        self.run(&["-c", "commit.gpgsign=false", "commit", "-qam", message]);
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
