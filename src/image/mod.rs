//! Reading a PNG or a JPEG into pixels (ADR-078).
//!
//! One job: bytes in, an RGB raster and a few lines of metadata out. Nothing
//! here knows what a terminal cell is, what zoom means or how big the pane is —
//! that is `app::image`'s and `ui::image`'s work — so this module is testable
//! against a handful of bytes and has no opinion about the screen.
//!
//! Which decoder runs is decided by the file's first bytes and not by its
//! extension. A `.png` that is really a JPEG is a thing cameras and download
//! folders produce all the time, and refusing it on the strength of its name
//! would be the editor believing a label over the file.

use std::fmt;
use std::io::Cursor;

/// The most pixels the editor will decode.
///
/// 64 megapixels is above every camera and every screenshot — a 50 MP phone
/// photo fits, an 8K frame fits eight times over — and it is what stops a
/// malformed or hostile header from turning `Ctrl+O` into a 12 GB allocation.
/// The raster costs three bytes a pixel, so the cap is 192 MB of RGB.
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// The greys an alpha channel is composited over, and the size of one square.
///
/// The checkerboard every image viewer uses, so "this part is transparent"
/// reads as transparency rather than as a colour the file does not contain.
/// Eight pixels is small enough that zooming out averages it to a flat grey
/// instead of leaving a moiré over the picture.
const CHECKER: [[u8; 3]; 2] = [[0x99, 0x99, 0x99], [0x66, 0x66, 0x66]];
const CHECKER_SQUARE: u32 = 8;

/// What the file turned out to be, whatever it was called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }

    /// The format a run of bytes begins with, if it is one the editor reads.
    ///
    /// The PNG signature is eight bytes and unambiguous; a JPEG starts with
    /// `FF D8 FF`, which is the SOI marker plus the first byte of whichever
    /// marker follows it.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        None
    }
}

/// Why an image could not be shown.
#[derive(Debug)]
pub enum ImageError {
    /// The bytes are not a PNG or a JPEG, whatever the name said.
    NotAnImage,
    /// The decoder rejected them.
    Decode(String),
    /// The header asks for more pixels than `MAX_PIXELS`.
    TooLarge { pixels: u64 },
    /// A colour layout the decoder produced and this module does not convert.
    Unsupported(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnImage => write!(f, "not a PNG or a JPEG"),
            Self::Decode(why) => write!(f, "{why}"),
            Self::TooLarge { pixels } => write!(
                f,
                "{} megapixels is more than the {} the viewer decodes",
                pixels / (1024 * 1024),
                MAX_PIXELS / (1024 * 1024)
            ),
            Self::Unsupported(what) => write!(f, "{what} is a layout the viewer cannot read"),
        }
    }
}

/// A decoded picture: the pixels, and what the file said about them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// How the file stored its colour — `RGBA 8-bit`, `Grayscale 16-bit`,
    /// `Indexed 4-bit`. Read off the header before any conversion, so the
    /// metadata column describes the file rather than this module's output.
    pub colour: String,
    /// Whether the file carries transparency. What makes the checkerboard
    /// underneath the picture worth explaining rather than a mystery.
    pub has_alpha: bool,
    /// Row-major RGB, `width * height` long, alpha already composited.
    pixels: Vec<[u8; 3]>,
}

impl Image {
    /// The pixel at `(x, y)`, or black outside the picture.
    ///
    /// Clamping rather than panicking because every caller is a renderer
    /// walking a window that may hang off an edge, and a viewer that crashed
    /// when the picture was scrolled past its corner would be worse than one
    /// that drew a cell of nothing.
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0];
        }
        self.pixels[(y as usize) * (self.width as usize) + (x as usize)]
    }

    pub fn megapixels(&self) -> f64 {
        (self.width as f64) * (self.height as f64) / 1_000_000.0
    }

    /// A picture built from RGB bytes rather than from a file, for the tests
    /// of everything downstream of the decoder.
    #[cfg(test)]
    pub fn from_raw(
        format: Format,
        width: u32,
        height: u32,
        colour: String,
        has_alpha: bool,
        rgb: Vec<u8>,
    ) -> Self {
        Self {
            format,
            width,
            height,
            colour,
            has_alpha,
            pixels: from_channels(&rgb, 3, width, height, |p| [p[0], p[1], p[2]]),
        }
    }
}

