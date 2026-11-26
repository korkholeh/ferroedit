//! UI: the Markdown preview (SPEC §72, ADR-086).
//!
//! The editor pane, drawn for a tab whose Markdown is being read typeset: the
//! rows `app::markdown` laid out, a scrollbar, and — when the pane has room and
//! the document has headings — an outline down the right that marks the
//! section being read and goes to a heading when clicked.
//!
//! Everything here is painting. The parse, the wrapping and the scaling of the
//! pictures were done in `MarkdownView::sync` (ARCHITECTURE invariant 4); what
//! is decided in this file is colour, and where the outline's window sits.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Clear};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::focus::FocusTarget;
use crate::app::markdown::{self as preview, Geometry, Look, MarkdownView, Tone};
use crate::app::App;
use crate::commands::Command;
use crate::editor::markdown::Alert;
use crate::event::keyboard;
use crate::ui::image::{colour, truecolor, HALF};
use crate::ui::scrollbar;
use crate::ui::theme::Theme;

/// Rows the outline spends on itself: its title above the entries and its keys
/// below them.
const OUTLINE_CHROME: u16 = 2;

/// Where a click on the preview landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// An entry of the outline.
    Heading(usize),
    /// The text: a row of the whole layout and a column of the text area.
    Text { row: usize, col: usize },
}

fn geometry_for(view: &MarkdownView, area: Rect) -> Geometry {
    preview::geometry(area.width, view.outline && view.has_headings())
}

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(view) = app.active().and_then(|tab| tab.preview.as_ref()) else {
        return;
    };
    // The editor is drawn underneath, so the ground is taken back first and
    // then repainted in the theme's colour (see `ui::table`).
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::new().style(Style::new().bg(theme.background).fg(theme.foreground)),
        area,
    );
    if area.width == 0 || area.height == 0 {
        return;
    }
    let geometry = geometry_for(view, area);
    let focused = app.focus == FocusTarget::Editor;
    let code = code_ground(theme);
    let truecolor = truecolor();
    let text_x = area.x + geometry.text_x;
    let text_right = (text_x + geometry.text_width).min(area.x + geometry.track_x);

    let rows = &view.rendered.rows;
    let buffer = frame.buffer_mut();
    for y in 0..area.height {
        let Some(row) = rows.get(view.top + y as usize) else {
            break;
        };
        let screen_y = area.y + y;
        if let Some(fill) = row.fill {
            for x in (text_x + fill).min(text_right)..text_right {
                buffer[(x, screen_y)].set_symbol(" ").set_bg(code);
            }
        }
        let mut x = text_x;
        for piece in &row.pieces {
            if x >= text_right {
                break;
            }
            let room = (text_right - x) as usize;
            let (end, _) = buffer.set_stringn(
                x,
                screen_y,
                &piece.text,
                room,
                style(piece.look, theme, code),
            );
            x = end;
        }
        if let Some(slice) = row.picture {
            if let Some(picture) = view.rendered.pictures.get(slice.picture) {
                let left = text_x + slice.x;
                for px in 0..picture.width {
                    let cx = left + px;
                    if cx >= text_right {
                        break;
                    }
                    let cell = picture.cell(px, slice.row);
                    buffer[(cx, screen_y)]
                        .set_symbol(HALF)
                        .set_fg(colour(cell.top, truecolor))
                        .set_bg(colour(cell.bottom, truecolor));
                }
            }
        }
    }

    let track = Rect::new(area.x + geometry.track_x, area.y, 1, area.height);
    scrollbar::render(
        frame,
        track,
        theme,
        focused,
        rows.len(),
        area.height as usize,
        view.top,
    );

    if let Some((x, width)) = geometry.outline {
        let outline = Rect::new(area.x + x, area.y, width, area.height);
        render_outline(frame, view, outline, theme, focused);
    }
}

/// The ground code is drawn on: the current-line tone where the theme has one,
/// which is a shade off the page — close enough not to shout, far enough to
/// read as a block — and the unfocused selection's otherwise.
fn code_ground(theme: &Theme) -> Color {
    theme
        .current_line
        .and_then(|style| style.bg)
        .or(theme.selection_unfocused.bg)
        .unwrap_or(theme.background)
}

