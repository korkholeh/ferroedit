//! `ClipboardProvider` trait, OSC52 writer and the internal register.
//!
//! ADR-005: the terminal is the only clipboard that works over SSH, so OSC52 is
//! the default write path and the internal register is what reads back — most
//! terminals refuse an OSC52 paste-back for good security reasons. The native
//! `arboard` backend is behind the non-default `native-clipboard` feature.
//!
//! Nothing here may abort an edit: a copy that cannot reach the terminal is a
//! notification, not an error dialog, so every failure is reported as a
//! `ClipboardError` the caller is free to show and forget.

use std::io::{self, Write};

use thiserror::Error;

/// Bytes of *encoded* payload past which the OSC52 write is skipped.
///
/// Terminals buffer an escape sequence whole; tmux truncates around 74 KB by
/// default and some terminals lock up on much larger ones. A big copy still
/// lands in the internal register, so pasting inside FerroEdit keeps working —
/// only the trip to the system clipboard is given up.
const OSC52_LIMIT: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("the clipboard is empty")]
    Empty,
    #[error("the terminal did not accept the copy: {0}")]
    Terminal(#[source] io::Error),
    #[error("{0} bytes is too much for the terminal clipboard")]
    TooLarge(usize),
    #[cfg(feature = "native-clipboard")]
    #[error("system clipboard: {0}")]
    System(String),
}

/// Somewhere text can be put and taken back.
///
/// The trait exists so the editor never learns which of the three mechanisms it
/// is talking to, and so tests can hand it a sink instead of a terminal.
pub trait ClipboardProvider {
    fn set(&mut self, text: &str) -> Result<(), ClipboardError>;
    fn get(&mut self) -> Result<String, ClipboardError>;
    /// Shown in the log and in clipboard failure messages.
    fn name(&self) -> &'static str;
}

/// The always-available fallback: one string, inside the process.
#[derive(Debug, Default)]
pub struct Register {
    text: Option<String>,
}

impl ClipboardProvider for Register {
    fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.text = Some(text.to_string());
        Ok(())
    }

    fn get(&mut self) -> Result<String, ClipboardError> {
        self.text.clone().ok_or(ClipboardError::Empty)
    }

    fn name(&self) -> &'static str {
        "internal register"
    }
}

/// Writes the OSC52 escape sequence that asks the terminal to put text on the
/// *user's* clipboard — the one on their local machine, even across SSH.
///
/// The sink is a parameter rather than `stdout` so the sequence can be asserted
/// in a test, and so a future "copy to a file" is a different sink rather than a
/// different code path.
pub struct Osc52<W: Write> {
    sink: W,
}

impl Osc52<io::Stdout> {
    /// The real one: the terminal FerroEdit is drawing on.
    pub fn stdout() -> Self {
        Self::new(io::stdout())
    }
}

impl<W: Write> Osc52<W> {
    pub fn new(sink: W) -> Self {
        Self { sink }
    }
}

impl<W: Write> ClipboardProvider for Osc52<W> {
    fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        let payload = base64(text.as_bytes());
        if payload.len() > OSC52_LIMIT {
            return Err(ClipboardError::TooLarge(text.len()));
        }
        // ESC ] 52 ; c ; <base64> BEL. `c` is the CLIPBOARD selection; BEL is
        // accepted by more terminals as a terminator than ST is, tmux included.
        self.sink
            .write_all(format!("\x1b]52;c;{payload}\x07").as_bytes())
            .and_then(|()| self.sink.flush())
            .map_err(ClipboardError::Terminal)
    }

    /// OSC52 reads require the terminal to answer a query, which most refuse
    /// outright and none answer synchronously. The register is what reads.
    fn get(&mut self) -> Result<String, ClipboardError> {
        Err(ClipboardError::Empty)
    }

    fn name(&self) -> &'static str {
        "terminal (OSC52)"
    }
}

/// The system clipboard, when the binary was built with it.
#[cfg(feature = "native-clipboard")]
#[derive(Default)]
pub struct Native;