/// Decodes a file the editor has already read.
///
/// Bytes rather than a path: the read is somebody else's job — for a large
/// file it is spread over frames (ADR-077) — and this way there is one decode
/// path however the bytes arrived.
pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    match Format::sniff(bytes) {
        Some(Format::Png) => decode_png(bytes),
        Some(Format::Jpeg) => decode_jpeg(bytes),
        None => Err(ImageError::NotAnImage),
    }
}

/// Whether a header's dimensions are inside the cap, before anything is
/// allocated on the strength of them.
fn check_size(width: u32, height: u32) -> Result<(), ImageError> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err(ImageError::TooLarge { pixels });
    }
    Ok(())
}

fn decode_png(bytes: &[u8]) -> Result<Image, ImageError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // `normalize_to_color8` is expand plus strip-16: a palette becomes RGB, a
    // 1-, 2- or 4-bit grey becomes 8-bit, `tRNS` becomes a real alpha channel
    // and 16-bit channels come back as 8. Doing it in the decoder rather than
    // here is what keeps this function to four output layouts instead of the
    // dozen the format allows.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|err| ImageError::Decode(err.to_string()))?;

    // The header, read before the pixels: this is the description of the file,
    // not of what the transformations above turned it into.
    let header = reader.info();
    check_size(header.width, header.height)?;
    let colour = format!(
        "{} {}-bit",
        png_colour_label(header.color_type),
        header.bit_depth as u8
    );
    let has_alpha = header.trns.is_some()
        || matches!(
            header.color_type,
            png::ColorType::Rgba | png::ColorType::GrayscaleAlpha
        );

    let mut buffer = vec![0u8; reader.output_buffer_size()];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|err| ImageError::Decode(err.to_string()))?;
    let (width, height) = (frame.width, frame.height);
    // `next_frame` fills the whole buffer including any padding past the last
    // row, so the conversion below reads only what the frame says is there.
    buffer.truncate(frame.buffer_size());

    let pixels = match frame.color_type {
        png::ColorType::Rgb => from_channels(&buffer, 3, width, height, |p| [p[0], p[1], p[2]]),
        png::ColorType::Rgba => {
            composite(&buffer, 4, width, height, |p| ([p[0], p[1], p[2]], p[3]))
        }
        png::ColorType::Grayscale => {
            from_channels(&buffer, 1, width, height, |p| [p[0], p[0], p[0]])
        }
        png::ColorType::GrayscaleAlpha => {
            composite(&buffer, 2, width, height, |p| ([p[0], p[0], p[0]], p[1]))
        }
        // `normalize_to_color8` expands a palette, so an indexed frame here
        // would mean the transformation did not run.
        other => return Err(ImageError::Unsupported(format!("{other:?}"))),
    };

    Ok(Image {
        format: Format::Png,
        width,
        height,
        colour,
        has_alpha,
        pixels,
    })
}

fn png_colour_label(colour: png::ColorType) -> &'static str {
    match colour {
        png::ColorType::Grayscale => "Grayscale",
        png::ColorType::Rgb => "RGB",
        png::ColorType::Indexed => "Indexed",
        png::ColorType::GrayscaleAlpha => "Grayscale + alpha",
        png::ColorType::Rgba => "RGBA",
    }
}

