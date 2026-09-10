//! UI: the read-only image viewer (ADR-078).
//!
//! The editor pane, drawn for a tab that holds a picture instead of a
//! document: a column of metadata down the left and the picture beside it, in
//! upper-half-block characters — the foreground of each cell is one row of
//! pixels and its background is the next, so a terminal cell carries two.
//!
//! Nothing is sampled here. `app::image` resamples into a grid of blocks once
//! per change and this file paints that grid, which is what keeps `ui/`
//! read-only over `&App` (ARCHITECTURE §1) and keeps a still picture free.

use std::sync::OnceLock;

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::image::ImageState;
use crate::app::App;
use crate::ui::layout::LayoutRects;
use crate::ui::theme::Theme;

/// The glyph the picture is made of: the top half of the cell, so its
/// foreground is the upper pixel row and its background the lower one.
///
/// One character rather than the quarter-block or braille alternatives. The
/// halves are the only blocks a terminal renders as an exact split of the cell
/// in every font that has them, and two independent colours per cell is what
/// makes a picture rather than a texture — quarter blocks would give four
/// pixels and still only two colours.
const HALF: &str = "▀";

/// Takes the whole rect table rather than one area: the metadata column and
/// the canvas are laid out in `ui::layout`, and the mouse hit-tests against
/// the same two rects. A renderer that split the pane a second time here would
/// be a second answer that has to agree with the first.
pub fn render(frame: &mut Frame, app: &App, rects: &LayoutRects, theme: &Theme) {
    let (Some(view), Some(area)) = (app.image(), rects.image) else {
        return;
    };
    let focused = app.focus == FocusTarget::Image;
    // The editor is drawn underneath, so the ground is taken back first.
    frame.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.border_for(focused))
        .style(Style::new().bg(theme.background))
        .title(Span::styled(view.title(), theme.panel_title))
        .title_bottom(
            Line::from(Span::styled(view.position(), Style::new().fg(theme.dim))).right_aligned(),
        );
    frame.render_widget(block, area);

    if let Some(meta) = rects.image_meta {
        render_meta(frame, view, meta, theme);
    }
    if let Some(canvas) = rects.image_canvas {
        render_canvas(frame, view, canvas);
    }
}

