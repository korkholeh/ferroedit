//! Git integration over the system `git` binary (ADR-001).

pub mod diff;
pub mod log;
pub mod models;
pub mod parser;
pub mod service;
#[cfg(test)]
pub mod testing;
pub mod worker;

pub use diff::Diff;
pub use log::{Commit, LogScope};
pub use service::GitService;
pub use worker::{GitJob, GitWorker, JobFailure, JobId, JobOutcome};