fn decode_jpeg(bytes: &[u8]) -> Result<Image, ImageError> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder
        .read_info()
        .map_err(|err| ImageError::Decode(err.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| ImageError::Decode("no frame header".into()))?;
    check_size(u32::from(info.width), u32::from(info.height))?;

    let data = decoder
        .decode()
        .map_err(|err| ImageError::Decode(err.to_string()))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));

    let pixels = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            from_channels(&data, 3, width, height, |p| [p[0], p[1], p[2]])
        }
        jpeg_decoder::PixelFormat::L8 => {
            from_channels(&data, 1, width, height, |p| [p[0], p[0], p[0]])
        }
        jpeg_decoder::PixelFormat::L16 => {
            // Two bytes a sample, little-endian; the high byte is the picture.
            from_channels(&data, 2, width, height, |p| [p[1], p[1], p[1]])
        }
        jpeg_decoder::PixelFormat::CMYK32 => from_channels(&data, 4, width, height, cmyk_to_rgb),
    };

    Ok(Image {
        format: Format::Jpeg,
        width,
        height,
        colour: jpeg_colour_label(info.pixel_format).to_string(),
        // JPEG has no alpha channel; the CMYK case has a fourth component that
        // is ink and not transparency.
        has_alpha: false,
        pixels,
    })
}

fn jpeg_colour_label(format: jpeg_decoder::PixelFormat) -> &'static str {
    match format {
        jpeg_decoder::PixelFormat::L8 => "Grayscale 8-bit",
        jpeg_decoder::PixelFormat::L16 => "Grayscale 16-bit",
        jpeg_decoder::PixelFormat::RGB24 => "YCbCr 8-bit",
        jpeg_decoder::PixelFormat::CMYK32 => "CMYK 8-bit",
    }
}

/// Adobe's inverted CMYK, which is the only CMYK a JPEG in the wild carries.
///
/// The four bytes are stored complemented, so the ink of each channel is
/// `255 - byte`, and the naive conversion below is the one every viewer uses:
/// a real one would need the file's ICC profile, and a picture the user opened
/// to look at is better shown approximately than refused.
fn cmyk_to_rgb(p: &[u8]) -> [u8; 3] {
    let k = p[3] as u32;
    [
        ((p[0] as u32 * k) / 255) as u8,
        ((p[1] as u32 * k) / 255) as u8,
        ((p[2] as u32 * k) / 255) as u8,
    ]
}

/// Packs interleaved samples into RGB, one call of `convert` per pixel.
fn from_channels(
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    convert: impl Fn(&[u8]) -> [u8; 3],
) -> Vec<[u8; 3]> {
    let count = (width as usize) * (height as usize);
    let mut out = Vec::with_capacity(count);
    out.extend(data.chunks_exact(stride).take(count).map(&convert));
    // A truncated file decodes to fewer rows than the header promised; the
    // rest is black rather than a panic, and the picture shows how far the
    // data got.
    out.resize(count, [0, 0, 0]);
    out
}

/// The same, for a layout with an alpha channel: every pixel is mixed onto the
/// checkerboard before it is stored.
fn composite(
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    split: impl Fn(&[u8]) -> ([u8; 3], u8),
) -> Vec<[u8; 3]> {
    let count = (width as usize) * (height as usize);
    let mut out = Vec::with_capacity(count);
    for (index, chunk) in data.chunks_exact(stride).take(count).enumerate() {
        let (rgb, alpha) = split(chunk);
        let x = (index % width as usize) as u32;
        let y = (index / width as usize) as u32;
        out.push(over(rgb, alpha, checker(x, y)));
    }
    out.resize(count, [0, 0, 0]);
    out
}

/// Which of the two greys the square at `(x, y)` is.
fn checker(x: u32, y: u32) -> [u8; 3] {
    let square = (x / CHECKER_SQUARE + y / CHECKER_SQUARE) % 2;
    CHECKER[square as usize]
}

