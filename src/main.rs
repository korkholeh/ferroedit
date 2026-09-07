//! FerroEdit — a modern desktop-like text editor that happens to run in a terminal.
//!
//! This file owns exactly four things (`docs/ARCHITECTURE.md` §1): CLI parsing,
//! terminal setup and teardown, the panic hook, and the run loop. Everything it
//! does to `App` goes through `execute_command`.

mod app;
mod cli;
mod commands;
mod config;
mod docs;
mod editor;
mod event;
mod filesystem;
mod git;
mod syntax;
mod terminal;
mod ui;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Instant;

use anyhow::Result;
use directories::ProjectDirs;
use log::LevelFilter;
use simplelog::WriteLogger;

use app::focus::FocusTarget;
use app::workspace::Workspace;
use app::{App, EditorView};
use cli::Cli;
use commands::execute::execute_command;
use commands::Command;
use event::{spawn_input_thread, AppEvent};
use terminal::TerminalGuard;
use ui::layout::LayoutRects;
use ui::theme::Theme;

fn main() -> ExitCode {
    let cli = Cli::parse_args();
    init_logging();

    // The one thing the binary prints to stdout besides `--help` and
    // `--version`: `docs/SHORTCUTS.md`, rendered from the keymap tables. It
    // runs before the terminal is touched, so it is usable in a pipe (ADR-028).
    if cli.dump_shortcuts {
        print!("{}", docs::shortcuts_markdown());
        return ExitCode::SUCCESS;
    }

    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        // The guard has already restored the terminal by the time we get here,
        // so stderr is safe to write to again.
        Err(err) => {
            eprintln!("ferroedit: {err:#}");
            log::error!("exiting with an error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    let workspace = Workspace::from_arg(cli.workspace_arg())?;
    log::info!(
        "opening workspace {} (line argument: {:?})",
        workspace.root().display(),
        cli.line
    );

    let mut app = App::new(workspace);
    log::info!(
        "clipboard writes go to the {}",
        app.clipboard.outward_name()
    );
    open_cli_files(&mut app, cli);
    let theme = Theme::default();

    // The hook goes in before the guard so a panic inside `TerminalGuard::new`
    // is also covered.
    terminal::install_panic_hook();
    let mut guard = TerminalGuard::new()?;

    if cli.panic_test {
        panic!("--panic-test: forcing a panic to verify terminal restore");
    }

    let (tx, rx) = mpsc::channel();
    spawn_input_thread(tx);

    let mut rects = LayoutRects::default();
    while !app.should_quit {
        app.notifications.prune();

        // Colour the lines that are about to be drawn. Before the draw and not
        // inside it, because `ui/` only ever reads `App` and a cache has to
        // write (ARCHITECTURE §1). It is a no-op unless the viewport moved or
        // the document changed.
        app.sync_highlight();
        app.sync_search();

        // The rects are computed inside the draw closure, where the real frame
        // area is known, and handed back out for the next mouse hit-test.
        let mut drawn = None;
        guard.terminal().draw(|frame| {
            let frame_rects = ui::layout::compute(frame.area(), &app);
            ui::render(frame, &app, &frame_rects, &theme);
            drawn = Some(frame_rects);
        })?;
        if let Some(frame_rects) = drawn {
            rects = frame_rects;
        }
        // The pane's size is an input to scrolling and is only known once a
        // frame has been laid out. When it changes — at startup, and on every
        // resize — the cursor may have fallen outside the new window, so the
        // corrected frame is drawn straight away instead of waiting for the
        // next key press.
        if sync_editor_view(&mut app, &rects) {
            continue;
        }

        // Block for the first event, then drain what is already queued, so a
        // paste burst or a fast scroll costs one frame instead of N.
        match next_event(&rx, &app) {
            Some(event) => handle_event(&mut app, &rects, event),
            // Timed out waiting for a notification to expire: redraw only.
            None if !rx_is_alive(&rx) => break,
            None => {}
        }
        while let Ok(event) = rx.try_recv() {
            handle_event(&mut app, &rects, event);
        }
    }

    log::info!("shutting down");
    Ok(())
}

/// Mirrors the pane sizes onto `App` and scrolls the cursor back into view when
/// the editor's changed. Returns whether anything moved.
fn sync_editor_view(app: &mut App, rects: &LayoutRects) -> bool {
    // One row of the explorer panel is its title; the rest is the list. The
    // selection only has to be corrected when a command moves it, so a changed
    // sidebar height needs no redraw of its own.
    app.explorer_rows = rects.explorer.height.saturating_sub(1);

    let view = EditorView {
        width: rects.editor.width,
        height: rects.editor.height,
    };
    if view == app.editor_view {
        return false;
    }
    app.editor_view = view;
    if let Some(tab) = app.active_mut() {
        tab.follow_cursor(view);
    }
    true
}

/// Opens the files named on the command line, if there are any.
///
/// A directory argument is the workspace and opens no tab; file arguments are
/// opened as tabs, left to right, with the first one active and the `+42` line
/// applying to it. A failure is reported and the editor still starts — the user
/// is better served by an editor with an explanation than by a process that
/// exits before they see it, and one unreadable file among several must not
/// cost them the rest.
fn open_cli_files(app: &mut App, cli: &Cli) {
    let files: Vec<&Path> = cli
        .paths
        .iter()
        .map(PathBuf::as_path)
        .filter(|path| !path.is_dir())
        .collect();
    if files.is_empty() {
        app.notifications
            .info(format!("Opened {}", app.workspace.name()));
        return;
    }

    let mut opened = 0;
    for (index, path) in files.iter().enumerate() {
        let existed = path.exists();
        // The line argument belongs to the file it was written next to, which
        // is the first one.
        let line = (index == 0).then_some(cli.line).flatten();
        match app.open_path(path, line) {
            Ok(()) => {
                opened += 1;
                if files.len() == 1 && existed {
                    app.notifications.info(format!("Opened {}", path.display()));
                } else if files.len() == 1 {
                    // Nothing is on disk yet: say so, so that an empty screen is
                    // not mistaken for a file that failed to load.
                    app.notifications
                        .info(format!("New file {} — Ctrl+S to create it", path.display()));
                }
            }
            Err(err) => {
                log::error!("could not open {}: {err}", path.display());
                app.notifications.error(format!("{err}"));
            }
        }
    }
    if files.len() > 1 && opened > 0 {
        app.notifications.info(format!("Opened {opened} files"));
    }
    // The first file that opened is the one to start on, not the last.
    app.active_tab = (!app.tabs.is_empty()).then_some(0);
}

/// Waits for the next event, waking early when a notification is due to expire
/// so a stale message cannot sit on the status bar of an idle terminal.
fn next_event(rx: &mpsc::Receiver<AppEvent>, app: &App) -> Option<AppEvent> {
    match app.notifications.expires_at() {
        Some(deadline) => {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => None,
            }
        }
        None => rx.recv().ok(),
    }
}

/// Distinguishes "timed out" from "the input thread is gone" after `next_event`
/// returns `None`.
fn rx_is_alive(rx: &mpsc::Receiver<AppEvent>) -> bool {
    !matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected))
}

