//! A large file being read a slice at a time (ADR-077).
//!
//! Everything below the threshold still opens in one go inside the command
//! that asked for it. Above it, the read is cut into slices the run loop takes
//! one per frame, so the window keeps drawing — and can say how far it has got
//! — instead of going still for the length of the file.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::Instant;

use crate::editor::document::DiskStamp;

/// Files this size and over are read a slice at a time.
///
/// Four megabytes is where a read stops being certain to fit in a frame: on the
/// machine this was measured on a warm cache reads about 4 GB/s and a cold one
/// a tenth of that, so 4 MB is roughly one frame's worth of the slow case. A
/// file under it opens the way it always did, with no state to carry and no box
/// to flicker on screen.
pub const SLICED_ABOVE: u64 = 4 * 1024 * 1024;

/// How long one frame may spend reading, matching the search walk's budget
/// (ADR-076): half a 16 ms frame, so the other half still pays for the draw.
const READ_BUDGET: Duration = Duration::from_millis(8);

/// How much is asked of the filesystem in one call.
///
/// Big enough that a fast disk is not held back by the loop around it, small
/// enough that the deadline below is checked often on a slow one — a network
/// share that answers at 5 MB/s still stops for the clock five times a second.
const CHUNK: usize = 1024 * 1024;

/// The most that is reserved up front, however long the file says it is.
const RESERVE_CAP: u64 = 256 * 1024 * 1024;

/// The spinner drawn while a read is spread over frames. The find bar's, for
/// the reason it is the find bar's: every frame of braille is one cell wide.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A read in flight: the file, what has come out of it so far, and what the
/// tab is to be when it lands.
#[derive(Debug)]
pub struct Opening {
    /// The absolute path, which is what the finished tab is matched by.
    path: PathBuf,
    /// The line the open was asked to land on, if it was asked for one.
    line: Option<usize>,
    file: File,
    /// The stamp taken before the first byte was read, held for the document
    /// that will be built from these bytes: it belongs to the moment the read
    /// started, not to the moment it finished (ADR-043).
    disk: Option<DiskStamp>,
    /// What the file said it was when the read began. A file that grew under
    /// the read is still read to its end; the number only drives the readout.
    total: u64,
    bytes: Vec<u8>,
    /// Which frame of the spinner to draw. Advanced once per slice, so a read
    /// that finishes inside one frame never draws one.
    tick: usize,
    started: Instant,
}