/// `src` at `alpha` over `dst`, in eight-bit sRGB.
///
/// The arithmetic is deliberately the naive one — no gamma — because it is what
/// the browsers, the file managers and the image viewers the user is comparing
/// against do, and a viewer that composited "correctly" would be the one that
/// looked wrong.
fn over(src: [u8; 3], alpha: u8, dst: [u8; 3]) -> [u8; 3] {
    let a = alpha as u32;
    let mut out = [0u8; 3];
    for i in 0..3 {
        out[i] = ((src[i] as u32 * a + dst[i] as u32 * (255 - a)) / 255) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-by-one PNG of a known colour, built by hand so the tests do not
    /// need an encoder in the dependency tree.
    fn png_1x1(rgb: [u8; 3]) -> Vec<u8> {
        use flate2::write::ZlibEncoder;
        use std::io::Write as _;

        fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            let mut covered = kind.to_vec();
            covered.extend_from_slice(body);
            out.extend_from_slice(&crc32(&covered).to_be_bytes());
            out
        }

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        // 8 bits, colour type 2 (RGB), deflate, no filter, no interlace.
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);

        let mut raw = vec![0u8];
        raw.extend_from_slice(&rgb);
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&raw).unwrap();
        let idat = encoder.finish().unwrap();

        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &ihdr));
        out.extend(chunk(b"IDAT", &idat));
        out.extend(chunk(b"IEND", &[]));
        out
    }

    /// The PNG chunk CRC, written out rather than taken from a crate: the
    /// fixture above is the only caller in the tree.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for byte in bytes {
            c ^= u32::from(*byte);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        c ^ 0xFFFF_FFFF
    }

    #[test]
    fn the_format_comes_from_the_bytes_and_not_from_the_name() {
        assert_eq!(Format::sniff(b"\x89PNG\r\n\x1a\nrest"), Some(Format::Png));
        assert_eq!(Format::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(Format::sniff(b"GIF89a"), None);
        assert_eq!(Format::sniff(b""), None);
    }

    #[test]
    fn a_png_decodes_to_the_colour_it_holds() {
        let image = decode(&png_1x1([10, 200, 30])).unwrap();
        assert_eq!(image.format, Format::Png);
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixel(0, 0), [10, 200, 30]);
        assert_eq!(image.colour, "RGB 8-bit");
        assert!(!image.has_alpha);
    }

    #[test]
    fn a_pixel_outside_the_picture_is_black_rather_than_a_panic() {
        let image = decode(&png_1x1([255, 255, 255])).unwrap();
        assert_eq!(image.pixel(9, 9), [0, 0, 0]);
    }

    #[test]
    fn something_that_is_neither_format_is_refused_by_its_bytes() {
        assert!(matches!(
            decode(b"# not an image\n"),
            Err(ImageError::NotAnImage)
        ));
    }

    #[test]
    fn a_header_larger_than_the_cap_is_refused_before_anything_is_allocated() {
        assert!(matches!(
            check_size(40_000, 40_000),
            Err(ImageError::TooLarge { .. })
        ));
        assert!(check_size(8_000, 8_000).is_ok());
    }

    /// Transparent pixels take the checkerboard's colour, opaque ones their
    /// own, and half-transparent ones land between the two.
    #[test]
    fn alpha_is_composited_onto_the_checkerboard() {
        assert_eq!(over([255, 0, 0], 255, CHECKER[0]), [255, 0, 0]);
        assert_eq!(over([255, 0, 0], 0, CHECKER[1]), CHECKER[1]);
        let half = over([255, 255, 255], 128, [0, 0, 0]);
        assert_eq!(half, [128, 128, 128]);
    }

    #[test]
    fn the_checkerboard_alternates_every_eight_pixels() {
        assert_eq!(checker(0, 0), CHECKER[0]);
        assert_eq!(checker(8, 0), CHECKER[1]);
        assert_eq!(checker(8, 8), CHECKER[0]);
    }

    /// Adobe stores CMYK complemented, so an all-`FF` pixel is white and not
    /// the black the raw numbers suggest.
    #[test]
    fn adobe_cmyk_reads_as_the_colour_a_viewer_shows() {
        assert_eq!(cmyk_to_rgb(&[255, 255, 255, 255]), [255, 255, 255]);
        assert_eq!(cmyk_to_rgb(&[255, 255, 255, 0]), [0, 0, 0]);
    }

    /// A file whose pixel data stops early still opens; the rows that did not
    /// arrive are black, which is a picture that shows how far the data got.
    #[test]
    fn a_short_run_of_samples_is_padded_rather_than_dropped() {
        let pixels = from_channels(&[1, 2, 3], 3, 2, 1, |p| [p[0], p[1], p[2]]);
        assert_eq!(pixels, vec![[1, 2, 3], [0, 0, 0]]);
    }
}
