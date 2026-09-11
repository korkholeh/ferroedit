//! FerroEdit — a modern desktop-like text editor that happens to run in a terminal.
//!
//! This file owns exactly four things (`docs/ARCHITECTURE.md` §1): CLI parsing,
//! terminal setup and teardown, the panic and signal hooks, and the run loop.
//! Everything it does to `App` goes through `execute_command`.

mod app;
mod cli;
mod commands;
mod config;
mod docs;
mod editor;
mod event;
mod filesystem;
mod git;
mod image;
mod syntax;
mod terminal;
mod ui;
mod update;

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
use app::image::Canvas;
use app::workspace::Workspace;
use app::{App, EditorView, Opened};
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
    // The settings file is read here and nowhere else: `App::new` is what every
    // test builds, and none of them may touch the user's real config.
    app.settings = config::Settings::load();
    app.persist_settings = true;
    log::info!(
        "clipboard writes go to the {}",
        app.clipboard.outward_name()
    );
    open_cli_files(&mut app, cli);

    // The one channel every producer writes into: the input thread, and the git
    // worker with the operations that must not block a frame (ADR-033). It is
    // built before the panel is, because attaching the worker is what lets the
    // panel run anything at all.
    let (tx, rx) = mpsc::channel();
    app.git.attach_worker(tx.clone());
    // Kept so the watcher can be started again on another root: Open Folder
    // moves the workspace while the editor runs (ADR-051).
    app.events = Some(tx.clone());
    // The git panel is drawn from a status, and a status is a subprocess: it
    // runs once here, before the first frame, and after that only when
    // something has changed (SPEC §30).
    let root = app.workspace.root().to_path_buf();
    app.git.discover(&root);

    // Both go in before the guard so a panic — or a signal — during
    // `TerminalGuard::new` is also covered. The signal handler is the third
    // restore path: `Drop` and the panic hook between them miss every exit the
    // process does not choose (ADR-073).
    terminal::install_panic_hook();
    terminal::install_signal_handler();
    let mut guard = TerminalGuard::new()?;

    if cli.panic_test {
        panic!("--panic-test: forcing a panic to verify terminal restore");
    }

    // The watcher goes in after the guard so a failure to start it can be said
    // on the status bar rather than printed over a terminal that is about to be
    // taken over. It is best effort: without it the editor behaves exactly as
    // it did before, and `F5` still re-reads everything (ADR-040).
    match filesystem::watcher::spawn(&root, tx.clone()) {
        // Held on `App`, because dropping it is how a watch is ended — and one
        // has to end whenever the workspace root moves.
        Ok(watch) => app.watch = Some(watch),
        Err(err) => {
            log::warn!("no filesystem watcher: {err}");
            app.notifications.warning(format!(
                "Not watching for outside changes: {err} — F5 refreshes"
            ));
        }
    }

    // Last of the three background producers to be started, and the only one
    // the user can switch off: asking GitHub about a newer release is a
    // request to a third party, so it is a setting and not a fact of running
    // the editor (ADR-065). Nothing waits for the answer.
    start_update_check(&app, &tx);

    spawn_input_thread(tx);

    let mut rects = LayoutRects::default();
    // The tab the strip was last scrolled to show. The strip follows the active
    // tab when it *changes* and not on every frame, so that scrolling it away
    // from the file being edited sticks (SPEC §11).
    let mut shown_tab = app.active_tab;
    let mut theme_kind = app.settings.theme;
    let mut theme = Theme::new(theme_kind);
    while !app.should_quit {
        app.notifications.prune();

        // The next slice of a large file, if one is being read (ADR-077).
        // Before the draw, like the caches below it: the box on screen is drawn
        // from what this leaves behind, and a read that lands inside its first
        // slice therefore never draws one at all.
        app.advance_open();
        // A tab can arrive without a command having put it there: the files
        // named on the command line, and the last slice of a large one, both
        // land here. Focus has to follow the tab in front either way, or the
        // pane on screen is one whose own keys are not bound yet and the first
        // press is swallowed (SPEC §26).
        app.normalize_focus();

        // Colour the lines that are about to be drawn. Before the draw and not
        // inside it, because `ui/` only ever reads `App` and a cache has to
        // write (ARCHITECTURE §1). It is a no-op unless the viewport moved or
        // the document changed.
        app.sync_highlight();
        app.sync_image();
        app.sync_table();
        app.sync_search();

        if shown_tab != app.active_tab {
            shown_tab = app.active_tab;
            app.tab_scroll = ui::layout::tab_scroll_showing(&app);
        }

        // Rebuilt when the choice changes rather than on every frame: a theme
        // is two dozen colours, and the View menu is not a hot path.
        if theme_kind != app.settings.theme {
            theme_kind = app.settings.theme;
            theme = Theme::new(theme_kind);
        }

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
            Wait::Event(event) => handle_event(&mut app, &rects, event),
            // Woken with nothing queued: a notification due to expire, or a
            // search with more of the buffer to walk. Redraw only.
            Wait::Idle => {}
            Wait::Closed => break,
        }
        while let Ok(event) = rx.try_recv() {
            handle_event(&mut app, &rects, event);
        }
    }

    log::info!("shutting down");
    Ok(())
}