/// A piece's role, as a colour of the theme.
fn style(look: Look, theme: &Theme, code: Color) -> Style {
    let syntax = &theme.syntax;
    let fg = match look.tone {
        Tone::Text | Tone::TableHead => theme.foreground,
        Tone::Heading(1) => theme.border_focused,
        Tone::Heading(2) => syntax.keyword,
        Tone::Heading(3) => syntax.function,
        Tone::Heading(4) => syntax.type_name,
        Tone::Heading(_) => syntax.tag,
        Tone::Code => syntax.string,
        Tone::Syntax(kind) => syntax.style(kind).fg.unwrap_or(theme.foreground),
        Tone::Link => theme.directory,
        Tone::Quote => syntax.comment,
        Tone::Marker => theme.border_focused,
        Tone::Done => theme.git_added,
        Tone::Rule | Tone::Border => theme.border,
        Tone::Dim => theme.dim,
        Tone::Image => syntax.constant,
        Tone::Alert(alert) => match alert {
            Alert::Note => theme.info,
            Alert::Tip => theme.git_added,
            Alert::Important => syntax.keyword,
            Alert::Warning => theme.warning,
            Alert::Caution => theme.error,
        },
        Tone::Footnote => syntax.number,
    };
    let mut style = Style::new().fg(fg);
    if look.code {
        style = style.bg(code);
    }
    let mut modifiers = Modifier::empty();
    if look.bold {
        modifiers |= Modifier::BOLD;
    }
    if look.italic {
        modifiers |= Modifier::ITALIC;
    }
    if look.strike {
        modifiers |= Modifier::CROSSED_OUT;
    }
    if look.underline {
        modifiers |= Modifier::UNDERLINED;
    }
    style.add_modifier(modifiers)
}

/// The first outline entry on screen: the current heading kept in the middle
/// of the list, so the entries either side of it — where the reader goes next
/// — are both in view.
fn outline_offset(view: &MarkdownView, height: u16) -> usize {
    let room = height.saturating_sub(OUTLINE_CHROME) as usize;
    let len = view.rendered.outline.len();
    let current = view.current_heading().unwrap_or(0);
    current
        .saturating_sub(room / 2)
        .min(len.saturating_sub(room))
}

fn render_outline(
    frame: &mut Frame,
    view: &MarkdownView,
    area: Rect,
    theme: &Theme,
    focused: bool,
) {
    let buffer = frame.buffer_mut();
    // A rule down the left, so the list reads as a panel beside the text and
    // not as a second column of it.
    for y in area.y..area.bottom() {
        buffer[(area.x, y)]
            .set_symbol("│")
            .set_style(Style::new().fg(theme.border));
    }
    let left = area.x + 2;
    let width = area.width.saturating_sub(3) as usize;
    if width == 0 {
        return;
    }
    buffer.set_stringn(
        left,
        area.y,
        "Outline",
        width,
        theme.panel_title.add_modifier(Modifier::BOLD),
    );

    let outline = &view.rendered.outline;
    let top_level = outline.iter().map(|h| h.level).min().unwrap_or(1);
    let current = view.current_heading();
    let offset = outline_offset(view, area.height);
    let room = area.height.saturating_sub(OUTLINE_CHROME);
    for (y, (index, heading)) in outline
        .iter()
        .enumerate()
        .skip(offset)
        .take(room as usize)
        .enumerate()
    {
        let screen_y = area.y + 1 + y as u16;
        let indent = 2 * usize::from(heading.level.saturating_sub(top_level));
        let label = clip(&heading.text, width.saturating_sub(indent));
        let is_current = current == Some(index);
        let base = if heading.level == top_level {
            Style::new()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.foreground)
        };
        let style = match (is_current, focused) {
            (true, true) => theme.selection.add_modifier(Modifier::BOLD),
            (true, false) => theme.selection_unfocused.fg(theme.foreground),
            _ => base,
        };
        if is_current {
            for x in left.saturating_sub(1)..area.right() {
                buffer[(x, screen_y)].set_symbol(" ").set_style(style);
            }
        }
        buffer.set_stringn(
            left + indent as u16,
            screen_y,
            &label,
            width - indent.min(width),
            style,
        );
    }

    // The two keys, on the last row: an outline nobody knows how to close, or
    // how to walk from the keyboard, is half a feature (ADR-071).
    let key = |command: &Command| keyboard::binding_for(command).map_or("", |b| b.label);
    let pieces = [
        (key(&Command::StepHeading(1)), "next"),
        (key(&Command::ToggleMarkdownOutline), "hide"),
    ];
    let spans = crate::ui::legend::spans(&pieces, width, theme);
    let mut x = left;
    for span in spans {
        let (end, _) = buffer.set_stringn(
            x,
            area.bottom() - 1,
            &span.content,
            (area.right() - x) as usize,
            span.style,
        );
        x = end;
    }
}

