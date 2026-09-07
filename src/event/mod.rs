//! Terminal events and worker messages, normalised into a single `AppEvent`
//! stream consumed by the main loop.

pub mod keyboard;
pub mod mouse;

use std::sync::mpsc::Sender;
use std::thread;

use crossterm::event::{self as term, Event, KeyEvent, KeyEventKind, MouseEvent};

/// Everything the main loop can be woken by. Git and filesystem workers push
/// their own variants into the same channel from Phase 6 onwards.
#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Resize(u16, u16),
}

/// Reads terminal events on a dedicated thread.
///
/// The thread exists so background workers can push results into the same
/// channel without the main loop having to poll with a timeout. It is detached:
/// it blocks in `read()` until the process exits, and the `send` failure when
/// the receiver is dropped is its shutdown signal in every other case.
pub fn spawn_input_thread(tx: Sender<AppEvent>) {
    thread::Builder::new()
        .name("input".into())
        .spawn(move || input_loop(&tx))
        .expect("failed to spawn the input thread");
}

fn input_loop(tx: &Sender<AppEvent>) {
    loop {
        let event = match term::read() {
            Ok(event) => event,
            Err(err) => {
                log::error!("terminal input read failed: {err}");
                break;
            }
        };

        let app_event = match event {
            // The kitty and Windows protocols report key *release* as well.
            // Acting on both edges would run every shortcut twice.
            Event::Key(key) if key.kind == KeyEventKind::Release => continue,
            Event::Key(key) => AppEvent::Key(key),
            Event::Mouse(mouse) => AppEvent::Mouse(mouse),
            Event::Paste(text) => AppEvent::Paste(text),
            Event::Resize(width, height) => AppEvent::Resize(width, height),
            Event::FocusGained | Event::FocusLost => continue,
        };

        if tx.send(app_event).is_err() {
            // The main loop is gone; nothing left to deliver to.
            break;
        }
    }
}
