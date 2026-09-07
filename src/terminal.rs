//! RAII terminal guard and the panic hook that restores the terminal before
//! the payload is printed.
//!
//! Two restore paths exist on purpose (SPEC §41, ADR-006). `Drop` covers normal
//! returns *and* unwinding; the panic hook runs *before* the payload is printed,
//! so a backtrace never lands inside a raw-mode alternate screen where the user
//! cannot read it and the shell is left unusable. Running the teardown twice is
//! harmless — every step is idempotent.

use std::io::{self, Stdout, Write};
use std::panic;

use anyhow::Result;
use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// The one terminal type the whole app renders through.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Owns the terminal's non-default state for the lifetime of the run.
pub struct TerminalGuard {
    terminal: Tui,
}

impl TerminalGuard {
    /// Enters raw mode and the alternate screen, and turns on mouse capture and
    /// bracketed paste.
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        terminal.hide_cursor()?;
        terminal.clear()?;
        Ok(Self { terminal })
    }

    pub fn terminal(&mut self) -> &mut Tui {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // We may already be unwinding, so a failure here can only be logged.
        if let Err(err) = restore() {
            log::error!("terminal restore failed: {err}");
        }
    }
}

/// Undoes everything `TerminalGuard::new` turned on, in reverse order.
pub fn restore() -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(
        stdout,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    )?;
    disable_raw_mode()?;
    stdout.flush()
}

/// Chains a terminal restore in front of the existing panic hook.
pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = restore();
        previous(info);
    }));
}
