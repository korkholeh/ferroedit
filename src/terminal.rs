//! RAII terminal guard, the panic hook, and the signal handler that restore the
//! terminal before the process is allowed to die.
//!
//! Three restore paths exist on purpose (SPEC §41, ADR-006, ADR-073). `Drop`
//! covers normal returns *and* unwinding; the panic hook runs *before* the
//! payload is printed, so a backtrace never lands inside a raw-mode alternate
//! screen where the user cannot read it; and the signal handler covers the exits
//! that run neither — `SIGTERM`, `SIGHUP`, `SIGINT` and `SIGQUIT` from outside
//! the process. Running the teardown twice is harmless — every step is
//! idempotent.

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
        // Before raw mode and not after: `enable_raw_mode` records whatever it
        // finds as the mode to put back, so a terminal inherited broken has to
        // be mended before it is read (ADR-073).
        sanitize_inherited_mode();
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
///
/// The escape sequences and the terminal mode are restored independently. A
/// write that fails is a lost mouse-capture reset, which the next program to
/// draw will overwrite; a mode left in raw survives the process and every run
/// after it (ADR-073), so it is never skipped because a write went wrong.
pub fn restore() -> io::Result<()> {
    let mut stdout = io::stdout();
    let sequences = execute!(
        stdout,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
    let mode = disable_raw_mode();
    let flushed = stdout.flush();
    sequences.and(mode).and(flushed)
}

/// Chains a terminal restore in front of the existing panic hook.
pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = restore();
        previous(info);
    }));
}

/// Restores the terminal when the process is asked to die by a signal.
///
/// Neither `Drop` nor the panic hook runs for `SIGTERM`, `SIGHUP`, `SIGINT` or
/// `SIGQUIT`, so without this an editor that is killed — by a supervisor, by a
/// closed terminal window, by `kill` — leaves raw mode behind (ADR-073). The
/// work is done on a thread rather than in the handler itself, which is what
/// makes it safe to run arbitrary code at all: the handler only writes to a
/// pipe.
#[cfg(unix)]
pub fn install_signal_handler() {
    use std::thread;

    use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
    use signal_hook::iterator::Signals;

    let signals = match Signals::new([SIGINT, SIGTERM, SIGHUP, SIGQUIT]) {
        Ok(signals) => signals,
        // Best effort, like the watcher: an editor that cannot register for
        // signals is still an editor, and the two other restore paths remain.
        Err(err) => {
            log::warn!("no signal handler: {err}");
            return;
        }
    };
    let spawned = thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            let mut signals = signals;
            for signal in signals.forever() {
                log::info!("signal {signal}: restoring the terminal and standing down");
                let _ = restore();
                // Die of the signal that was sent rather than of an exit code
                // invented here, so a shell reports `^C` and a supervisor sees
                // the cause it is looking for. For a terminating signal this
                // does not return.
                let _ = signal_hook::low_level::emulate_default_handler(signal);
            }
        });
    if let Err(err) = spawned {
        log::warn!("no signal handler: {err}");
    }
}

#[cfg(not(unix))]
pub fn install_signal_handler() {}

/// Puts a terminal that was handed to us in raw mode back into a usable one.
///
/// Whoever left it that way is gone — this is the wreckage of an earlier run
/// killed before it could restore anything (ADR-073). It matters because
/// `enable_raw_mode` saves the mode it finds and `disable_raw_mode` puts *that*
/// back: read a raw terminal and the editor faithfully restores a raw terminal
/// on the way out, which is how one lost run breaks every run after it.
#[cfg(unix)]
fn sanitize_inherited_mode() {
    use std::fs::OpenOptions;
    use std::mem::MaybeUninit;
    use std::os::unix::io::AsRawFd;

    // The controlling terminal, because that is the device crossterm reads and
    // writes — stdin may have been redirected away from it.
    let Ok(tty) = OpenOptions::new().read(true).write(true).open("/dev/tty") else {
        return;
    };
    let fd = tty.as_raw_fd();
    let mut mode = MaybeUninit::<libc::termios>::uninit();
    // SAFETY: `fd` is open for the length of this function and `mode` is a
    // correctly aligned `termios` for `tcgetattr` to fill.
    if unsafe { libc::tcgetattr(fd, mode.as_mut_ptr()) } != 0 {
        return;
    }
    // SAFETY: `tcgetattr` returned success, so it initialised the value.
    let mut mode = unsafe { mode.assume_init() };
    if !needs_sanitizing(mode.c_lflag, mode.c_oflag) {
        return;
    }

    log::warn!("the terminal was inherited in raw mode; mending it before taking it over");
    // The three flag words are *set*, not merged: what is there is the mode
    // some other process wrote on its way out, so there is nothing in it worth
    // keeping. `IUTF8` is the exception — it says what the terminal is rather
    // than how it behaves, and clearing it would break every non-ASCII
    // character typed at the shell afterwards.
    let utf8 = mode.c_iflag & libc::IUTF8;
    mode.c_iflag = SANE_IFLAG | utf8;
    mode.c_oflag = SANE_OFLAG;
    mode.c_lflag = SANE_LFLAG;
    mode.c_cflag |= libc::CREAD | libc::CS8;
    // The control characters are left alone. Raw mode does not touch them, so
    // a terminal is never broken by way of them, and a user who has moved
    // their erase key keeps it.
    //
    // SAFETY: `fd` is still open and `mode` is the structure `tcgetattr` filled.
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &mode) };
}

