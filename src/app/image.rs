//! The image viewer's state: one picture, where it is being looked at from,
//! and the grid of coloured blocks that is drawn for it (ADR-078).
//!
//! Read-only, like the diff and the log beside it, and a tab for the same
//! reason: a picture is something a user opens, keeps, switches away from and
//! closes. What it adds is a *window over a raster* — a zoom and a pan — which
//! is the same shape the editor's viewport has, in pixels instead of lines.
//!
//! The block grid is cached here rather than computed by the renderer, for the
//! reason the highlighter's colours are: `ui/` is read-only over `&App`
//! (ARCHITECTURE §1), and averaging a twelve-megapixel photo down to a pane
//! forty rows tall is work that must not be repeated on a frame where nothing
//! moved.

use std::path::{Path, PathBuf};

use crate::image::Image;

/// The zoom steps, as image pixels per half-cell.
///
/// Steps rather than a continuous factor so that zooming in and back out lands
/// exactly where it started, and so `1:1` is a stop the user passes through
/// rather than a value they have to hit. The ratios are the ones every image
/// viewer offers, which makes the readout beside them (`50%`, `200%`) mean what
/// it means everywhere else.
pub const LEVELS: &[f64] = &[
    1.0 / 16.0,
    1.0 / 12.0,
    1.0 / 8.0,
    1.0 / 6.0,
    1.0 / 4.0,
    1.0 / 3.0,
    1.0 / 2.0,
    2.0 / 3.0,
    1.0,
    1.5,
    2.0,
    3.0,
    4.0,
    6.0,
    8.0,
    12.0,
    16.0,
];

/// The index of `1.0` in `LEVELS`: one image pixel per half-cell, which is what
/// `1` on the keyboard goes to.
pub const ACTUAL: usize = 8;

/// How much of the visible span one arrow key moves.
///
/// An eighth, so eight presses cross the pane: enough that panning a large
/// photo is not a chore, small enough that the eye keeps its place.
const PAN_FRACTION: f64 = 1.0 / 8.0;

/// The most pixels one half-cell averages over, per axis.
///
/// A cap and not a rule: a box smaller than this is averaged whole. It is what
/// keeps a frame's cost proportional to the *pane* rather than to the picture,
/// so panning a 64-megapixel photo costs what panning a 3-megapixel one does.
const SAMPLES: u32 = 8;

/// The width of the metadata column, in cells.
///
/// Fixed rather than proportional: it holds a dozen short labelled values, and
/// a column that grew with the terminal would only put more space between a
/// label and its number.
pub const META_WIDTH: u16 = 26;

/// The narrowest picture the metadata column will leave room for. Under this
/// the column is dropped for the frame, whatever the toggle says: a viewer that
/// gave twenty-six columns to labels and eight to the photograph would be
/// showing the wrong thing.
const MIN_CANVAS: u16 = 24;

/// The pane the picture is drawn into, in terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Canvas {
    pub width: u16,
    pub height: u16,
}

impl Canvas {
    /// Rows of pixels the pane can show. Two per cell: every cell is drawn as
    /// an upper half block, so its foreground is one row and its background is
    /// the next (ADR-078).
    pub fn rows(self) -> u16 {
        self.height.saturating_mul(2)
    }

    fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// One terminal cell of the picture: the pixel above the dividing line and the
/// pixel below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Block {
    pub top: [u8; 3],
    pub bottom: [u8; 3],
}

/// What the cached grid was built for. A frame whose key still matches redraws
/// from the cache and samples nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CacheKey {
    canvas: Canvas,
    scale: f64,
    origin: (f64, f64),
}

/// How the picture is scaled: to the pane, or to one of the fixed steps.
///
/// `Fit` is a *rule* and not a number, so a resized terminal re-fits the
/// picture instead of leaving it at whatever the old pane happened to need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    Fit,
    Level(usize),
}

