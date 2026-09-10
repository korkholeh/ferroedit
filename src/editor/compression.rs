//! Files that arrive compressed, and the text inside them (ADR-074).
//!
//! One format, gzip, and one direction: a `.gz` is unpacked on the way in and
//! never packed on the way out. The point is the peek a `dump.sql.gz` is
//! otherwise two commands and a temporary file away from — not a second way to
//! write a file, which would mean choosing a compression level and a timestamp
//! on the user's behalf every time they pressed `Ctrl+S`.
//!
//! What comes out is bytes, not text. Unpacking sits *before* `Charset` in the
//! read: the container says nothing about what encoding is inside it, so a
//! gzipped Windows-1251 file is decoded exactly as the plain one beside it
//! would be (ADR-059), and a gzipped tarball still fails the NUL guard and is
//! refused as binary.

use std::io::Read;

use flate2::read::MultiGzDecoder;

/// The most text one compressed file is unpacked into.
///
/// A gzip stream does not say how long it is, and the ratios that make the
/// format worth using are exactly what makes reading one unbounded a bad idea:
/// eight megabytes of `dump.sql.gz` is comfortably a gigabyte of SQL, and a
/// stream built to be a bomb is more. Sixty-four megabytes is past what SPEC
/// §44 asks the editor to stay comfortable with and small enough to hold twice
/// while the rope is built; a file with more in it opens on its first 64 MB and
/// says so, which is what someone opening a dump to look at it wanted anyway.
pub const MAX_UNPACKED: usize = 64 * 1024 * 1024;

/// How a file's bytes reached the decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    /// The file is its own contents — every file the editor opened before this.
    #[default]
    None,
    /// A gzip stream, possibly several concatenated.
    Gzip,
}

impl Compression {
    /// What the status bar calls it, or `None` when there is nothing to say.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Gzip => Some("gzip"),
        }
    }

    /// What the file's first bytes announce.
    ///
    /// The magic number and not the extension, unlike the CSV view's rule
    /// (ADR-062). The two questions are different: a `.csv` that is read as a
    /// table is still readable as text if the guess was wrong, while a gzip
    /// stream read as text is refused as binary — so the name is the less
    /// reliable of the two answers here, and being wrong about it is not a
    /// view the user can toggle back.
    pub fn sniff(bytes: &[u8]) -> Self {
        match bytes {
            [0x1F, 0x8B, ..] => Self::Gzip,
            _ => Self::None,
        }
    }
}

/// What one unpacked file is: its bytes, and whether the cap cut them short.
pub struct Unpacked {
    pub bytes: Vec<u8>,
    pub compression: Compression,
    /// `true` when the stream had more in it than `MAX_UNPACKED`.
    pub truncated: bool,
}

/// Unpacks `bytes` when they are a container, and hands them back untouched
/// when they are not.
///
/// A stream that is cut at the cap is cut back further, to the last line break
/// in what was read: the alternative is a last line that stops mid-record, and
/// half a row of a dump reads as data rather than as the edge of the window.
pub fn unpack(bytes: Vec<u8>) -> std::io::Result<Unpacked> {
    unpack_capped(bytes, MAX_UNPACKED)
}

/// The same with the cap named, which is what the tests use: building sixty-four
/// megabytes of anything to watch the guard fire is a slow way to ask a question
/// the guard answers the same way at sixty-four bytes.
fn unpack_capped(bytes: Vec<u8>, cap: usize) -> std::io::Result<Unpacked> {
    let compression = Compression::sniff(&bytes);
    if compression == Compression::None {
        return Ok(Unpacked {
            bytes,
            compression,
            truncated: false,
        });
    }
    // One byte past the cap, so that "there was more" and "it ended here" are
    // distinguishable without asking the decoder anything.
    let mut out = Vec::new();
    MultiGzDecoder::new(&bytes[..])
        .take(cap as u64 + 1)
        .read_to_end(&mut out)?;
    let truncated = out.len() > cap;
    if truncated {
        out.truncate(cap);
        if let Some(at) = out.iter().rposition(|byte| *byte == b'\n') {
            out.truncate(at + 1);
        }
    }
    Ok(Unpacked {
        bytes: out,
        compression,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::write::GzEncoder;
    use flate2::Compression as Level;

    use super::*;

    fn gzipped(text: &str) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Level::fast());
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn plain_bytes_come_back_as_they_went_in() {
        let unpacked = unpack(b"select 1;\n".to_vec()).unwrap();
        assert_eq!(unpacked.compression, Compression::None);
        assert_eq!(unpacked.bytes, b"select 1;\n");
        assert!(!unpacked.truncated);
    }

    #[test]
    fn a_gzip_stream_is_unpacked() {
        let unpacked = unpack(gzipped("select 1;\n")).unwrap();
        assert_eq!(unpacked.compression, Compression::Gzip);
        assert_eq!(unpacked.bytes, b"select 1;\n");
        assert!(!unpacked.truncated);
    }

    /// `cat a.gz b.gz` is a valid `.gz`, and the single-member decoder would
    /// stop at the end of the first one without saying anything.
    #[test]
    fn concatenated_members_are_all_read() {
        let mut bytes = gzipped("first\n");
        bytes.extend(gzipped("second\n"));
        let unpacked = unpack(bytes).unwrap();
        assert_eq!(unpacked.bytes, b"first\nsecond\n");
    }

    #[test]
    fn a_stream_past_the_cap_is_cut_at_a_line_break() {
        let text = "0123456789\n".repeat(10);
        let unpacked = unpack_capped(gzipped(&text), 35).unwrap();
        assert!(unpacked.truncated);
        assert_eq!(unpacked.bytes, b"0123456789\n0123456789\n0123456789\n");
    }

    /// The guard is on the size of what comes *out*, so a stream of nothing but
    /// zeroes — the shape of a bomb, and a file with no line break in it — is
    /// stopped by the same rule a real file is, and cut at the cap itself when
    /// there is no break to cut back to.
    #[test]
    fn a_bomb_is_stopped_by_the_cap() {
        let unpacked = unpack_capped(gzipped(&"\0".repeat(1 << 20)), 64).unwrap();
        assert!(unpacked.truncated);
        assert_eq!(unpacked.bytes.len(), 64);
    }

    #[test]
    fn a_truncated_gzip_stream_is_an_error() {
        let mut bytes = gzipped("select 1;\n");
        bytes.truncate(bytes.len() - 4);
        assert!(unpack(bytes).is_err());
    }

    #[test]
    fn only_gzip_is_recognised() {
        assert_eq!(Compression::sniff(&[0x1F, 0x8B, 0x08]), Compression::Gzip);
        assert_eq!(Compression::sniff(&[0x50, 0x4B, 0x03]), Compression::None);
        assert_eq!(Compression::sniff(&[0x1F]), Compression::None);
        assert_eq!(Compression::sniff(&[]), Compression::None);
    }
}
