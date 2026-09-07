//! Transient status-bar messages with expiry.

use std::time::{Duration, Instant};

/// How long a message stays on the status bar before it clears itself.
pub const TTL: Duration = Duration::from_secs(4);

/// `Error` arrived in Phase 2 with the first thing that can fail in front of
/// the user: a save the filesystem refuses (SPEC §39).
///
/// The three are told apart by **what happened to the document, the
/// repository or the disk**, not by how serious the sentence sounds (ADR-046).
/// The rule is the one thing about a message a reader learns once and then
/// reads by colour alone, so it has to be mechanical:
///
/// - `Info` — it was done. `Saved main.rs`, `Copied 5 characters`.
/// - `Warning` — nothing was done, and the reason is the state the editor is
///   in rather than anything going wrong. `No file to save`, `Nothing to undo`.
/// - `Error` — it was attempted and something outside the editor refused:
///   the filesystem, git, the terminal. `Failed to save: Permission denied`.
///
/// "Nothing changed" is the whole of the middle case. It is a duller line than
/// "this is bad", and it is the one a user can act on: a yellow line means the
/// keystroke landed and produced nothing, which is exactly the moment a silent
/// no-op would leave them pressing the key again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Info,
    Warning,
    Error,
}

#[derive(Debug)]
pub struct Notification {
    pub message: String,
    pub kind: NotificationKind,
    shown_at: Instant,
}

/// A single slot, not a queue: the status bar has room for one line, and the
/// newest message is always the relevant one (SPEC §39).
#[derive(Debug, Default)]
pub struct Notifications {
    current: Option<Notification>,
}

impl Notifications {
    pub fn push(&mut self, kind: NotificationKind, message: impl Into<String>) {
        let message = message.into();
        log::info!("notification ({kind:?}): {message}");
        self.current = Some(Notification {
            message,
            kind,
            shown_at: Instant::now(),
        });
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.push(NotificationKind::Info, message);
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.push(NotificationKind::Warning, message);
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.push(NotificationKind::Error, message);
    }

    /// Clears an expired message. Called from the event loop, never from
    /// rendering, so that `ui/` stays read-only over `App`.
    pub fn prune(&mut self) {
        if self.expired() {
            self.current = None;
        }
    }

    fn expired(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|n| n.shown_at.elapsed() >= TTL)
    }

    pub fn current(&self) -> Option<&Notification> {
        self.current.as_ref()
    }

    /// When the visible message stops being visible, so the event loop can wake
    /// up to redraw instead of leaving a stale message on a quiet terminal.
    pub fn expires_at(&self) -> Option<Instant> {
        self.current.as_ref().map(|n| n.shown_at + TTL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_message_is_visible_and_has_a_deadline() {
        let mut n = Notifications::default();
        assert!(n.current().is_none());
        assert!(n.expires_at().is_none());

        n.info("Saved main.rs");
        n.prune();
        assert_eq!(n.current().unwrap().message, "Saved main.rs");
        assert!(n.expires_at().is_some());
    }

    #[test]
    fn pushing_replaces_the_visible_message() {
        let mut n = Notifications::default();
        n.info("first");
        n.warning("second");
        assert_eq!(n.current().unwrap().message, "second");
        assert_eq!(n.current().unwrap().kind, NotificationKind::Warning);
    }
}
