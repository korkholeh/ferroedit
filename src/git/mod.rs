//! Git integration over the system `git` binary (ADR-001).

pub mod models;
pub mod parser;
pub mod service;
#[cfg(test)]
pub mod testing;
pub mod worker;

pub use service::GitService;