fn handle_event(app: &mut App, rects: &LayoutRects, event: AppEvent) {
    let command = match event {
        AppEvent::Key(key) => event::keyboard::resolve(key, app.focus, app.dialog_wants_text()),
        AppEvent::Mouse(mouse) => {
            log::trace!("mouse {:?} at {},{}", mouse.kind, mouse.column, mouse.row);
            event::mouse::hit_test(app, rects, mouse)
        }
        // Bracketed paste: the terminal hands over the text in one piece, so
        // it becomes one insert instead of N keystrokes — and one undo step in
        // Phase 4. It only goes into the document when the editor has focus;
        // pasting into a sidebar would otherwise type into a file invisibly.
        AppEvent::Paste(text) => {
            log::debug!("bracketed paste of {} bytes", text.len());
            app.clipboard.remember(&text);
            // A dialog's field takes a paste too — a path pasted into Rename is
            // the obvious case — and it is the one place other than the editor
            // where typing goes anywhere at all.
            if app.dialog_wants_text() {
                Some(Command::DialogInputText(text.replace(['\n', '\r'], " ")))
            } else if app.focus == FocusTarget::Search {
                Some(Command::SearchInputText(text))
            } else {
                (app.focus == FocusTarget::Editor).then_some(Command::InsertText(text))
            }
        }
        AppEvent::Resize(width, height) => {
            log::debug!("resize to {width}x{height}");
            // ratatui re-reads the size on the next draw; the loop redraws
            // every iteration, so there is nothing else to do.
            None
        }
    };

    if let Some(command) = command {
        execute_command(app, command);
    }
}

/// Sets up file logging.
///
/// stdout belongs to the TUI (SPEC §46), so logs go to a file or nowhere at
/// all — never to the screen. A logging failure is not worth aborting a run
/// over, so every step here degrades to "no logging".
fn init_logging() {
    let level = std::env::var("FERROEDIT_LOG")
        .ok()
        .and_then(|value| value.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Info);
    if level == LevelFilter::Off {
        return;
    }

    let Some(path) = log_path() else { return };
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    // Dependencies are chatty at trace level (mio in particular); the log is
    // for diagnosing FerroEdit, so only our own records are kept.
    let config = simplelog::ConfigBuilder::new()
        .add_filter_allow_str("ferroedit")
        .build();
    if let Ok(file) = File::create(&path) {
        let _ = WriteLogger::init(level, config, file);
    }
}

fn log_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "ferroedit")?;
    // `state_dir` exists on Linux only; elsewhere the local data dir is the
    // conventional home for this kind of file.
    let base = dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir());
    Some(base.join("ferroedit.log"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    /// A file opened at a line past the first screen has to be scrolled to, and
    /// the size that decides how far is not known until a frame has been laid
    /// out — so the first sync is what makes `ferroedit +42 file` land.
    #[test]
    fn the_first_layout_scrolls_the_cursor_into_view() {
        let mut app = App::fixture();
        app.tabs[0] = app::Tab::scratch("long.txt", &"line\n".repeat(60));
        app.editor_view = app::EditorView::default();
        app.tabs[0].document.goto_line(42);

        let rects = LayoutRects {
            editor: Rect::new(20, 2, 60, 20),
            ..LayoutRects::default()
        };

        assert!(sync_editor_view(&mut app, &rects), "the size changed");
        assert_eq!(app.tabs[0].viewport.top_line, 22, "line 42 is the last row");
        assert!(!sync_editor_view(&mut app, &rects), "and settles");
    }
}