/// The labelled values down the left: what the file is, what the picture is,
/// and where in it the window sits.
///
/// The label is dim and the value is not, so the column is read down the values
/// and the labels are there when one of them needs naming.
fn render_meta(frame: &mut Frame, view: &ImageState, area: Rect, theme: &Theme) {
    let width = area.width as usize;
    let lines: Vec<Line> = view
        .meta_rows()
        .into_iter()
        .take(area.height as usize)
        .map(|(label, value)| {
            if label.is_empty() {
                return Line::default();
            }
            // A fixed label column, so the values line up down the panel; a
            // value too long for what is left is cut rather than wrapped,
            // because a wrapped value would push the row below it off the
            // bottom and take a different one away.
            let label_width = 9.min(width);
            let room = width.saturating_sub(label_width + 1);
            Line::from(vec![
                Span::styled(
                    format!("{label:<label_width$} "),
                    Style::new().fg(theme.dim),
                ),
                Span::styled(truncate(&value, room), Style::new().fg(theme.foreground)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Paints the block grid straight into the frame's buffer.
///
/// Cell by cell rather than through a widget: every cell has a foreground and a
/// background of its own, which is two colours per cell and no runs to build
/// spans out of.
fn render_canvas(frame: &mut Frame, view: &ImageState, area: Rect) {
    let truecolor = truecolor();
    let buffer = frame.buffer_mut();
    for y in 0..area.height {
        for x in 0..area.width {
            let block = view.block(x, y);
            buffer[(area.x + x, area.y + y)]
                .set_symbol(HALF)
                .set_fg(colour(block.top, truecolor))
                .set_bg(colour(block.bottom, truecolor));
        }
    }
}

/// A value cut to `width` cells, with an ellipsis where it was cut.
fn truncate(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if value.chars().count() <= width {
        return value.to_string();
    }
    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Whether the terminal was told to expect twenty-four-bit colour.
///
/// Answered once and remembered: it is an environment variable, and reading it
/// per cell of a full-screen picture would be twenty thousand lookups a frame.
fn truecolor() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(|| {
        matches!(
            std::env::var("COLORTERM").as_deref(),
            Ok("truecolor") | Ok("24bit")
        )
    })
}

/// One pixel as a terminal colour.
///
/// The theme is indexed on purpose (ADR-007) and a picture is the one place
/// that rule does not reach: a photograph in sixteen colours is not a
/// photograph. So RGB where the terminal advertises it, and the xterm-256 cube
/// where it does not — which every terminal the editor supports does have, and
/// which is the same picture a little coarser rather than a different one.
fn colour(rgb: [u8; 3], truecolor: bool) -> Color {
    if truecolor {
        return Color::Rgb(rgb[0], rgb[1], rgb[2]);
    }
    Color::Indexed(xterm256(rgb))
}

/// The nearest colour in the xterm-256 palette.
///
/// Two candidates are compared and the closer wins: the 6×6×6 colour cube at
/// index 16, and the twenty-four-step grey ramp at index 232. The greys matter
/// because the cube's own greys are six steps apart, and a grey photograph
/// quantised to six levels is a poster.
fn xterm256(rgb: [u8; 3]) -> u8 {
    let cube = rgb.map(cube_index);
    let cube_rgb = cube.map(cube_level);
    let cube_code = 16 + 36 * cube[0] + 6 * cube[1] + cube[2];

    // The grey ramp runs from 8 to 238 in steps of ten.
    let luma = (u32::from(rgb[0]) * 3 + u32::from(rgb[1]) * 6 + u32::from(rgb[2])) / 10;
    let step = ((luma as i32 - 8).clamp(0, 238) as u32 * 2 + 10) / 20;
    let step = step.min(23) as u8;
    let grey = 8 + step * 10;
    let grey_rgb = [grey, grey, grey];

    if distance(rgb, cube_rgb) <= distance(rgb, grey_rgb) {
        cube_code
    } else {
        232 + step
    }
}

/// Which of the cube's six levels a channel is nearest.
///
/// The levels are 0, 95, 135, 175, 215, 255 — not evenly spaced, which is why
/// this is a search over the table and not a division.
fn cube_index(value: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let mut best = 0;
    let mut best_distance = u16::MAX;
    for (index, level) in LEVELS.iter().enumerate() {
        let distance = (i16::from(value) - i16::from(*level)).unsigned_abs();
        if distance < best_distance {
            best_distance = distance;
            best = index as u8;
        }
    }
    best
}

fn cube_level(index: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    LEVELS[index as usize % 6]
}

/// Squared distance in RGB, which is enough to choose between two candidates
/// that are already close.
fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3)
        .map(|i| {
            let d = i32::from(a[i]) - i32::from(b[i]);
            (d * d) as u32
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_colours_land_on_the_corners_of_the_cube() {
        // 16 is the cube's black, 231 its white, 196 pure red.
        assert_eq!(xterm256([0, 0, 0]), 16);
        assert_eq!(xterm256([255, 255, 255]), 231);
        assert_eq!(xterm256([255, 0, 0]), 196);
        assert_eq!(xterm256([0, 0, 255]), 21);
    }

    /// A grey between two cube levels goes to the ramp, which is where the
    /// nearer colour is.
    #[test]
    fn a_mid_grey_takes_the_ramp_rather_than_the_cube() {
        let code = xterm256([120, 120, 120]);
        assert!((232..=255).contains(&code), "{code} is not on the ramp");
    }

    #[test]
    fn truecolor_is_the_pixel_itself_and_indexed_is_the_nearest_to_it() {
        assert_eq!(colour([1, 2, 3], true), Color::Rgb(1, 2, 3));
        assert_eq!(colour([0, 0, 0], false), Color::Indexed(16));
    }

    #[test]
    fn a_value_too_long_for_the_column_is_cut_with_an_ellipsis() {
        assert_eq!(truncate("photo.png", 20), "photo.png");
        assert_eq!(truncate("a-very-long-name.png", 8), "a-very-…");
        assert_eq!(truncate("x", 0), "");
    }
}