#[derive(Debug)]
pub struct ImageState {
    /// The absolute path, which is what re-opening the file is matched by.
    path: PathBuf,
    /// What the tab strip calls it.
    title: String,
    /// The file's size on disk, for the metadata column.
    file_bytes: u64,
    pub image: Image,
    pub zoom: Zoom,
    /// Whether the metadata column is drawn.
    pub show_meta: bool,
    /// The image pixel at the pane's top-left corner. Fractional, because a
    /// zoomed-out pane advances by less than a pixel per cell and rounding
    /// every pan to whole pixels would make a slow drag stutter.
    origin: (f64, f64),
    /// Where a drag started: the pointer's cell and the origin under it.
    /// `None` unless a button is down.
    grab: Option<((u16, u16), (f64, f64))>,
    canvas: Canvas,
    /// The scale the last sync resolved `zoom` to, in pixels per half-cell.
    scale: f64,
    blocks: Vec<Block>,
    key: Option<CacheKey>,
}

impl ImageState {
    pub fn new(path: &Path, image: Image, file_bytes: u64) -> Self {
        let title = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image")
            .to_string();
        Self {
            path: path.to_path_buf(),
            title,
            file_bytes,
            image,
            // A picture opens whole. Somebody who opened a file to look at it
            // wants to see what it is before they see what it is made of, and
            // `1` is one key away.
            zoom: Zoom::Fit,
            show_meta: true,
            origin: (0.0, 0.0),
            grab: None,
            canvas: Canvas::default(),
            scale: 1.0,
            blocks: Vec::new(),
            key: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_at(&self, path: &Path) -> bool {
        self.path == path
    }

    /// The tab strip's label.
    pub fn name(&self) -> String {
        self.title.clone()
    }

    /// Replaces the picture with a freshly read one, keeping the zoom and the
    /// pan: `F5` on a file being re-exported by another program should show the
    /// new pixels in the place the user was looking, not throw the view away.
    pub fn replace(&mut self, image: Image, file_bytes: u64) {
        self.image = image;
        self.file_bytes = file_bytes;
        self.key = None;
    }

    /// Image pixels per half-cell, as the last sync resolved it.
    #[cfg(test)]
    fn scale(&self) -> f64 {
        self.scale
    }

    /// Whether the metadata column fits beside a picture worth looking at.
    ///
    /// Asked of the *pane* rather than of the canvas, because it is what
    /// decides how the pane is split.
    pub fn meta_fits(&self, pane_width: u16) -> bool {
        self.show_meta && pane_width >= META_WIDTH + MIN_CANVAS
    }

    // --- the view ---------------------------------------------------------

    /// Rebuilds the block grid if anything it was built from has moved.
    ///
    /// Called once a frame from the run loop, beside `sync_highlight` and for
    /// the same reason: it is a cache, and a cache has to write.
    pub fn sync(&mut self, canvas: Canvas) {
        self.canvas = canvas;
        self.scale = self.resolve_scale();
        self.clamp();
        let key = CacheKey {
            canvas,
            scale: self.scale,
            origin: self.origin,
        };
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.resample();
    }

    /// The scale `zoom` means in this pane.
    fn resolve_scale(&self) -> f64 {
        match self.zoom {
            Zoom::Level(index) => LEVELS[index.min(LEVELS.len() - 1)],
            Zoom::Fit => self.fit_scale(),
        }
    }

    /// The largest scale at which the whole picture is inside the pane.
    ///
    /// Capped at the top step rather than left free: a 16×16 icon in a wide
    /// pane would otherwise be fitted at four hundred pixels a cell, which is
    /// one colour and no picture.
    fn fit_scale(&self) -> f64 {
        if self.canvas.is_empty() || self.image.width == 0 || self.image.height == 0 {
            return 1.0;
        }
        let by_width = f64::from(self.canvas.width) / f64::from(self.image.width);
        let by_height = f64::from(self.canvas.rows()) / f64::from(self.image.height);
        by_width.min(by_height).min(LEVELS[LEVELS.len() - 1])
    }

    /// How many image pixels the pane spans, across and down.
    fn span(&self) -> (f64, f64) {
        let scale = self.scale.max(f64::MIN_POSITIVE);
        (
            f64::from(self.canvas.width) / scale,
            f64::from(self.canvas.rows()) / scale,
        )
    }

    /// Keeps the window over the picture rather than beside it.
    ///
    /// An axis the picture is smaller than is centred — which is where a
    /// viewer puts a picture that fits — and an axis it is larger than is held
    /// between its edges, so there is never a margin on one side and cropped
    /// pixels on the other.
    fn clamp(&mut self) {
        let (span_x, span_y) = self.span();
        self.origin.0 = clamp_axis(self.origin.0, f64::from(self.image.width), span_x);
        self.origin.1 = clamp_axis(self.origin.1, f64::from(self.image.height), span_y);
    }

    /// Fills `blocks` from the picture: one sample per half-cell.
    ///
    /// Magnifying takes the nearest pixel, so a zoomed-in picture shows square
    /// pixels and not a blur — at 8× the user is looking *at* the pixels.
    /// Shrinking averages the box each half-cell covers, because nearest-
    /// neighbour over a photo reduced forty-fold is a field of stray pixels
    /// with none of the picture left in it.
    fn resample(&mut self) {
        let cells = (self.canvas.width as usize) * (self.canvas.height as usize);
        self.blocks.clear();
        self.blocks.resize(cells, Block::default());
        if cells == 0 || self.image.width == 0 || self.image.height == 0 {
            return;
        }
        let magnifying = self.scale >= 1.0;
        let step = 1.0 / self.scale;
        for cy in 0..self.canvas.height {
            for cx in 0..self.canvas.width {
                let x0 = self.origin.0 + f64::from(cx) * step;
                let top = self.origin.1 + f64::from(cy * 2) * step;
                let bottom = top + step;
                let block = Block {
                    top: self.sample(x0, top, step, magnifying),
                    bottom: self.sample(x0, bottom, step, magnifying),
                };
                self.blocks[(cy as usize) * (self.canvas.width as usize) + (cx as usize)] = block;
            }
        }
    }

    /// The colour of the `step`-by-`step` box of image whose corner is at
    /// `(x, y)`.
    fn sample(&self, x: f64, y: f64, step: f64, magnifying: bool) -> [u8; 3] {
        if magnifying {
            let px = x.floor().max(0.0) as u32;
            let py = y.floor().max(0.0) as u32;
            return self.image.pixel(px, py);
        }
        let x0 = x.floor().max(0.0) as u32;
        let y0 = y.floor().max(0.0) as u32;
        let x1 = ((x + step).ceil().max(0.0) as u32).min(self.image.width);
        let y1 = ((y + step).ceil().max(0.0) as u32).min(self.image.height);
        if x1 <= x0 || y1 <= y0 {
            return self.image.pixel(x0, y0);
        }
        // A box wider or taller than `SAMPLES` is strided rather than walked,
        // which is what bounds the cost of a frame: at "fit" the box grows
        // with the picture, so a 64-megapixel photo would otherwise cost
        // twenty times a 3-megapixel one for a pane of exactly the same size.
        // Sixty-four pixels spread evenly across a box of six hundred is the
        // same average to within a shade, and it is the difference between a
        // pan that keeps up and one that does not.
        let stride_x = (x1 - x0).div_ceil(SAMPLES).max(1);
        let stride_y = (y1 - y0).div_ceil(SAMPLES).max(1);
        let mut sum = [0u32; 3];
        let mut count = 0u32;
        for py in (y0..y1).step_by(stride_y as usize) {
            for px in (x0..x1).step_by(stride_x as usize) {
                let pixel = self.image.pixel(px, py);
                sum[0] += u32::from(pixel[0]);
                sum[1] += u32::from(pixel[1]);
                sum[2] += u32::from(pixel[2]);
                count += 1;
            }
        }
        if count == 0 {
            return [0, 0, 0];
        }
        [
            (sum[0] / count) as u8,
            (sum[1] / count) as u8,
            (sum[2] / count) as u8,
        ]
    }

    /// The cell at `(x, y)` of the grid, for the renderer.
    pub fn block(&self, x: u16, y: u16) -> Block {
        if x >= self.canvas.width || y >= self.canvas.height {
            return Block::default();
        }
        self.blocks[(y as usize) * (self.canvas.width as usize) + (x as usize)]
    }

    // --- moving around ----------------------------------------------------

    /// Steps the zoom, keeping the pixel under the middle of the pane where it
    /// is.
    ///
    /// Anchoring on the centre rather than on the origin is what makes zooming
    /// feel like a magnifying glass: the thing being looked at stays under the
    /// eye, instead of sliding off towards the bottom right as the picture
    /// grows.
    pub fn zoom_by(&mut self, delta: i16) {
        let current = self.nearest_level();
        let next = (current as i16 + delta).clamp(0, LEVELS.len() as i16 - 1) as usize;
        self.set_scale(LEVELS[next]);
        self.zoom = Zoom::Level(next);
    }

    /// Goes to one pixel per half-cell.
    pub fn zoom_actual(&mut self) {
        self.set_scale(LEVELS[ACTUAL]);
        self.zoom = Zoom::Level(ACTUAL);
    }

    /// Goes back to the whole picture in the pane.
    pub fn zoom_fit(&mut self) {
        self.zoom = Zoom::Fit;
        self.key = None;
    }

    /// Which step the current scale is on, for a zoom that started from `Fit`.
    fn nearest_level(&self) -> usize {
        let scale = self.scale;
        LEVELS
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let (a, b) = ((*a - scale).abs(), (*b - scale).abs());
                a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
            .unwrap_or(ACTUAL)
    }

    /// Changes the scale about the centre of the pane.
    fn set_scale(&mut self, scale: f64) {
        let (span_x, span_y) = self.span();
        let centre = (self.origin.0 + span_x / 2.0, self.origin.1 + span_y / 2.0);
        self.scale = scale;
        let (span_x, span_y) = self.span();
        self.origin = (centre.0 - span_x / 2.0, centre.1 - span_y / 2.0);
        self.clamp();
        self.key = None;
    }

    /// Moves the window by a fraction of what it can see, in each axis.
    ///
    /// Measured against the *visible span* rather than in pixels, so one press
    /// moves the same distance across the pane whether the picture is at 6% or
    /// at 800%.
    pub fn pan(&mut self, dx: i16, dy: i16) {
        let (span_x, span_y) = self.span();
        self.shift(
            f64::from(dx) * span_x * PAN_FRACTION,
            f64::from(dy) * span_y * PAN_FRACTION,
        );
    }

    /// A whole pane's worth, vertically: what `PageUp` and `PageDown` do to
    /// every other pane in the editor.
    pub fn pan_page(&mut self, delta: i16) {
        let (_, span_y) = self.span();
        self.shift(0.0, f64::from(delta) * span_y);
    }

    fn shift(&mut self, dx: f64, dy: f64) {
        self.origin = (self.origin.0 + dx, self.origin.1 + dy);
        self.clamp();
        self.key = None;
    }

    /// Puts the middle of the picture in the middle of the pane.
    pub fn centre(&mut self) {
        let (span_x, span_y) = self.span();
        self.origin = (
            (f64::from(self.image.width) - span_x) / 2.0,
            (f64::from(self.image.height) - span_y) / 2.0,
        );
        self.clamp();
        self.key = None;
    }

    /// Remembers where a drag started, so the pan that follows is measured
    /// from the press and not from the previous frame.
    ///
    /// From the press, because a drag reported as a series of positions can
    /// skip cells over a slow link — and a pan accumulated from the gaps would
    /// drift away from the pointer over the length of one gesture.
    pub fn grab(&mut self, cell: (u16, u16)) {
        self.grab = Some((cell, self.origin));
    }

    pub fn release(&mut self) {
        self.grab = None;
    }

    /// Drags the picture under the pointer: the pixel grabbed stays under it.
    pub fn drag_to(&mut self, cell: (u16, u16)) {
        let Some((from, origin)) = self.grab else {
            return;
        };
        let step = 1.0 / self.scale.max(f64::MIN_POSITIVE);
        let dx = f64::from(from.0) - f64::from(cell.0);
        // Two pixel rows to a cell, so a cell of vertical drag is two of them.
        let dy = (f64::from(from.1) - f64::from(cell.1)) * 2.0;
        self.origin = (origin.0 + dx * step, origin.1 + dy * step);
        self.clamp();
        self.key = None;
    }

    // --- what the chrome says ---------------------------------------------

    /// ` photo.jpg — JPEG 4032×3024 — 12% `, the frame's title.
    pub fn title(&self) -> String {
        format!(
            " {} — {} {}×{} — {} ",
            self.title,
            self.image.format.label(),
            self.image.width,
            self.image.height,
            self.zoom_label()
        )
    }

    /// `12%`, or `Fit 12%` while the zoom is following the pane.
    ///
    /// The percentage is shown either way: `Fit` alone says how the number was
    /// chosen and not what it is, and "how big is this on screen?" is the
    /// question the readout exists to answer.
    pub fn zoom_label(&self) -> String {
        let percent = self.scale * 100.0;
        // A decimal only where a whole number would round to nothing: at 3.6%
        // "4%" is a different answer, and at 250% a decimal is noise.
        let rendered = if percent >= 10.0 {
            format!("{percent:.0}%")
        } else {
            format!("{percent:.1}%")
        };
        match self.zoom {
            Zoom::Fit => format!("Fit {rendered}"),
            Zoom::Level(_) => rendered,
        }
    }

    /// `1024,768`, the bottom-right readout: which pixel of the picture is in
    /// the pane's top-left corner.
    pub fn position(&self) -> String {
        format!(
            " {},{} ",
            self.origin.0.max(0.0).round() as u64,
            self.origin.1.max(0.0).round() as u64
        )
    }

    /// The metadata column, as labelled rows.
    ///
    /// A blank label is a spacer, which is how the four groups — the file, the
    /// picture, the colour and the view — are separated without a widget for it.
    pub fn meta_rows(&self) -> Vec<(&'static str, String)> {
        let (span_x, span_y) = self.span();
        vec![
            ("File", self.title.clone()),
            ("Size", byte_label(self.file_bytes)),
            ("", String::new()),
            ("Format", self.image.format.label().to_string()),
            ("Width", format!("{} px", self.image.width)),
            ("Height", format!("{} px", self.image.height)),
            ("Pixels", format!("{:.1} MP", self.image.megapixels())),
            ("Aspect", aspect_label(self.image.width, self.image.height)),
            ("", String::new()),
            ("Colour", self.image.colour.clone()),
            (
                "Alpha",
                if self.image.has_alpha {
                    "yes — over a checkerboard".to_string()
                } else {
                    "no".to_string()
                },
            ),
            ("Bytes/px", bytes_per_pixel(self.file_bytes, &self.image)),
            ("", String::new()),
            ("Zoom", self.zoom_label()),
            (
                "Origin",
                format!(
                    "{}, {}",
                    self.origin.0.max(0.0).round() as u64,
                    self.origin.1.max(0.0).round() as u64
                ),
            ),
            (
                "Showing",
                format!("{} × {} px", span_x.round() as u64, span_y.round() as u64),
            ),
        ]
    }
}

/// Where one axis of the window sits over one axis of the picture.
fn clamp_axis(origin: f64, picture: f64, span: f64) -> f64 {
    if span >= picture {
        // Smaller than the pane: centred, which puts the origin before zero.
        (picture - span) / 2.0
    } else {
        origin.clamp(0.0, picture - span)
    }
}

/// A byte count as a file manager shows one. The same rule the open progress
/// box follows: powers of two under names that say ten, because that is what
/// the tool the user checked in said.
fn byte_label(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

/// `16:9`, or `1.91:1` when there is no small whole-number ratio.
///
/// Reduced by the greatest common divisor, and only shown as two integers when
/// both are small enough to read: `4032:3024` is arithmetic, `4:3` is the
/// answer somebody wanted.
fn aspect_label(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return "—".to_string();
    }
    let divisor = gcd(width, height);
    let (w, h) = (width / divisor, height / divisor);
    if w <= 40 && h <= 40 {
        format!("{w}:{h}")
    } else {
        format!("{:.2}:1", f64::from(width) / f64::from(height))
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a.max(1)
    } else {
        gcd(b, a % b)
    }
}

/// How hard the file was compressed, as bytes on disk per pixel.
///
/// The one number in the column that says something the header does not: a
/// photograph at 0.3 and a screenshot at 0.02 are different kinds of file, and
/// a PNG at 3.0 is one that was never compressed at all.
fn bytes_per_pixel(file_bytes: u64, image: &Image) -> String {
    let pixels = u64::from(image.width) * u64::from(image.height);
    if pixels == 0 {
        return "—".to_string();
    }
    format!("{:.2}", file_bytes as f64 / pixels as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Format;

    /// A picture of flat colour bands: row `y` is grey `y * 10`, so a sample
    /// says which row it came from. The bands repeat every 26 rows, which is
    /// where the byte wraps — no test below reads past one wrap.
    fn striped(width: u32, height: u32) -> Image {
        let mut bytes = Vec::new();
        for y in 0..height {
            for _ in 0..width {
                let v = (y * 10) as u8;
                bytes.extend_from_slice(&[v, v, v]);
            }
        }
        Image::from_raw(Format::Png, width, height, "RGB 8-bit".into(), false, bytes)
    }

    fn viewer(width: u32, height: u32) -> ImageState {
        ImageState::new(Path::new("/tmp/x.png"), striped(width, height), 1234)
    }

    #[test]
    fn a_picture_opens_fitted_to_the_pane() {
        let mut view = viewer(100, 100);
        view.sync(Canvas {
            width: 50,
            height: 25,
        });
        // Fifty cells across is fifty pixels wide; fifty rows of half-cells is
        // fifty pixels down. Both give a half, and the whole picture is in.
        assert_eq!(view.zoom, Zoom::Fit);
        assert!((view.scale() - 0.5).abs() < 1e-9, "{}", view.scale());
        assert!(
            view.zoom_label().starts_with("Fit "),
            "{}",
            view.zoom_label()
        );
    }

    #[test]
    fn a_picture_smaller_than_the_pane_is_centred_rather_than_pinned() {
        let mut view = viewer(10, 10);
        view.zoom_actual();
        view.sync(Canvas {
            width: 40,
            height: 20,
        });
        // Forty pixels of span over a ten-pixel picture: the origin is fifteen
        // pixels before the left edge, which is the picture in the middle.
        assert!((view.origin.0 + 15.0).abs() < 1e-9, "{:?}", view.origin);
    }

    #[test]
    fn the_window_never_leaves_the_picture_on_an_axis_larger_than_the_pane() {
        let mut view = viewer(400, 400);
        view.zoom_actual();
        view.sync(Canvas {
            width: 40,
            height: 20,
        });
        view.pan(100, 100);
        view.sync(Canvas {
            width: 40,
            height: 20,
        });
        assert_eq!(view.origin, (360.0, 360.0), "stopped at the far edge");
        view.pan(-100, -100);
        view.sync(Canvas {
            width: 40,
            height: 20,
        });
        assert_eq!(view.origin, (0.0, 0.0), "and at the near one");
    }

    #[test]
    fn zooming_steps_through_the_levels_and_stops_at_both_ends() {
        let mut view = viewer(100, 100);
        view.zoom_actual();
        assert_eq!(view.zoom, Zoom::Level(ACTUAL));
        view.zoom_by(1);
        assert_eq!(view.zoom, Zoom::Level(ACTUAL + 1));
        view.zoom_by(-1);
        assert_eq!(view.zoom, Zoom::Level(ACTUAL));
        view.zoom_by(-99);
        assert_eq!(view.zoom, Zoom::Level(0));
        view.zoom_by(99);
        assert_eq!(view.zoom, Zoom::Level(LEVELS.len() - 1));
    }

    /// Zooming in on a picture larger than the pane keeps what was in the
    /// middle of the pane in the middle of the pane.
    #[test]
    fn zooming_holds_the_middle_of_the_pane_still() {
        let mut view = viewer(400, 400);
        view.zoom_actual();
        let canvas = Canvas {
            width: 40,
            height: 20,
        };
        view.sync(canvas);
        view.pan(4, 4);
        view.sync(canvas);
        let before = (view.origin.0 + 20.0, view.origin.1 + 20.0);
        view.zoom_by(2);
        view.sync(canvas);
        let (span_x, span_y) = view.span();
        let after = (view.origin.0 + span_x / 2.0, view.origin.1 + span_y / 2.0);
        assert!((before.0 - after.0).abs() < 1.0, "{before:?} {after:?}");
        assert!((before.1 - after.1).abs() < 1.0, "{before:?} {after:?}");
    }

    /// A cell is two pixel rows, so at 1:1 the top and the bottom of one cell
    /// are consecutive rows of the picture.
    #[test]
    fn one_cell_holds_two_rows_of_the_picture() {
        let mut view = viewer(4, 8);
        view.zoom_actual();
        view.sync(Canvas {
            width: 4,
            height: 4,
        });
        assert_eq!(view.block(0, 0).top, [0, 0, 0]);
        assert_eq!(view.block(0, 0).bottom, [10, 10, 10]);
        assert_eq!(view.block(0, 1).top, [20, 20, 20]);
    }

    /// Shrinking averages the rows a half-cell covers rather than picking one:
    /// four rows of 0, 10, 20 and 30 come back as 15.
    #[test]
    fn shrinking_averages_the_pixels_a_half_cell_covers() {
        let mut view = viewer(4, 8);
        view.zoom = Zoom::Level(4); // a quarter: four pixel rows per half-cell
        view.sync(Canvas {
            width: 1,
            height: 1,
        });
        assert_eq!(view.block(0, 0).top, [15, 15, 15]);
    }

    /// A box taller than the cap is strided rather than walked, so the cost of
    /// a frame follows the pane and not the picture. The stride still spans
    /// the whole box, so it averages the same stretch of picture.
    #[test]
    fn a_box_larger_than_the_cap_is_sampled_at_a_stride() {
        // A sixteenth: sixteen rows of picture into one half-cell, which is
        // twice `SAMPLES`, so every second row is read.
        let mut view = viewer(4, 128);
        view.zoom = Zoom::Level(0);
        view.sync(Canvas {
            width: 1,
            height: 1,
        });
        // Rows 0, 2, 4 … 14 at ten grey levels a row: 560 / 8 = 70.
        assert_eq!(view.block(0, 0).top, [70, 70, 70]);
    }

    #[test]
    fn a_frame_where_nothing_moved_does_not_resample() {
        let mut view = viewer(64, 64);
        let canvas = Canvas {
            width: 20,
            height: 10,
        };
        view.sync(canvas);
        let key = view.key;
        view.sync(canvas);
        assert_eq!(view.key, key, "the cache key is unchanged");
        view.pan(1, 0);
        assert!(view.key.is_none(), "a pan drops the cache");
    }

    #[test]
    fn a_drag_keeps_the_pixel_that_was_grabbed_under_the_pointer() {
        let mut view = viewer(400, 400);
        view.zoom_actual();
        let canvas = Canvas {
            width: 40,
            height: 20,
        };
        view.sync(canvas);
        view.pan(4, 4);
        view.sync(canvas);
        let before = view.origin;
        view.grab((20, 10));
        view.drag_to((15, 10));
        // The picture was pulled five cells to the left, so the window over it
        // moved five pixels to the right: the pixel grabbed at cell 20 is now
        // at cell 15.
        assert!(
            (view.origin.0 - (before.0 + 5.0)).abs() < 1e-9,
            "{:?}",
            view.origin
        );
        view.release();
        view.drag_to((0, 0));
        assert!(
            (view.origin.0 - (before.0 + 5.0)).abs() < 1e-9,
            "a released drag moves nothing"
        );
    }

    #[test]
    fn the_metadata_column_is_dropped_when_it_would_crowd_out_the_picture() {
        let view = viewer(10, 10);
        assert!(view.meta_fits(80));
        assert!(!view.meta_fits(40));
    }

    #[test]
    fn the_readouts_say_what_the_file_is_and_where_the_window_is() {
        let mut view = viewer(1920, 1080);
        view.zoom_actual();
        let canvas = Canvas {
            width: 80,
            height: 24,
        };
        view.sync(canvas);
        assert!(view.title().contains("PNG 1920×1080"), "{}", view.title());
        assert!(view.title().contains("100%"), "{}", view.title());
        let rows = view.meta_rows();
        let aspect = rows.iter().find(|(label, _)| *label == "Aspect").unwrap();
        assert_eq!(aspect.1, "16:9");
    }

    #[test]
    fn aspect_ratios_reduce_when_they_can_and_read_as_a_decimal_when_they_cannot() {
        assert_eq!(aspect_label(1920, 1080), "16:9");
        assert_eq!(aspect_label(4032, 3024), "4:3");
        assert_eq!(aspect_label(1001, 1000), "1.00:1");
        assert_eq!(aspect_label(0, 100), "—");
    }

    #[test]
    fn sizes_read_the_way_a_file_manager_shows_them() {
        assert_eq!(byte_label(512), "512 B");
        assert_eq!(byte_label(2048), "2.0 KB");
        assert_eq!(byte_label(3 * 1024 * 1024), "3.0 MB");
    }

    /// An empty pane is a resize in progress, not a bug: nothing is sampled and
    /// nothing panics.
    #[test]
    fn a_pane_with_no_room_in_it_draws_nothing_and_does_not_panic() {
        let mut view = viewer(64, 64);
        view.sync(Canvas {
            width: 0,
            height: 0,
        });
        assert_eq!(view.block(0, 0), Block::default());
    }
}