/// Starts the start-up update check when the settings ask for one and the last
/// one was long enough ago (ADR-065).
///
/// The decision is here rather than in `execute_command` because it is
/// start-up wiring like the watcher above it: the *answer* goes back through
/// the command door, which is what keeps the mutation in one place. The
/// timestamp is written when the check answers, so a check that never returns
/// leaves the next launch free to try again.
fn start_update_check(app: &App, tx: &mpsc::Sender<AppEvent>) {
    if !app.settings.check_for_updates {
        log::info!("the update check is switched off");
        return;
    }
    let Some(now) = update::now_secs() else {
        return;
    };
    if !update::is_due(app.settings.last_update_check, now) {
        log::debug!("the update check is not due yet");
        return;
    }
    update::spawn(tx.clone(), false);
}

/// Mirrors the pane sizes onto `App` and scrolls the cursor back into view when
/// the editor's changed. Returns whether anything moved.
fn sync_editor_view(app: &mut App, rects: &LayoutRects) -> bool {
    // One row of the explorer panel is its title; the rest is the list. The
    // selection only has to be corrected when a command moves it, so a changed
    // sidebar height needs no redraw of its own.
    app.explorer_rows = rects.explorer.height.saturating_sub(1);
    app.git_rows = rects.git_panel.height.saturating_sub(1);
    // Two rows of border, like the dialog's; paging through a diff is measured
    // against what is left.
    app.diff_rows = rects.diff.map_or(0, |diff| diff.height.saturating_sub(2));
    // Already the inside of the log's frame, less its field's row, so nothing
    // is subtracted here (ADR-068).
    app.log_rows = rects.log_rows.map_or(0, |rows| rows.height);
    // The picture's pane, already inside the frame and beside the metadata
    // column. Unlike the rows above it this one is *returned*: the zoom, the
    // pan and the block grid are all built against it, so a frame drawn before
    // it was known would be a picture at the wrong scale (ADR-078).
    let canvas = rects
        .image_canvas
        .map_or(Canvas::default(), |canvas| Canvas {
            width: canvas.width,
            height: canvas.height,
        });
    let canvas_moved = canvas != app.image_canvas;
    app.image_canvas = canvas;
    // The help screen wraps its notes, so its width is geometry the scroll
    // depends on as much as its height is (ADR-038).
    app.help_rows = rects.help.map_or(0, |help| help.height.saturating_sub(2));
    app.help_cols = rects.help.map_or(0, |help| help.width.saturating_sub(2));
    // Already the inner area of the browser's frame, so nothing is subtracted.
    app.dialog_rows = rects.dialog_list.map_or(0, |list| list.height);
    app.tab_bar_width = rects.tab_bar.width;

    let view = EditorView {
        width: rects.editor.width,
        height: rects.editor.height,
    };
    if view == app.editor_view {
        return canvas_moved;
    }
    app.editor_view = view;
    let text_view = app.text_view();
    if let Some(tab) = app.active_mut() {
        tab.follow_cursor(text_view);
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
            // A large file is read a slice at a time and lands after the first
            // frames (ADR-077). It counts as opened here all the same: the
            // read has started, and the loop below is what carries it.
            Ok(opened_now) => {
                opened += 1;
                let landed = opened_now == Opened::Now;
                if landed && files.len() == 1 && existed {
                    app.notifications.info(format!("Opened {}", path.display()));
                } else if landed && files.len() == 1 {
                    // Nothing is on disk yet: say so, so that an empty screen is
                    // not mistaken for a file that failed to load.
                    app.notifications
                        .info(format!("New file {} — Ctrl+S to create it", path.display()));
                }
            }
            Err(err) => {
                log::error!("could not open {}: {err}", path.display());
                app.notifications.error(format!("Failed to open: {err}"));
            }
        }
    }
    if files.len() > 1 {
        // Several files named at once are all wanted, and the last of them may
        // still be being read. Nothing has been drawn yet — the terminal is not
        // even in raw mode — so there is no animation to keep and the read is
        // finished here, which is also what keeps the *first* file the one the
        // editor starts on.
        app.finish_pending_open();
        if opened > 0 {
            app.notifications.info(format!("Opened {opened} files"));
        }
    }
    // The first file that opened is the one to start on, not the last.
    app.active_tab = (!app.tabs.is_empty()).then_some(0);
}

