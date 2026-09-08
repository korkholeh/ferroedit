//! How a file's bytes become text, and text becomes bytes again (ADR-059).
//!
//! SPEC §17 makes UTF-8 the default and leaves legacy encodings out of the MVP.
//! They are here now because the status bar's `UTF-8` became a question
//! (ADR-058) and a question with one answer is not one. What a `Charset` adds
//! over `encoding_rs`'s own `Encoding` is the byte order mark: `UTF-8` and
//! `UTF-8 with BOM` are the same decoder and two different files, and which one
//! a file is has to survive a round trip.
//!
//! Two rules the Encoding Standard has and this module does not. Unmappable
//! characters do *not* become `&#1071;` here — that is right for a web form and
//! is silent corruption in an editor, so a save that cannot hold what is in the
//! buffer fails and says which character (`Document::save`). And UTF-16 is a
//! real output encoding here: the standard says every encoder is UTF-8 for it,
//! which would write a file that is not what its own label says.

use encoding_rs::Encoding;

/// The byte order mark a charset writes, and looks for when reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bom {
    /// Never written, and never expected — every legacy charset.
    None,
    /// `EF BB BF`.
    Utf8,
    /// `FF FE`.
    Utf16Le,
    /// `FE FF`.
    Utf16Be,
}

impl Bom {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::None => &[],
            Self::Utf8 => &[0xEF, 0xBB, 0xBF],
            Self::Utf16Le => &[0xFF, 0xFE],
            Self::Utf16Be => &[0xFE, 0xFF],
        }
    }
}

/// How one file is read and written: a decoder, and whether it carries a mark.
///
/// A value rather than a reference to a table entry, so a document owns its
/// answer and nothing has to be looked up to write a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charset {
    /// What the status bar and the picker call it. It is also the identity: the
    /// command a picker row carries is this string, because a `Command` that
    /// held a `&'static Encoding` could not be compared or logged usefully.
    pub label: &'static str,
    encoding: &'static Encoding,
    bom: Bom,
}

impl Default for Charset {
    fn default() -> Self {
        Self::UTF8
    }
}

impl Charset {
    /// What a new buffer is, and what a file with no mark and valid UTF-8 bytes
    /// is read as (SPEC §17).
    pub const UTF8: Self = Self {
        label: "UTF-8",
        encoding: encoding_rs::UTF_8,
        bom: Bom::None,
    };

    /// What a file with bytes no charset could be guessed from is read as.
    ///
    /// Every byte maps to a character and back, so a file opened this way by
    /// accident still saves byte for byte — which is what makes guessing here
    /// safe where guessing between two Cyrillic charsets would not be.
    pub const FALLBACK: Self = Self {
        label: "Windows-1252",
        encoding: encoding_rs::WINDOWS_1252,
        bom: Bom::None,
    };