#[cfg(feature = "native-clipboard")]
impl ClipboardProvider for Native {
    fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text.to_string()))
            .map_err(|err| ClipboardError::System(err.to_string()))
    }

    fn get(&mut self) -> Result<String, ClipboardError> {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.get_text())
            .map_err(|err| ClipboardError::System(err.to_string()))
    }

    fn name(&self) -> &'static str {
        "system clipboard"
    }
}

/// Two providers tried in order: the second is asked only when the first
/// cannot do the job.
#[cfg(feature = "native-clipboard")]
struct Fallback {
    first: Box<dyn ClipboardProvider + Send>,
    second: Box<dyn ClipboardProvider + Send>,
}

#[cfg(feature = "native-clipboard")]
impl ClipboardProvider for Fallback {
    fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        match self.first.set(text) {
            Ok(()) => Ok(()),
            Err(err) => {
                log::debug!("{} refused the copy: {err}", self.first.name());
                self.second.set(text)
            }
        }
    }

    fn get(&mut self) -> Result<String, ClipboardError> {
        self.first.get().or_else(|_| self.second.get())
    }

    fn name(&self) -> &'static str {
        self.first.name()
    }
}

/// What the editor actually holds: a register that always works, plus a
/// best-effort route to the user's own clipboard.
///
/// A copy goes to both. A paste comes from the register, or — with the native
/// feature — from the system clipboard when it has something newer to offer.
/// The outward route failing is reported to the caller *after* the register has
/// already been written, so cut and copy always work inside the editor.
pub struct Clipboard {
    register: Register,
    outward: Box<dyn ClipboardProvider + Send>,
}

impl Clipboard {
    pub fn new(outward: Box<dyn ClipboardProvider + Send>) -> Self {
        Self {
            register: Register::default(),
            outward,
        }
    }

    /// The default chain for a running editor.
    #[cfg(not(feature = "native-clipboard"))]
    pub fn for_terminal() -> Self {
        Self::new(Box::new(Osc52::stdout()))
    }

    /// With `native-clipboard`, the system clipboard is the outward route and
    /// can also be read back, so a copy made in another application pastes.
    /// OSC52 stays behind it: a binary built with the feature is still run over
    /// SSH, where there is no local clipboard for `arboard` to reach.
    #[cfg(feature = "native-clipboard")]
    pub fn for_terminal() -> Self {
        Self::new(Box::new(Fallback {
            first: Box::new(Native),
            second: Box::new(Osc52::stdout()),
        }))
    }

    /// A clipboard that reaches nothing outside the process — tests, where an
    /// OSC52 sequence on the runner's stdout would be noise at best.
    #[cfg(test)]
    pub fn detached() -> Self {
        Self::new(Box::new(Osc52::new(io::sink())))
    }

    /// Copies text, reporting whether it reached beyond this process.
    ///
    /// The register is written first and unconditionally: whatever the terminal
    /// does, the text is in FerroEdit's own clipboard.
    pub fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        let _ = self.register.set(text);
        self.outward.set(text)
    }

    /// The text to paste.
    ///
    /// The outward provider is asked first only when it can actually read —
    /// OSC52 cannot, and asking it would cost a failed round trip on every
    /// paste — so in the default build this is the register.
    pub fn get(&mut self) -> Result<String, ClipboardError> {
        match self.outward.get() {
            Ok(text) if !text.is_empty() => Ok(text),
            _ => self.register.get(),
        }
    }

    /// Records text that arrived from outside as the current clipboard content.
    ///
    /// Bracketed paste is the terminal handing over what the user's clipboard
    /// holds; remembering it means a following `Ctrl+V` repeats the same text
    /// rather than pasting whatever FerroEdit last copied.
    pub fn remember(&mut self, text: &str) {
        let _ = self.register.set(text);
    }

    pub fn outward_name(&self) -> &'static str {
        self.outward.name()
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::for_terminal()
    }
}