#[cfg(not(unix))]
fn sanitize_inherited_mode() {}

/// What `stty sane` writes, which on both platforms is what a login shell is
/// handed: break and CR handling, flow control, and the bell on a full queue.
#[cfg(unix)]
const SANE_IFLAG: libc::tcflag_t =
    libc::BRKINT | libc::ICRNL | libc::IXON | libc::IXANY | libc::IMAXBEL;

/// Output processing, and the newline-to-CRLF translation whose absence is the
/// staircase every line of output walks down.
#[cfg(unix)]
const SANE_OFLAG: libc::tcflag_t = libc::OPOST | libc::ONLCR;

/// Line editing, signals, and the echo a shell is read by.
#[cfg(unix)]
const SANE_LFLAG: libc::tcflag_t = libc::ECHO
    | libc::ECHOE
    | libc::ECHOKE
    | libc::ECHOCTL
    | libc::ISIG
    | libc::ICANON
    | libc::IEXTEN;

/// The flags without which no shell works at all. Only these decide whether a
/// terminal is mended, so one a user has configured to taste — a bell they have
/// silenced, `IXON` off — is left exactly as they set it.
#[cfg(unix)]
const ESSENTIAL_LFLAG: libc::tcflag_t = libc::ICANON | libc::ECHO | libc::ISIG;

#[cfg(unix)]
const ESSENTIAL_OFLAG: libc::tcflag_t = libc::OPOST | libc::ONLCR;

/// Whether a terminal in this mode is one no shell could be used in.
#[cfg(unix)]
fn needs_sanitizing(lflag: libc::tcflag_t, oflag: libc::tcflag_t) -> bool {
    lflag & ESSENTIAL_LFLAG != ESSENTIAL_LFLAG || oflag & ESSENTIAL_OFLAG != ESSENTIAL_OFLAG
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_working_terminal_is_left_alone() {
        assert!(!needs_sanitizing(SANE_LFLAG, SANE_OFLAG));
        // Flags outside the essential set are none of our business: a user who
        // has silenced the bell or turned off flow control still has a shell.
        assert!(!needs_sanitizing(
            SANE_LFLAG | libc::NOFLSH,
            SANE_OFLAG | libc::ONOCR
        ));
        assert!(!needs_sanitizing(SANE_LFLAG & !libc::ECHOCTL, SANE_OFLAG));
    }

    /// The wreckage of a run that was killed: `ONLCR` gone is the staircase,
    /// `ECHO` and `ICANON` gone is the shell that answers nothing.
    #[test]
    fn a_terminal_left_in_raw_mode_is_mended() {
        assert!(needs_sanitizing(SANE_LFLAG, SANE_OFLAG & !libc::ONLCR));
        assert!(needs_sanitizing(SANE_LFLAG & !libc::ECHO, SANE_OFLAG));
        assert!(needs_sanitizing(SANE_LFLAG & !libc::ICANON, SANE_OFLAG));
        assert!(needs_sanitizing(SANE_LFLAG & !libc::ISIG, SANE_OFLAG));
        assert!(needs_sanitizing(0, 0));
    }

    /// The mode written back has to *be* sane, not merely contain the flags the
    /// check looks for.
    #[test]
    fn the_mode_written_back_covers_the_essentials() {
        assert_eq!(SANE_LFLAG & ESSENTIAL_LFLAG, ESSENTIAL_LFLAG);
        assert_eq!(SANE_OFLAG & ESSENTIAL_OFLAG, ESSENTIAL_OFLAG);
        assert!(!needs_sanitizing(SANE_LFLAG, SANE_OFLAG));
    }
}