    /// Every charset the picker offers, in the order it offers them: the
    /// Unicode ones first, then the legacy ones grouped by the writing system
    /// they are for.
    ///
    /// Curated rather than every label the Encoding Standard defines. A picker
    /// is a list a person reads, and forty rows of aliases for the same four
    /// tables is not a list — these are the ones files actually arrive in.
    pub const ALL: &'static [Self] = &[
        Self::UTF8,
        Self {
            label: "UTF-8 with BOM",
            encoding: encoding_rs::UTF_8,
            bom: Bom::Utf8,
        },
        Self {
            label: "UTF-16 LE",
            encoding: encoding_rs::UTF_16LE,
            bom: Bom::Utf16Le,
        },
        Self {
            label: "UTF-16 BE",
            encoding: encoding_rs::UTF_16BE,
            bom: Bom::Utf16Be,
        },
        Self::FALLBACK,
        Self::legacy("Windows-1250", encoding_rs::WINDOWS_1250),
        Self::legacy("Windows-1251", encoding_rs::WINDOWS_1251),
        Self::legacy("Windows-1253", encoding_rs::WINDOWS_1253),
        Self::legacy("Windows-1254", encoding_rs::WINDOWS_1254),
        Self::legacy("Windows-1257", encoding_rs::WINDOWS_1257),
        Self::legacy("ISO-8859-1", encoding_rs::WINDOWS_1252),
        Self::legacy("ISO-8859-2", encoding_rs::ISO_8859_2),
        Self::legacy("ISO-8859-5", encoding_rs::ISO_8859_5),
        Self::legacy("ISO-8859-7", encoding_rs::ISO_8859_7),
        Self::legacy("ISO-8859-15", encoding_rs::ISO_8859_15),
        Self::legacy("KOI8-U", encoding_rs::KOI8_U),
        Self::legacy("KOI8-R", encoding_rs::KOI8_R),
        Self::legacy("IBM866", encoding_rs::IBM866),
        Self::legacy("Macintosh", encoding_rs::MACINTOSH),
        Self::legacy("Shift_JIS", encoding_rs::SHIFT_JIS),
        Self::legacy("EUC-JP", encoding_rs::EUC_JP),
        Self::legacy("EUC-KR", encoding_rs::EUC_KR),
        Self::legacy("GBK", encoding_rs::GBK),
        Self::legacy("GB18030", encoding_rs::GB18030),
        Self::legacy("Big5", encoding_rs::BIG5),
    ];

    const fn legacy(label: &'static str, encoding: &'static Encoding) -> Self {
        Self {
            label,
            encoding,
            bom: Bom::None,
        }
    }

    /// The charset the picker's row with this label chooses.
    ///
    /// `None` for a label no row carries, which is what a stale command from a
    /// dialog that has since changed would be.
    pub fn by_label(label: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|set| set.label == label)
    }

    /// Whether writing this charset can fail on a character the buffer holds.
    ///
    /// UTF-8 and UTF-16 hold everything; every legacy charset holds a few
    /// hundred characters and nothing else, which is what `Document::save`
    /// checks for before it opens the file.
    pub fn is_unicode(self) -> bool {
        matches!(self.bom, Bom::Utf16Le | Bom::Utf16Be) || self.encoding == encoding_rs::UTF_8
    }

    /// What a file starts with, when it starts with anything.
    pub fn bom_bytes(self) -> &'static [u8] {
        self.bom.bytes()
    }

    /// The charset a file's own first bytes announce, if any.
    ///
    /// Only a mark counts. Guessing a legacy charset from the shape of the
    /// bytes is a coin flip between Windows-1251 and KOI8-U that no heuristic
    /// wins reliably, and a wrong guess is a file the user then saves back
    /// mangled — so the answer for unmarked bytes is UTF-8 when they are valid
    /// UTF-8 and `FALLBACK` when they are not, both of which round-trip.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        for set in Self::ALL {
            let mark = set.bom.bytes();
            if !mark.is_empty() && bytes.starts_with(mark) {
                return Some(*set);
            }
        }
        None
    }

    /// Decodes a whole file, with the mark — which is not text — removed first.
    ///
    /// Malformed bytes become `U+FFFD` rather than an error: the charset was
    /// either sniffed from the file's own mark or named by the user, and a file
    /// that is *nearly* right is one the user needs to see in order to choose
    /// again.
    pub fn decode(self, bytes: &[u8]) -> String {
        let body = bytes.strip_prefix(self.bom.bytes()).unwrap_or(bytes);
        // `decode_without_bom_handling` is the one that does not re-sniff: the
        // mark has been taken off already, and a second opinion here would
        // override the charset the user just chose.
        self.encoding
            .decode_without_bom_handling(body)
            .0
            .into_owned()
    }

    /// Encodes one piece of text, appending to `out`.
    ///
    /// Returns whether anything in it could not be represented. The caller
    /// decides what that means — `Document::save` refuses, because an editor
    /// that writes `&#1071;` where the user typed `Я` has corrupted the file it
    /// was asked to keep.
    pub fn encode(self, text: &str, out: &mut Vec<u8>) -> bool {
        match self.bom {
            // Hand-rolled, because the Encoding Standard defines no UTF-16
            // *encoder* — `new_encoder()` on it yields a UTF-8 one, which would
            // write a file that is not what its own name says.
            Bom::Utf16Le => {
                out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
                false
            }
            Bom::Utf16Be => {
                out.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
                false
            }
            _ if self.encoding == encoding_rs::UTF_8 => {
                out.extend_from_slice(text.as_bytes());
                false
            }
            _ => {
                let (bytes, _, unmappable) = self.encoding.encode(text);
                out.extend_from_slice(&bytes);
                unmappable
            }
        }
    }

    /// The first character of `text` this charset cannot hold, for the message
    /// a refused save shows. `None` when it can hold all of it.
    ///
    /// One character at a time, which is slow and is only ever run once a save
    /// has already been refused — the fast answer is `encode`'s own flag.
    pub fn first_unmappable(self, text: &str) -> Option<char> {
        let mut scratch = Vec::new();
        text.chars().find(|ch| {
            scratch.clear();
            self.encode(ch.encode_utf8(&mut [0; 4]), &mut scratch)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_of_the_picker_has_a_label_of_its_own() {
        let mut labels: Vec<&str> = Charset::ALL.iter().map(|set| set.label).collect();
        let count = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), count);
        for label in &labels {
            assert!(Charset::by_label(label).is_some(), "{label}");
        }
    }

    /// The two UTF-8 rows are the same decoder and two different files, which
    /// is the whole reason a `Charset` is not just an `Encoding`.
    #[test]
    fn utf8_with_a_mark_and_without_are_different_charsets() {
        let with = Charset::by_label("UTF-8 with BOM").unwrap();
        assert_ne!(with, Charset::UTF8);
        assert_eq!(with.bom_bytes(), [0xEF, 0xBB, 0xBF]);
        assert!(Charset::UTF8.bom_bytes().is_empty());
    }

    #[test]
    fn a_mark_is_what_a_file_is_recognised_by_and_it_is_not_text() {
        let bytes = [0xFF, 0xFE, b'h', 0, b'i', 0];
        let charset = Charset::sniff(&bytes).expect("a UTF-16 LE mark");
        assert_eq!(charset.label, "UTF-16 LE");
        assert_eq!(charset.decode(&bytes), "hi");

        assert_eq!(Charset::sniff(b"plain text"), None);
    }

    /// Round-tripping is the property that matters: what was read is what is
    /// written back, mark and all.
    #[test]
    fn every_charset_round_trips_text_it_can_hold() {
        for set in Charset::ALL {
            let mut bytes = set.bom_bytes().to_vec();
            assert!(!set.encode("ok\n", &mut bytes), "{}", set.label);
            assert_eq!(set.decode(&bytes), "ok\n", "{}", set.label);
        }
    }

    #[test]
    fn cyrillic_round_trips_through_the_charsets_that_have_it() {
        for label in ["UTF-8", "UTF-16 LE", "UTF-16 BE", "Windows-1251", "KOI8-U"] {
            let set = Charset::by_label(label).unwrap();
            let mut bytes = set.bom_bytes().to_vec();
            assert!(!set.encode("Привіт", &mut bytes), "{label}");
            assert_eq!(set.decode(&bytes), "Привіт", "{label}");
        }
    }

    /// The Encoding Standard would write `&#1071;` here. An editor must not:
    /// the save is refused instead, and this is the flag that refuses it.
    #[test]
    fn a_character_a_legacy_charset_cannot_hold_is_reported_and_not_escaped() {
        let latin = Charset::by_label("Windows-1252").unwrap();
        let mut bytes = Vec::new();
        assert!(latin.encode("Я", &mut bytes));
        assert_eq!(latin.first_unmappable("hello Я world"), Some('Я'));
        assert_eq!(latin.first_unmappable("hello world"), None);
    }

    /// Any byte at all reads and writes back unchanged, which is what makes it
    /// safe to open a file nothing could be guessed about.
    #[test]
    fn the_fallback_round_trips_every_byte() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        let text = Charset::FALLBACK.decode(&bytes);
        let mut back = Vec::new();
        assert!(!Charset::FALLBACK.encode(&text, &mut back));
        assert_eq!(back, bytes);
    }

    #[test]
    fn only_the_unicode_charsets_hold_everything() {
        for label in ["UTF-8", "UTF-8 with BOM", "UTF-16 LE", "UTF-16 BE"] {
            assert!(Charset::by_label(label).unwrap().is_unicode(), "{label}");
        }
        for label in ["Windows-1251", "Shift_JIS", "Big5"] {
            assert!(!Charset::by_label(label).unwrap().is_unicode(), "{label}");
        }
    }
}