/// Waits for the next event, waking early when a notification is due to expire
/// so a stale message cannot sit on the status bar of an idle terminal.
///
/// A search still walking the buffer does not wait at all (ADR-076), and
/// neither does a large file still being read (ADR-077): the next frame is the
/// one that carries the work on and draws the next turn of the spinner, so it
/// is taken as soon as whatever is already queued has been handled. This is the
/// whole of the editor's animation clock, and it stops the moment the last of
/// them lands — an idle terminal is never woken to draw.
fn next_event(rx: &mpsc::Receiver<AppEvent>, app: &App) -> Wait {
    if app.search.is_scanning() || app.opening.is_some() {
        return match rx.try_recv() {
            Ok(event) => Wait::Event(event),
            Err(mpsc::TryRecvError::Empty) => Wait::Idle,
            Err(mpsc::TryRecvError::Disconnected) => Wait::Closed,
        };
    }
    match app.notifications.expires_at() {
        Some(deadline) => {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(event) => Wait::Event(event),
                Err(RecvTimeoutError::Timeout) => Wait::Idle,
                Err(RecvTimeoutError::Disconnected) => Wait::Closed,
            }
        }
        None => match rx.recv() {
            Ok(event) => Wait::Event(event),
            Err(_) => Wait::Closed,
        },
    }
}

/// What one turn of the loop got out of the channel.
///
/// Three outcomes rather than an `Option`, because "nothing yet" and "the input
/// thread is gone" are different answers and the only way to tell them apart
/// used to be a second `try_recv` — which threw away any event that arrived
/// between the two calls. Rare when the only thing that woke the loop early was
/// a notification expiring; every frame, once a search is animating.
enum Wait {
    Event(AppEvent),
    /// Woken with nothing to handle: redraw, and go round again.
    Idle,
    Closed,
}

fn handle_event(app: &mut App, rects: &LayoutRects, event: AppEvent) {
    let command = match event {
        AppEvent::Key(key) => {
            // Traced like the mouse, and for the reason the mouse is: when a
            // shortcut "does nothing", the first question is whether the key
            // reached the process at all. Several terminals never send `Alt`,
            // and the log is the only place that difference is visible.
            log::trace!(
                "key {:?}+{:?} (focus {:?})",
                key.modifiers,
                key.code,
                app.focus
            );
            event::keyboard::resolve(key, app.focus, app.dialog_wants_text())
        }
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
            } else if app.focus == FocusTarget::LogSearch {
                // One line, like a dialog's field: the log's search box is one
                // row and a pasted newline would be invisible in it.
                Some(Command::LogSearchText(text.replace(['\n', '\r'], " ")))
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
        // A finished background job. It becomes a `Command` like everything
        // else, so the worker mutates `App` through the same single door as the
        // keyboard (ARCHITECTURE invariant 3).
        AppEvent::GitJob(outcome) => Some(Command::GitJobFinished(outcome)),
        // A coalesced burst of filesystem events, through the same door.
        AppEvent::FilesChanged(change) => Some(Command::ExternalChange(change)),
        // And the update check's one answer, through it as well.
        AppEvent::UpdateChecked(check) => Some(Command::UpdateCheckFinished(check)),
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
        app.tabs[0] = app::TabItem::editing(app::Tab::scratch("long.txt", &"line\n".repeat(60)));
        app.editor_view = app::EditorView::default();
        app.tab_mut(0).document.goto_line(42);

        let rects = LayoutRects {
            editor: Rect::new(20, 2, 60, 20),
            ..LayoutRects::default()
        };

        assert!(sync_editor_view(&mut app, &rects), "the size changed");
        assert_eq!(
            app.tab_mut(0).viewport.top_line,
            22,
            "line 42 is the last row"
        );
        assert!(!sync_editor_view(&mut app, &rects), "and settles");
    }
}