/// A label cut to `width` cells, with an ellipsis where it was cut.
fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw + 1 > width {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

/// What is under the pointer, for the mouse.
pub fn hit(app: &App, area: Rect, at: Position) -> Option<Hit> {
    let view = app.active()?.preview.as_ref()?;
    let geometry = geometry_for(view, area);
    let y = at.y.checked_sub(area.y)?;
    let x = at.x.checked_sub(area.x)?;
    if let Some((left, _)) = geometry.outline {
        if x >= left {
            let room = area.height.saturating_sub(OUTLINE_CHROME);
            if y == 0 || y > room {
                return None;
            }
            let index = outline_offset(view, area.height) + (y - 1) as usize;
            return (index < view.rendered.outline.len()).then_some(Hit::Heading(index));
        }
    }
    let col = x.checked_sub(geometry.text_x)?;
    if col >= geometry.text_width {
        return None;
    }
    Some(Hit::Text {
        row: view.top + y as usize,
        col: col as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{EditorView, Tab, TabItem};
    use crate::commands::execute::execute_command;
    use crate::editor::document::Document;
    use crate::ui::layout;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    const README: &str = "# FerroEdit\n\nA **terminal** editor. See [usage](#usage).\n\n\
- one\n- two\n\n## Usage\n\n| key | does |\n|-----|------|\n| F7 | preview |\n\n\
```rust\nfn main() {}\n```\n\n## Licence\n\nMIT\n";

    /// A Markdown tab in front, previewed, laid out for a `width`×`height`
    /// screen the way the run loop would have.
    fn previewing(width: u16, height: u16) -> App {
        let mut app = App::fixture();
        let path = std::path::PathBuf::from("/dev/null/ferroedit-fixture/README.md");
        let document = Document::from_text(README, Some(path));
        app.tabs.push(TabItem::editing(Tab::new(document)));
        app.active_tab = Some(app.tabs.len() - 1);
        app.focus = FocusTarget::Editor;
        execute_command(&mut app, Command::ToggleMarkdownPreview);
        let rects = layout::compute(Rect::new(0, 0, width, height), &app);
        app.editor_view = EditorView {
            width: rects.editor.width,
            height: rects.editor.height,
        };
        app.sync_preview();
        app
    }

    fn draw(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), app);
                crate::ui::render(frame, app, &rects, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn the_preview_is_typeset_rather_than_source() {
        let app = previewing(120, 40);
        let screen = draw(&app, 120, 40).join("\n");
        assert!(screen.contains("A terminal editor."), "{screen}");
        assert!(!screen.contains("**terminal**"), "{screen}");
        assert!(!screen.contains("# FerroEdit"), "{screen}");
        assert!(screen.contains("• one"), "{screen}");
        assert!(screen.contains("┌─────┬─────────┐"), "{screen}");
        assert!(screen.contains("fn main() {}"), "{screen}");
        assert!(screen.contains("Outline"), "{screen}");
        assert!(
            screen.contains("Preview"),
            "the status bar says so: {screen}"
        );
    }

    #[test]
    fn a_narrow_pane_has_no_outline() {
        let app = previewing(60, 30);
        let screen = draw(&app, 60, 30).join("\n");
        assert!(!screen.contains("Outline"), "{screen}");
        assert!(screen.contains("FerroEdit"), "{screen}");
    }

    #[test]
    fn a_click_on_the_outline_goes_to_its_heading() {
        let mut app = previewing(120, 40);
        let rects = layout::compute(Rect::new(0, 0, 120, 40), &app);
        let area = rects.preview.expect("the preview is showing");
        let (x, _) = preview::geometry(area.width, true).outline.unwrap();
        // The title is the first row, so the second entry is the third.
        let at = Position::new(area.x + x + 3, area.y + 2);
        assert_eq!(hit(&app, area, at), Some(Hit::Heading(1)));
        execute_command(&mut app, Command::JumpToHeading(1));
        let view = app.active().unwrap().preview.as_ref().unwrap();
        assert_eq!(view.current_heading(), Some(1));
        assert_eq!(view.top, view.rendered.outline[1].row);
    }
}