impl std::fmt::Debug for Clipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Clipboard")
            .field("outward", &self.outward.name())
            .finish_non_exhaustive()
    }
}

/// Standard base64, which is all OSC52 needs.
///
/// Hand-written rather than pulled in as a dependency: it is fifteen lines, it
/// is on no hot path, and the alternative is another crate in a binary whose
/// point is to link statically everywhere.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let triple = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                let sextet = (triple >> (18 - index * 6)) & 0x3f;
                out.push(ALPHABET[sextet as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sink that keeps what was written, standing in for the terminal.
    #[derive(Default)]
    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Sink {
        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
        fn handle(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn base64_matches_the_rfc_test_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encodes_the_bytes_of_non_ascii_text() {
        // Cyrillic is two bytes per character; the encoder must see bytes, not
        // chars, or a paste comes back mangled.
        assert_eq!(base64("Привіт".as_bytes()), "0J/RgNC40LLRltGC");
    }

    #[test]
    fn osc52_writes_the_escape_sequence_the_terminal_expects() {
        let sink = Sink::default();
        let mut clipboard = Osc52::new(sink.handle());
        clipboard.set("foo").unwrap();
        assert_eq!(sink.contents(), "\x1b]52;c;Zm9v\x07");
    }

    #[test]
    fn osc52_refuses_a_payload_no_terminal_would_take() {
        let sink = Sink::default();
        let mut clipboard = Osc52::new(sink.handle());
        let huge = "x".repeat(OSC52_LIMIT);
        assert!(matches!(
            clipboard.set(&huge),
            Err(ClipboardError::TooLarge(_))
        ));
        assert!(sink.contents().is_empty(), "nothing is half-written");
    }

    #[test]
    fn osc52_cannot_be_read_back() {
        let mut clipboard = Osc52::new(Sink::default());
        assert!(clipboard.get().is_err());
    }

    #[test]
    fn the_register_round_trips_and_starts_empty() {
        let mut register = Register::default();
        assert!(matches!(register.get(), Err(ClipboardError::Empty)));
        register.set("Привіт").unwrap();
        assert_eq!(register.get().unwrap(), "Привіт");
    }

    #[test]
    fn a_copy_reaches_the_register_even_when_the_terminal_refuses_it() {
        struct Broken;
        impl ClipboardProvider for Broken {
            fn set(&mut self, _: &str) -> Result<(), ClipboardError> {
                Err(ClipboardError::Terminal(io::Error::other("no terminal")))
            }
            fn get(&mut self) -> Result<String, ClipboardError> {
                Err(ClipboardError::Empty)
            }
            fn name(&self) -> &'static str {
                "broken"
            }
        }

        let mut clipboard = Clipboard::new(Box::new(Broken));
        assert!(clipboard.set("text").is_err(), "the failure is reported");
        assert_eq!(
            clipboard.get().unwrap(),
            "text",
            "and pasting inside the editor still works"
        );
    }

    #[test]
    fn a_readable_outward_provider_wins_over_a_stale_register() {
        struct Elsewhere;
        impl ClipboardProvider for Elsewhere {
            fn set(&mut self, _: &str) -> Result<(), ClipboardError> {
                Ok(())
            }
            fn get(&mut self) -> Result<String, ClipboardError> {
                Ok("copied in another application".into())
            }
            fn name(&self) -> &'static str {
                "elsewhere"
            }
        }

        let mut clipboard = Clipboard::new(Box::new(Elsewhere));
        clipboard.set("mine").unwrap();
        assert_eq!(clipboard.get().unwrap(), "copied in another application");
    }

    #[test]
    fn a_bracketed_paste_becomes_what_the_next_paste_repeats() {
        let mut clipboard = Clipboard::new(Box::new(Osc52::new(Sink::default())));
        clipboard.remember("pasted by the terminal");
        assert_eq!(clipboard.get().unwrap(), "pasted by the terminal");
    }
}
