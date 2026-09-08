//! Headless editor core. Compiles and is testable without ratatui/crossterm.

pub mod charset;
pub mod clipboard;
pub mod coords;
pub mod cursor;
pub mod document;
pub mod history;
pub mod search;
pub mod selection;
pub mod viewport;
pub mod wrap;