impl Opening {
    /// Opens the file and takes its stamp, ready for the first slice.
    ///
    /// Nothing is read here: the frame that asks for the open is the frame that
    /// puts the box on screen, and the first slice is the next one's work.
    pub fn start(path: &Path, line: Option<usize>) -> io::Result<Self> {
        let disk = DiskStamp::of(path);
        let file = File::open(path)?;
        let total = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_path_buf(),
            line,
            file,
            disk,
            total,
            // The whole file is going to be here, so the room for it is asked
            // for once rather than grown a megabyte at a time — up to a cap,
            // because a length read off a `/proc` file or a device is not a
            // promise, and reserving on the strength of one is how a mistyped
            // path becomes an allocation failure.
            bytes: Vec::with_capacity(total.min(RESERVE_CAP) as usize),
            tick: 0,
            started: Instant::now(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn line(&self) -> Option<usize> {
        self.line
    }

    /// Reads until the budget runs out or the file ends. `true` means the file
    /// is all here.
    pub fn read_slice(&mut self) -> io::Result<bool> {
        let deadline = Instant::now() + READ_BUDGET;
        loop {
            let start = self.bytes.len();
            self.bytes.resize(start + CHUNK, 0);
            let read = match self.file.read(&mut self.bytes[start..]) {
                Ok(read) => read,
                Err(err) => {
                    self.bytes.truncate(start);
                    return Err(err);
                }
            };
            self.bytes.truncate(start + read);
            if read == 0 {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                self.tick += 1;
                return Ok(false);
            }
        }
    }

    /// Reads the rest of the file with no deadline, for the caller that cannot
    /// wait for frames — a second open asked for while this one is in flight.
    pub fn read_rest(&mut self) -> io::Result<()> {
        while !self.read_slice()? {}
        Ok(())
    }

    /// The bytes, and the stamp they are to be dated by.
    pub fn finish(self) -> (PathBuf, Vec<u8>, Option<DiskStamp>) {
        log::info!(
            "read {} ({} bytes) in {:?} over {} slices",
            self.path.display(),
            self.bytes.len(),
            self.started.elapsed(),
            self.tick + 1
        );
        (self.path, self.bytes, self.disk)
    }

    /// The spinner, the file's name and how far the read has got — the whole of
    /// what the box says.
    ///
    /// The percentage is left out when the file's length is unknown, which is
    /// the one case where a number would be invented rather than measured.
    pub fn label(&self) -> String {
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file");
        let spinner = SPINNER[self.tick % SPINNER.len()];
        match self.percent() {
            Some(percent) => format!(
                "{spinner} Opening {name} — {percent}% of {}",
                size_label(self.total)
            ),
            None => format!("{spinner} Opening {name}"),
        }
    }

    /// How much of the file is in hand, 0 to 100.
    pub fn percent(&self) -> Option<u16> {
        if self.total == 0 {
            return None;
        }
        let done = (self.bytes.len() as u64).min(self.total);
        Some((done * 100 / self.total) as u16)
    }
}

/// A byte count as a person reads one: `189 MB`, `1.4 GB`.
///
/// Powers of two under names that say ten, which is what every file manager on
/// every platform the editor runs on shows — being right about the base here
/// would only make the editor disagree with the tool the user checked in.
fn size_label(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{} KB", bytes / KB)
    } else if bytes < GB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big_file(dir: &std::path::Path, name: &str, lines: usize) -> PathBuf {
        let path = dir.join(name);
        let body = "the quick brown fox jumps over the lazy dog\n".repeat(lines);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn a_read_ends_with_every_byte_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = big_file(dir.path(), "big.txt", 200_000);
        let on_disk = std::fs::read(&path).unwrap();

        let mut opening = Opening::start(&path, None).unwrap();
        opening.read_rest().unwrap();
        let (read_path, bytes, disk) = opening.finish();

        assert_eq!(read_path, path);
        assert_eq!(bytes, on_disk);
        assert!(disk.is_some(), "the stamp is taken before the first slice");
    }

    #[test]
    fn nothing_is_read_before_the_first_slice() {
        let dir = tempfile::tempdir().unwrap();
        let path = big_file(dir.path(), "big.txt", 1_000);
        let opening = Opening::start(&path, Some(12)).unwrap();
        assert_eq!(opening.percent(), Some(0));
        assert_eq!(opening.line(), Some(12));
    }

    #[test]
    fn the_readout_names_the_file_and_says_how_far_it_has_got() {
        let dir = tempfile::tempdir().unwrap();
        let path = big_file(dir.path(), "big.txt", 200_000);
        let mut opening = Opening::start(&path, None).unwrap();
        opening.read_rest().unwrap();

        let label = opening.label();
        assert!(label.contains("Opening big.txt"), "{label}");
        assert!(label.contains("100%"), "{label}");
        assert_eq!(opening.percent(), Some(100));
    }

    #[test]
    fn an_empty_file_offers_no_percentage_to_lie_with() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.txt");
        std::fs::write(&path, "").unwrap();
        let opening = Opening::start(&path, None).unwrap();
        assert_eq!(opening.percent(), None);
        assert!(!opening.label().contains('%'), "{}", opening.label());
    }

    #[test]
    fn sizes_read_the_way_a_file_manager_shows_them() {
        assert_eq!(size_label(512), "512 B");
        assert_eq!(size_label(4 * 1024), "4 KB");
        assert_eq!(size_label(189 * 1024 * 1024), "189 MB");
        assert_eq!(size_label(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }
}
