//! The Markdown preview's state, and the layout that turns a parse into rows
//! (SPEC §72, ADR-086).
//!
//! Like the CSV view (ADR-062) the preview hangs off the editor tab: the same
//! document, the same `Ctrl+S`, the same history — it is a second reading of
//! the buffer and not a second kind of tab. Unlike the grid it writes nothing:
//! the rows on screen are a typeset copy of the text, and a caret in a wrapped,
//! restyled paragraph would be a caret in characters that are not in the file.
//!
//! The work is split in two caches, both rebuilt by `sync` from the run loop
//! (ARCHITECTURE invariant 4). The parse (`editor::markdown`) is redone when
//! the document's revision moves; the layout is redone when the parse or the
//! width does, because wrapping is the one part of the picture that depends on
//! the pane. What `ui::markdown` gets is a list of rows of styled pieces it can
//! paint without measuring anything.
//!
//! The style of a piece is a `Look` — a *role*, like the syntax highlighter's
//! `StyleKind` — and the theme decides the colour, so a new theme does not have
//! to know there is a preview.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::image::Block as Cell;
use crate::editor::document::Document;
use crate::editor::markdown::{plain, Alert, Align, Block, ImageRef, Item, Markdown, Run};
use crate::image::{decode, Image};
use crate::syntax::highlighter::{syntax_set, LineState, StyleKind, Token};

/// Columns between the pane's left edge and the text, and between the text
/// and the scrollbar.
pub const MARGIN: u16 = 2;

/// The widest the text is set. Prose a hundred and eighty columns wide is a
/// line the eye loses on the way back, which is why every reader caps it.
pub const MEASURE: u16 = 100;

/// A pane narrower than this has no outline: the text needs the room more.
pub const OUTLINE_MIN_PANE: u16 = 72;

/// The tallest a picture is drawn, in rows. Two pixels a row, so forty pixels
/// of height — enough to tell a screenshot from a diagram, and not so much that
/// a README with six of them is six screens of pictures.
pub const PICTURE_ROWS: u16 = 20;

/// The largest file the preview will decode as a picture.
const PICTURE_BYTES: u64 = 32 * 1024 * 1024;

/// What a piece of the preview *is*, for the theme to colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Text,
    Heading(u8),
    /// Inline code, and the text of a code block with no grammar.
    Code,
    /// Code a grammar coloured.
    Syntax(StyleKind),
    Link,
    /// The text of a plain block quote.
    Quote,
    /// A bullet, a list number, an unticked box.
    Marker,
    /// A ticked box.
    Done,
    Rule,
    /// A table's frame.
    Border,
    TableHead,
    /// Captions, a code block's language, anything secondary.
    Dim,
    /// An image that is shown as its alt text.
    Image,
    Alert(Alert),
    Footnote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Look {
    pub tone: Tone,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub underline: bool,
    /// Drawn on the code ground rather than the page's.
    pub code: bool,
}

impl Look {
    fn tone(tone: Tone) -> Self {
        Self {
            tone,
            ..Self::default()
        }
    }
}

/// A run of one look on one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub look: Look,
    /// An index into the document's links, when this is a link.
    pub link: Option<usize>,
}

impl Piece {
    fn new(text: impl Into<String>, look: Look) -> Self {
        Self {
            text: text.into(),
            look,
            link: None,
        }
    }

    pub fn width(&self) -> usize {
        self.text.width()
    }
}

/// Where a row shows a slice of a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    /// An index into `Rendered::pictures`.
    pub picture: usize,
    /// Which row of the picture.
    pub row: u16,
    /// The column of the text area the picture starts at.
    pub x: u16,
}

/// One row of the preview.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Row {
    pub pieces: Vec<Piece>,
    /// The source line the row was laid out from, which is how the preview and
    /// the text agree on where the reader is.
    pub line: usize,
    /// For a row of a code block: the column the code ground starts at. It is
    /// painted from there to the end of the text area, so a block is a block
    /// and not a ragged run of highlighted lines.
    pub fill: Option<u16>,
    pub picture: Option<Slice>,
}

/// A picture scaled to the cells it is drawn in, two pixels a cell (ADR-078).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Cell>,
}

impl Picture {
    pub fn cell(&self, x: u16, y: u16) -> Cell {
        if x >= self.width || y >= self.height {
            return Cell::default();
        }
        self.cells[(y as usize) * (self.width as usize) + (x as usize)]
    }
}

/// One entry of the outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub anchor: String,
    /// The row the heading is drawn on.
    pub row: usize,
    pub line: usize,
}

/// The document, laid out at one width.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub rows: Vec<Row>,
    pub pictures: Vec<Picture>,
    pub outline: Vec<Heading>,
    pub links: Vec<String>,
}

/// How the pane is divided: the text, the scrollbar beside it, and the outline.
/// Every offset is from the pane's left edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub text_x: u16,
    pub text_width: u16,
    pub track_x: u16,
    /// Where the outline starts and how wide it is, when it is drawn.
    pub outline: Option<(u16, u16)>,
}

/// The pane split for a preview `pane_width` cells wide.
///
/// One function for the layout, the renderer and the hit test, so the three
/// cannot disagree about where a column is.
pub fn geometry(pane_width: u16, outline: bool) -> Geometry {
    let outline_width = if outline && pane_width >= OUTLINE_MIN_PANE {
        (pane_width / 4).clamp(20, 32)
    } else {
        0
    };
    let body = pane_width.saturating_sub(outline_width);
    let track_x = body.saturating_sub(1);
    let text_width = track_x.saturating_sub(MARGIN * 2).clamp(1, MEASURE);
    Geometry {
        text_x: MARGIN.min(track_x),
        text_width,
        track_x,
        outline: (outline_width > 0).then_some((body, outline_width)),
    }
}

/// The preview of one tab.
#[derive(Debug, Default)]
pub struct MarkdownView {
    parsed: Option<u64>,
    doc: Markdown,
    laid: Option<(u64, Geometry)>,
    pub rendered: Rendered,
    pub geometry: Option<Geometry>,
    /// The first row on screen.
    pub top: usize,
    /// Whether the outline is wanted. It is drawn only when the pane is wide
    /// enough and the document has a heading to put in it.
    pub outline: bool,
    /// Pictures already read, by path — `None` for one that could not be.
    images: HashMap<PathBuf, Option<Image>>,
    /// The source line to open on, once there is a layout to find it in.
    reveal: Option<usize>,
}

impl MarkdownView {
    /// A preview that opens on the row laid out from `line`.
    pub fn new(line: usize) -> Self {
        Self {
            outline: true,
            reveal: Some(line),
            ..Self::default()
        }
    }

    /// Whether the outline has anything to show.
    pub fn has_headings(&self) -> bool {
        self.doc
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Heading { .. }))
    }

    /// Re-reads and re-lays the document if anything either was made from has
    /// changed. `pane_width` is the whole pane, scrollbar included.
    pub fn sync(&mut self, document: &Document, pane_width: u16) {
        let revision = document.revision();
        if self.parsed != Some(revision) {
            let mut text = String::new();
            for index in 0..document.line_count() {
                if index > 0 {
                    text.push('\n');
                }
                text.push_str(&document.line(index));
            }
            self.doc = Markdown::read(&text);
            self.parsed = Some(revision);
            // A reload is the one way the text changes under a preview, and a
            // picture that was re-exported with it should be read again too.
            self.images.clear();
        }
        let geometry = geometry(pane_width, self.outline && self.has_headings());
        self.geometry = Some(geometry);
        if self.laid == Some((revision, geometry)) {
            return;
        }
        // Where the reader was, in the one coordinate both layouts share.
        let anchor = self
            .reveal
            .take()
            .or_else(|| self.rendered.rows.get(self.top).map(|row| row.line));
        let base = document
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        self.rendered = layout(
            &self.doc,
            geometry.text_width as usize,
            base.as_deref(),
            &mut self.images,
        );
        self.laid = Some((revision, geometry));
        self.top = anchor.map_or(0, |line| self.row_for_line(line));
    }

    /// The first row laid out from `line` or from a line after it.
    pub fn row_for_line(&self, line: usize) -> usize {
        let rows = &self.rendered.rows;
        rows.iter()
            .position(|row| row.line >= line)
            .unwrap_or(rows.len().saturating_sub(1))
    }

    /// The source line of the row at the top of the pane.
    pub fn top_line(&self) -> usize {
        self.rendered.rows.get(self.top).map_or(0, |row| row.line)
    }

    /// The furthest `top` can be scrolled to and still fill a pane `height`
    /// rows tall.
    pub fn max_top(&self, height: usize) -> usize {
        self.rendered.rows.len().saturating_sub(height.max(1))
    }

    /// Moves the window. Going down stops where the last row reaches the
    /// bottom — unless a heading jump already put the window lower, in which
    /// case it stays where it is rather than jumping back up.
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        if delta >= 0 {
            let limit = self.max_top(height).max(self.top);
            self.top = (self.top + delta as usize).min(limit);
        } else {
            self.top = self.top.saturating_sub(delta.unsigned_abs());
        }
    }

    pub fn home(&mut self) {
        self.top = 0;
    }

    pub fn end(&mut self, height: usize) {
        self.top = self.max_top(height);
    }

    /// The heading whose section the top of the pane is in.
    pub fn current_heading(&self) -> Option<usize> {
        let outline = &self.rendered.outline;
        match outline.iter().rposition(|heading| heading.row <= self.top) {
            Some(index) => Some(index),
            // Above the first heading the reader is in its preamble, which the
            // outline has no entry for, and the first entry is the nearest
            // thing to where they are.
            None => (!outline.is_empty()).then_some(0),
        }
    }

    /// Scrolls so the outline's entry `index` is the top row.
    pub fn jump_to_heading(&mut self, index: usize) {
        if let Some(heading) = self.rendered.outline.get(index) {
            self.top = heading.row.min(self.rendered.rows.len().saturating_sub(1));
        }
    }

    /// The next heading below the top of the pane, or the one above it.
    /// Returns the outline index it went to.
    pub fn step_heading(&mut self, delta: isize) -> Option<usize> {
        let outline = &self.rendered.outline;
        let index = if delta > 0 {
            outline.iter().position(|heading| heading.row > self.top)?
        } else {
            outline.iter().rposition(|heading| heading.row < self.top)?
        };
        self.jump_to_heading(index);
        Some(index)
    }

    /// The heading an anchor names, for a `#fragment` link.
    pub fn heading_for_anchor(&self, anchor: &str) -> Option<usize> {
        let anchor = anchor.to_lowercase();
        self.rendered
            .outline
            .iter()
            .position(|heading| heading.anchor == anchor)
    }

    /// The link under column `col` of row `row`, if there is one.
    pub fn link_at(&self, row: usize, col: usize) -> Option<&str> {
        let row = self.rendered.rows.get(row)?;
        let mut x = 0;
        for piece in &row.pieces {
            let width = piece.width();
            if (x..x + width).contains(&col) {
                return piece
                    .link
                    .and_then(|index| self.rendered.links.get(index))
                    .map(String::as_str);
            }
            x += width;
        }
        None
    }

    /// What the status bar says about where the reader is.
    pub fn position(&self, height: usize) -> String {
        let rows = self.rendered.rows.len();
        if rows <= height {
            return "All".to_string();
        }
        let percent = (self.top * 100) / self.max_top(height).max(1);
        format!("{}%", percent.min(100))
    }
}

// --- layout -------------------------------------------------------------------

/// One level of indentation: what it draws on the first row of its block, and
/// on every row after.
struct Level {
    first: Vec<Piece>,
    rest: Vec<Piece>,
    used: bool,
}

struct Layout<'a> {
    width: usize,
    base: Option<&'a Path>,
    images: &'a mut HashMap<PathBuf, Option<Image>>,
    out: Rendered,
    levels: Vec<Level>,
    /// How many lists deep the layout is, for the bullet.
    lists: usize,
    /// How many plain quotes deep, which is what turns text into quote text.
    quotes: usize,
}

/// Lays `doc` out at `width` columns.
pub fn layout(
    doc: &Markdown,
    width: usize,
    base: Option<&Path>,
    images: &mut HashMap<PathBuf, Option<Image>>,
) -> Rendered {
    let mut layout = Layout {
        width: width.max(1),
        base,
        images,
        out: Rendered {
            links: doc.links.clone(),
            ..Rendered::default()
        },
        levels: Vec::new(),
        lists: 0,
        quotes: 0,
    };
    layout.blocks(&doc.blocks, false);
    layout.out
}

impl Layout<'_> {
    fn indent(&self) -> usize {
        self.levels
            .iter()
            .map(|level| level.first.iter().map(Piece::width).sum::<usize>())
            .sum()
    }

    /// The columns left for text inside the current indentation.
    fn room(&self) -> usize {
        self.width.saturating_sub(self.indent()).max(1)
    }

    fn emit(&mut self, pieces: Vec<Piece>, line: usize) -> &mut Row {
        let mut row = Vec::new();
        for level in &mut self.levels {
            if level.used {
                row.extend(level.rest.iter().cloned());
            } else {
                row.extend(level.first.iter().cloned());
                level.used = true;
            }
        }
        row.extend(pieces);
        self.out.rows.push(Row {
            pieces: merge(row),
            line,
            fill: None,
            picture: None,
        });
        self.out.rows.last_mut().expect("just pushed")
    }

    /// A blank row between two blocks. The indentation's continuation is drawn
    /// in it — a quote's bar does not stop between its paragraphs — but it
    /// does not use up a bullet that has not been drawn yet.
    ///
    /// It belongs to the block above it: a row is found by the first one laid
    /// out from a line, and a blank that claimed the next block's line would be
    /// found in place of the block — a heading jump, or a resize, would put
    /// the gap and not the heading at the top of the pane.
    fn blank(&mut self) {
        let line = self.out.rows.last().map_or(0, |row| row.line);
        let mut row: Vec<Piece> = Vec::new();
        for level in &self.levels {
            row.extend(level.rest.iter().cloned());
        }
        while row.last().is_some_and(|piece| piece.text.trim().is_empty()) {
            row.pop();
        }
        self.out.rows.push(Row {
            pieces: row,
            line,
            ..Row::default()
        });
    }

    fn blocks(&mut self, blocks: &[Block], tight: bool) {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 && !tight {
                self.blank();
            }
            self.block(block, tight);
        }
    }

    fn block(&mut self, block: &Block, tight: bool) {
        match block {
            Block::Heading {
                level,
                runs,
                anchor,
                line,
            } => self.heading(*level, runs, anchor, *line),
            Block::Paragraph { runs, line, .. } => {
                let base = if self.quotes > 0 {
                    Look {
                        italic: true,
                        ..Look::tone(Tone::Quote)
                    }
                } else {
                    Look::default()
                };
                let pieces = pieces(runs, base);
                for row in wrap(&pieces, self.room()) {
                    self.emit(row, *line);
                }
            }
            Block::Images { images, line } => self.pictures(images, *line),
            Block::Code { lang, lines, line } => self.code(lang.as_deref(), lines, *line),
            Block::Quote {
                alert,
                blocks,
                line,
            } => {
                let tone = alert.map_or(Tone::Quote, Tone::Alert);
                let bar = vec![Piece::new("▎ ", Look::tone(tone))];
                self.levels.push(Level {
                    first: bar.clone(),
                    rest: bar,
                    used: false,
                });
                if let Some(alert) = alert {
                    let label = Look {
                        bold: true,
                        ..Look::tone(tone)
                    };
                    self.emit(vec![Piece::new(alert.label(), label)], *line);
                } else {
                    self.quotes += 1;
                }
                self.blocks(blocks, false);
                if alert.is_none() {
                    self.quotes -= 1;
                }
                self.levels.pop();
            }
            Block::List { start, items, line } => self.list(*start, items, *line),
            Block::Table {
                aligns,
                header,
                rows,
                line,
            } => self.table(aligns, header, rows, *line),
            Block::Rule { line } => {
                let rule = "─".repeat(self.room());
                self.emit(vec![Piece::new(rule, Look::tone(Tone::Rule))], *line);
            }
            Block::Footnote {
                label,
                blocks,
                line: _,
            } => {
                let marker = format!("[{label}] ");
                let width = marker.width();
                self.levels.push(Level {
                    first: vec![Piece::new(marker, Look::tone(Tone::Footnote))],
                    rest: vec![Piece::new(" ".repeat(width), Look::default())],
                    used: false,
                });
                self.blocks(blocks, tight);
                self.levels.pop();
            }
        }
    }

    fn heading(&mut self, level: u8, runs: &[Run], anchor: &str, line: usize) {
        let look = Look {
            bold: level <= 4,
            italic: level >= 5,
            ..Look::tone(Tone::Heading(level))
        };
        self.out.outline.push(Heading {
            level,
            text: plain(runs).trim().to_string(),
            anchor: anchor.to_string(),
            row: self.out.rows.len(),
            line,
        });
        let pieces = pieces(runs, look);
        let rows = wrap(&pieces, self.room());
        let longest = rows
            .iter()
            .map(|row| row.iter().map(Piece::width).sum::<usize>())
            .max()
            .unwrap_or(0);
        for row in rows {
            self.emit(row, line);
        }
        // The two top levels are underlined, the way a setext heading is
        // written: the rule is what makes a chapter read as one from across
        // the screen, where bold alone is only a slightly brighter line.
        let rule = match level {
            1 => Some(("━", Tone::Heading(1), self.room())),
            2 => Some(("─", Tone::Rule, self.room().min(longest.max(1) + 4))),
            _ => None,
        };
        if let Some((glyph, tone, width)) = rule {
            self.emit(
                vec![Piece::new(glyph.repeat(width), Look::tone(tone))],
                line,
            );
        }
    }

    fn list(&mut self, start: Option<u64>, items: &[Item], line: usize) {
        // A list is tight when none of its items holds a paragraph of its own,
        // and only a loose list puts a blank row between its items — which is
        // CommonMark's rule, and the reason a list someone spaced out by hand
        // still looks spaced out.
        let tight = items.iter().all(|item| {
            !item.blocks.iter().any(|block| {
                matches!(
                    block,
                    Block::Paragraph {
                        implicit: false,
                        ..
                    }
                )
            })
        });
        let last = start.map(|first| first + items.len().saturating_sub(1) as u64);
        let digits = last.map_or(0, |last| last.to_string().len());
        let bullet = ["•", "◦", "▪"][self.lists % 3];
        self.lists += 1;
        for (index, item) in items.iter().enumerate() {
            let line = item.blocks.first().map_or(line, Block::line);
            if index > 0 && !tight {
                self.blank();
            }
            let (marker, tone) = match (item.task, start) {
                (Some(true), _) => ("☑ ".to_string(), Tone::Done),
                (Some(false), _) => ("☐ ".to_string(), Tone::Marker),
                (None, Some(first)) => {
                    let number = first + index as u64;
                    (format!("{number:>digits$}. "), Tone::Marker)
                }
                (None, None) => (format!("{bullet} "), Tone::Marker),
            };
            let width = marker.width();
            self.levels.push(Level {
                first: vec![Piece::new(marker, Look::tone(tone))],
                rest: vec![Piece::new(" ".repeat(width), Look::default())],
                used: false,
            });
            if item.blocks.is_empty() {
                self.emit(Vec::new(), line);
            }
            self.blocks(&item.blocks, tight);
            self.levels.pop();
        }
        self.lists -= 1;
    }

    fn code(&mut self, lang: Option<&str>, lines: &[String], line: usize) {
        let room = self.room();
        // A column of the code ground either side, so the text does not touch
        // the edge of its own block.
        let inner = room.saturating_sub(2).max(1);
        let ground = Look {
            code: true,
            ..Look::tone(Tone::Code)
        };
        if let Some(lang) = lang.filter(|lang| !lang.is_empty()) {
            let label = Look {
                code: true,
                italic: true,
                ..Look::tone(Tone::Dim)
            };
            let pad = room.saturating_sub(lang.width() + 1);
            let pieces = vec![
                Piece::new(" ".repeat(pad), ground),
                Piece::new(lang.to_string(), label),
            ];
            let indent = self.indent() as u16;
            self.emit(pieces, line).fill = Some(indent);
        }
        let coloured = lang.and_then(|lang| highlight(lang, lines));
        for (offset, text) in lines.iter().enumerate() {
            let pieces: Vec<Piece> = match coloured.as_ref().and_then(|c| c.get(offset)) {
                Some(runs) => runs
                    .iter()
                    .map(|(text, kind)| {
                        Piece::new(
                            expand_tabs(text),
                            Look {
                                code: true,
                                ..Look::tone(Tone::Syntax(*kind))
                            },
                        )
                    })
                    .collect(),
                None => vec![Piece::new(expand_tabs(text), ground)],
            };
            for row in chop(&pieces, inner) {
                let mut padded = vec![Piece::new(" ", ground)];
                padded.extend(row);
                let indent = self.indent() as u16;
                self.emit(padded, line + 1 + offset).fill = Some(indent);
            }
        }
    }

    fn table(
        &mut self,
        aligns: &[Align],
        header: &[Vec<Run>],
        rows: &[Vec<Vec<Run>>],
        line: usize,
    ) {
        let columns = rows
            .iter()
            .map(Vec::len)
            .chain([header.len(), aligns.len()])
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return;
        }
        let head_look = Look {
            bold: true,
            ..Look::tone(Tone::TableHead)
        };
        let cell = |runs: Option<&Vec<Run>>, look: Look| -> Vec<Piece> {
            runs.map_or_else(Vec::new, |runs| pieces(runs, look))
        };
        let head: Vec<Vec<Piece>> = (0..columns)
            .map(|c| cell(header.get(c), head_look))
            .collect();
        let body: Vec<Vec<Vec<Piece>>> = rows
            .iter()
            .map(|row| {
                (0..columns)
                    .map(|c| cell(row.get(c), Look::default()))
                    .collect()
            })
            .collect();

        // Each column as wide as its widest cell, then the widest columns
        // narrowed a cell at a time until the table fits. Narrowing the widest
        // first is what keeps a column of short codes readable beside a
        // column of sentences: the sentences wrap, the codes do not.
        let mut widths = vec![1usize; columns];
        for row in std::iter::once(&head).chain(body.iter()) {
            for (c, pieces) in row.iter().enumerate() {
                let width = pieces.iter().map(Piece::width).sum::<usize>();
                widths[c] = widths[c].max(width);
            }
        }
        let frame = 3 * columns + 1;
        let room = self.room();
        let available = room.saturating_sub(frame);
        while widths.iter().sum::<usize>() > available {
            let (widest, width) = widths
                .iter()
                .copied()
                .enumerate()
                .max_by_key(|(_, width)| *width)
                .unwrap_or((0, 0));
            if width <= 3 {
                break;
            }
            widths[widest] = width - 1;
        }

        let border = Look::tone(Tone::Border);
        let rule = |left: &str, mid: &str, right: &str| -> Vec<Piece> {
            let inner: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            vec![Piece::new(
                format!("{left}{}{right}", inner.join(mid)),
                border,
            )]
        };
        let fits = |pieces: Vec<Piece>| -> Vec<Piece> {
            chop(&pieces, room).into_iter().next().unwrap_or_default()
        };
        let top = fits(rule("┌", "┬", "┐"));
        self.emit(top, line);
        let header_line = fits(rule("├", "┼", "┤"));
        let bottom = fits(rule("└", "┴", "┘"));
        let rows_of = |cells: &[Vec<Piece>]| -> Vec<Vec<Piece>> {
            let wrapped: Vec<Vec<Vec<Piece>>> = cells
                .iter()
                .zip(&widths)
                .map(|(pieces, width)| wrap(pieces, *width))
                .collect();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
            (0..height)
                .map(|r| {
                    let mut out = vec![Piece::new("│", border)];
                    for (c, lines) in wrapped.iter().enumerate() {
                        let content = lines.get(r).cloned().unwrap_or_default();
                        let used: usize = content.iter().map(Piece::width).sum();
                        let spare = widths[c].saturating_sub(used);
                        let (before, after) = match aligns.get(c).copied().unwrap_or_default() {
                            Align::Left => (0, spare),
                            Align::Right => (spare, 0),
                            Align::Centre => (spare / 2, spare - spare / 2),
                        };
                        out.push(Piece::new(" ".repeat(before + 1), Look::default()));
                        out.extend(content);
                        out.push(Piece::new(" ".repeat(after + 1), Look::default()));
                        out.push(Piece::new("│", border));
                    }
                    out
                })
                .collect()
        };
        for row in rows_of(&head) {
            let row = fits(row);
            self.emit(row, line);
        }
        self.emit(header_line, line);
        for (index, cells) in body.iter().enumerate() {
            for row in rows_of(cells) {
                let row = fits(row);
                self.emit(row, line + 2 + index);
            }
        }
        self.emit(bottom, line + 2 + body.len().saturating_sub(1));
    }

    /// The picture an image names, read and scaled to the room there is —
    /// `None` for one that cannot be drawn: a remote one, a missing file, a
    /// format the decoder does not read.
    fn scaled(&mut self, image: &ImageRef) -> Option<Picture> {
        let room = self.room();
        let images = &mut *self.images;
        local_path(&image.url, self.base).and_then(|path| {
            images
                .entry(path.clone())
                .or_insert_with(|| load(&path))
                .as_ref()
                .map(|picture| scale(picture, room))
        })
    }

    /// What stands in for a picture that cannot be drawn. The alt text is what
    /// the writer said the picture shows, and it is still a link, so a click
    /// can open what the preview could not draw.
    fn stand_in(image: &ImageRef) -> Vec<Piece> {
        let text = if image.alt.is_empty() {
            image.url.clone()
        } else {
            image.alt.clone()
        };
        let look = Look {
            underline: true,
            ..Look::tone(Tone::Image)
        };
        vec![
            Piece::new("▨ ", Look::tone(Tone::Image)),
            Piece {
                text,
                look,
                link: Some(image.link),
            },
        ]
    }

    /// Draws a picture on rows of its own, with its alt text as the caption.
    fn draw_picture(&mut self, picture: Picture, image: &ImageRef, line: usize) {
        let index = self.out.pictures.len();
        let height = picture.height;
        self.out.pictures.push(picture);
        for row in 0..height {
            let x = self.indent() as u16;
            self.emit(Vec::new(), line).picture = Some(Slice {
                picture: index,
                row,
                x,
            });
        }
        if !image.alt.is_empty() {
            let caption = Piece {
                text: image.alt.clone(),
                look: Look {
                    italic: true,
                    ..Look::tone(Tone::Dim)
                },
                link: Some(image.link),
            };
            for row in wrap(&[caption], self.room()) {
                self.emit(row, line);
            }
        }
    }

    /// A paragraph of pictures. The ones that cannot be drawn share a line,
    /// the way a row of badges does on the page it was written for; one that
    /// can is drawn on rows of its own.
    fn pictures(&mut self, images: &[ImageRef], line: usize) {
        let mut pending: Vec<Piece> = Vec::new();
        for image in images {
            match self.scaled(image) {
                Some(picture) => {
                    self.flush_stand_ins(&mut pending, line);
                    self.draw_picture(picture, image, line);
                }
                None => {
                    if !pending.is_empty() {
                        pending.push(Piece::new("  ", Look::default()));
                    }
                    pending.extend(Self::stand_in(image));
                }
            }
        }
        self.flush_stand_ins(&mut pending, line);
    }

    fn flush_stand_ins(&mut self, pending: &mut Vec<Piece>, line: usize) {
        if pending.is_empty() {
            return;
        }
        for row in wrap(&std::mem::take(pending), self.room()) {
            self.emit(row, line);
        }
    }
}

/// The pieces a list of runs is drawn as, on top of `base`.
fn pieces(runs: &[Run], base: Look) -> Vec<Piece> {
    runs.iter()
        .map(|run| {
            let emph = run.emph;
            let mut look = base;
            look.bold |= emph.bold;
            look.italic |= emph.italic;
            look.strike |= emph.strike;
            let mut text = run.text.clone();
            if emph.code {
                look.tone = Tone::Code;
                look.code = true;
            }
            if emph.footnote {
                look.tone = Tone::Footnote;
            }
            if emph.link.is_some() {
                look.tone = Tone::Link;
                look.underline = true;
            }
            if emph.image {
                look.tone = Tone::Image;
                text = format!("▨ {text}");
            }
            Piece {
                text,
                look,
                link: emph.link,
            }
        })
        .collect()
}

/// Joins neighbours that look the same and go to the same place, so a row is
/// as few spans as it can be.
fn merge(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        if piece.text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.look == piece.look && last.link == piece.link => {
                last.text.push_str(&piece.text);
            }
            _ => out.push(piece),
        }
    }
    out
}

/// One unit of wrapping: a word, a run of spaces, or a line break.
enum Atom {
    Word(Piece),
    Space(Piece),
    Break,
}

fn atoms(pieces: &[Piece]) -> Vec<Atom> {
    let mut atoms = Vec::new();
    for piece in pieces {
        if piece.text == "\n" {
            atoms.push(Atom::Break);
            continue;
        }
        let mut current = String::new();
        let mut spaces = false;
        let flush = |text: &mut String, spaces: bool, atoms: &mut Vec<Atom>| {
            if text.is_empty() {
                return;
            }
            let part = Piece {
                text: std::mem::take(text),
                look: piece.look,
                link: piece.link,
            };
            atoms.push(if spaces {
                Atom::Space(part)
            } else {
                Atom::Word(part)
            });
        };
        for ch in piece.text.chars() {
            let is_space = ch == ' ' || ch == '\t';
            if is_space != spaces {
                flush(&mut current, spaces, &mut atoms);
                spaces = is_space;
            }
            current.push(if ch == '\t' { ' ' } else { ch });
        }
        flush(&mut current, spaces, &mut atoms);
    }
    atoms
}

/// Word-wraps pieces into rows no wider than `width`. A word wider than the
/// whole row is broken where it reaches the edge, which is the only way a URL
/// in a narrow pane is shown at all.
pub fn wrap(pieces: &[Piece], width: usize) -> Vec<Vec<Piece>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<Piece>> = Vec::new();
    let mut row: Vec<Piece> = Vec::new();
    let mut row_width = 0;
    let mut gap: Vec<Piece> = Vec::new();
    let mut word: Vec<Piece> = Vec::new();

    fn sum(pieces: &[Piece]) -> usize {
        pieces.iter().map(Piece::width).sum()
    }

    let place_word = |row: &mut Vec<Piece>,
                      row_width: &mut usize,
                      rows: &mut Vec<Vec<Piece>>,
                      gap: &mut Vec<Piece>,
                      word: &mut Vec<Piece>| {
        if word.is_empty() {
            return;
        }
        let word_width = sum(word);
        let gap_width = if row.is_empty() { 0 } else { sum(gap) };
        if *row_width + gap_width + word_width <= width {
            if !row.is_empty() {
                row.append(gap);
            }
            *row_width += gap_width + word_width;
            row.append(word);
        } else if word_width <= width {
            rows.push(std::mem::take(row));
            *row_width = word_width;
            row.append(word);
        } else {
            if !row.is_empty() && *row_width + gap_width < width {
                row.append(gap);
                *row_width += gap_width;
            } else if !row.is_empty() {
                rows.push(std::mem::take(row));
                *row_width = 0;
            }
            for piece in word.drain(..) {
                for ch in piece.text.chars() {
                    let cw = ch.width().unwrap_or(0);
                    if *row_width + cw > width && *row_width > 0 {
                        rows.push(std::mem::take(row));
                        *row_width = 0;
                    }
                    row.push(Piece {
                        text: ch.to_string(),
                        look: piece.look,
                        link: piece.link,
                    });
                    *row_width += cw;
                }
            }
        }
        gap.clear();
    };

    for atom in atoms(pieces) {
        match atom {
            Atom::Word(piece) => word.push(piece),
            Atom::Space(piece) => {
                place_word(&mut row, &mut row_width, &mut rows, &mut gap, &mut word);
                gap.push(piece);
            }
            Atom::Break => {
                place_word(&mut row, &mut row_width, &mut rows, &mut gap, &mut word);
                rows.push(std::mem::take(&mut row));
                row_width = 0;
                gap.clear();
            }
        }
    }
    place_word(&mut row, &mut row_width, &mut rows, &mut gap, &mut word);
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows.into_iter().map(merge).collect()
}

/// Cuts pieces into rows of exactly `width` columns, spaces and all — the
/// wrapping code wants, where a space is part of the text.
pub fn chop(pieces: &[Piece], width: usize) -> Vec<Vec<Piece>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row: Vec<Piece> = Vec::new();
    let mut used = 0;
    for piece in pieces {
        for ch in piece.text.chars() {
            let cw = ch.width().unwrap_or(0);
            if used + cw > width && used > 0 {
                rows.push(merge(std::mem::take(&mut row)));
                used = 0;
            }
            row.push(Piece {
                text: ch.to_string(),
                look: piece.look,
                link: piece.link,
            });
            used += cw;
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(merge(row));
    }
    rows
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}

/// A code block's lines coloured by the grammar its fence names, or `None` when
/// no grammar answers to it — the same grammars the text view uses, so a
/// `rust` block in the preview is the colour a `.rs` file is.
fn highlight(lang: &str, lines: &[String]) -> Option<Vec<Vec<(String, StyleKind)>>> {
    let set = syntax_set();
    let syntax = set
        .find_syntax_by_token(lang)
        .or_else(|| set.find_syntax_by_extension(&lang.to_lowercase()))?;
    if syntax.name == "Plain Text" {
        return None;
    }
    let mut state = LineState::new(syntax);
    let mut tokens: Vec<Token> = Vec::new();
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let fed = format!("{line}\n");
        state.highlight(&fed, line.len(), &mut tokens).ok()?;
        let mut start = 0;
        let mut runs = Vec::new();
        for token in &tokens {
            let end = token.end.min(line.len());
            if end > start {
                if let Some(text) = line.get(start..end) {
                    runs.push((text.to_string(), token.kind));
                }
                start = end;
            }
        }
        if start < line.len() {
            runs.push((line[start..].to_string(), StyleKind::Text));
        }
        out.push(runs);
    }
    Some(out)
}

/// The file a picture's URL names, when it names one on this machine.
pub fn local_path(url: &str, base: Option<&Path>) -> Option<PathBuf> {
    if url.is_empty()
        || url.contains("://")
        || url.starts_with("data:")
        || url.starts_with("mailto:")
    {
        return None;
    }
    let url = url.split(['#', '?']).next().unwrap_or(url);
    if url.is_empty() {
        return None;
    }
    let url = url.replace("%20", " ");
    let path = Path::new(&url);
    Some(match base {
        Some(base) if path.is_relative() => base.join(path),
        _ => path.to_path_buf(),
    })
}

fn load(path: &Path) -> Option<Image> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > PICTURE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    decode(&bytes).ok()
}

/// The picture shrunk to fit `room` columns and `PICTURE_ROWS` rows, keeping
/// its shape: a cell is two pixels tall and one wide, so a square picture is
/// twice as many columns as rows. Never enlarged — a sixteen-pixel icon is
/// drawn sixteen cells wide, not stretched to the measure.
fn scale(image: &Image, room: usize) -> Picture {
    let (iw, ih) = (image.width.max(1) as f64, image.height.max(1) as f64);
    let mut width = (room as f64).min(iw).max(1.0);
    let mut rows = (ih * width / iw / 2.0).ceil().max(1.0);
    if rows > f64::from(PICTURE_ROWS) {
        rows = f64::from(PICTURE_ROWS);
        width = (iw * rows * 2.0 / ih).round().clamp(1.0, room as f64);
    }
    let (width, height) = (width as u16, rows as u16);
    let step_x = iw / f64::from(width);
    let step_y = ih / f64::from(height * 2);
    let mut cells = Vec::with_capacity(width as usize * height as usize);
    for cy in 0..height {
        for cx in 0..width {
            let x = f64::from(cx) * step_x;
            let top = f64::from(cy * 2) * step_y;
            cells.push(Cell {
                top: average(image, x, top, step_x, step_y),
                bottom: average(image, x, top + step_y, step_x, step_y),
            });
        }
    }
    Picture {
        width,
        height,
        cells,
    }
}

/// The mean colour of a box of the picture, sampled at most eight by eight.
fn average(image: &Image, x: f64, y: f64, w: f64, h: f64) -> [u8; 3] {
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = ((x + w).ceil() as u32).clamp(x0 + 1, image.width.max(x0 + 1));
    let y1 = ((y + h).ceil() as u32).clamp(y0 + 1, image.height.max(y0 + 1));
    let sx = ((x1 - x0) / 8).max(1) as usize;
    let sy = ((y1 - y0) / 8).max(1) as usize;
    let mut sum = [0u32; 3];
    let mut count = 0;
    for py in (y0..y1).step_by(sy) {
        for px in (x0..x1).step_by(sx) {
            let pixel = image.pixel(px, py);
            for (total, channel) in sum.iter_mut().zip(pixel) {
                *total += u32::from(channel);
            }
            count += 1;
        }
    }
    let count = count.max(1);
    [
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(text: &str, width: usize) -> Rendered {
        layout(&Markdown::read(text), width, None, &mut HashMap::new())
    }

    fn lines(rendered: &Rendered) -> Vec<String> {
        rendered
            .rows
            .iter()
            .map(|row| row.pieces.iter().map(|p| p.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn paragraphs_wrap_at_words() {
        let out = rendered("one two three four five six\n", 10);
        assert_eq!(lines(&out), vec!["one two", "three four", "five six"]);
    }

    #[test]
    fn a_word_wider_than_the_row_is_broken() {
        let out = rendered("abcdefghijkl\n", 5);
        assert_eq!(lines(&out), vec!["abcde", "fghij", "kl"]);
    }

    #[test]
    fn headings_are_underlined_and_in_the_outline() {
        let out = rendered("# Top\n\nbody\n\n## Sub\n\n### Deep\n", 20);
        let text = lines(&out);
        assert_eq!(text[0], "Top");
        assert_eq!(text[1], "━".repeat(20));
        assert_eq!(text[2], "");
        assert_eq!(text[3], "body");
        assert_eq!(text[5], "Sub");
        assert_eq!(text[6], "─".repeat(7));
        let outline: Vec<_> = out
            .outline
            .iter()
            .map(|h| (h.level, h.text.as_str(), h.row))
            .collect();
        assert_eq!(outline, vec![(1, "Top", 0), (2, "Sub", 5), (3, "Deep", 8)]);
    }

    #[test]
    fn lists_hang_their_wrapped_lines() {
        let out = rendered("- alpha beta gamma\n- [x] done\n\n3. x\n4. y\n", 10);
        assert_eq!(
            lines(&out),
            vec!["• alpha", "  beta", "  gamma", "☑ done", "", "3. x", "4. y"]
        );
    }

    #[test]
    fn nested_lists_change_their_bullet() {
        let out = rendered("- a\n  - b\n", 20);
        assert_eq!(lines(&out), vec!["• a", "  ◦ b"]);
    }

    #[test]
    fn quotes_carry_their_bar_through_blank_rows() {
        let out = rendered("> one\n>\n> two\n", 20);
        assert_eq!(lines(&out), vec!["▎ one", "▎ ", "▎ two"]);
        assert!(out.rows[0].pieces[1].look.italic);
    }

    #[test]
    fn alerts_are_labelled() {
        let out = rendered("> [!TIP]\n> do this\n", 20);
        assert_eq!(lines(&out), vec!["▎ Tip", "▎ do this"]);
        assert_eq!(out.rows[0].pieces[0].look.tone, Tone::Alert(Alert::Tip));
    }

    #[test]
    fn tables_are_framed_and_aligned() {
        let out = rendered("| a | bb |\n|---|---:|\n| 1 | 2 |\n", 40);
        assert_eq!(
            lines(&out),
            vec![
                "┌───┬────┐",
                "│ a │ bb │",
                "├───┼────┤",
                "│ 1 │  2 │",
                "└───┴────┘",
            ]
        );
    }

    #[test]
    fn a_wide_table_wraps_its_widest_column() {
        let out = rendered("| k | v |\n|---|---|\n| x | one two three |\n", 16);
        assert!(
            lines(&out).iter().all(|line| line.width() <= 16),
            "{:?}",
            lines(&out)
        );
        assert!(lines(&out).iter().any(|line| line.contains("three")));
    }

    #[test]
    fn code_blocks_fill_their_ground_and_name_the_language() {
        let out = rendered("```rust\nfn main() {}\n```\n", 30);
        let text = lines(&out);
        assert!(text[0].ends_with("rust"));
        assert_eq!(text[1], " fn main() {}");
        assert!(out.rows.iter().all(|row| row.fill == Some(0)));
        assert!(out.rows[1]
            .pieces
            .iter()
            .any(|piece| piece.look.tone == Tone::Syntax(StyleKind::Keyword)));
    }

    #[test]
    fn a_missing_picture_is_its_alt_text_and_a_link() {
        let out = rendered("![the logo](nowhere.png)\n", 30);
        assert_eq!(lines(&out), vec!["▨ the logo"]);
        assert_eq!(
            out.links[out.rows[0].pieces[1].link.unwrap()],
            "nowhere.png"
        );
    }

    #[test]
    fn a_picture_is_drawn_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        // A 4×4 red PNG, written by the encoder the tests already have.
        let path = dir.path().join("red.png");
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = png::Encoder::new(file, 4, 4);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 0, 0].repeat(16)).unwrap();
        drop(writer);
        let doc = Markdown::read("![red](red.png)\n");
        let out = layout(&doc, 40, Some(dir.path()), &mut HashMap::new());
        assert_eq!(out.pictures.len(), 1);
        let picture = &out.pictures[0];
        assert_eq!((picture.width, picture.height), (4, 2));
        assert_eq!(picture.cell(0, 0).top, [255, 0, 0]);
        assert_eq!(out.rows[0].picture.unwrap().row, 0);
        assert_eq!(lines(&out).last().unwrap(), "red");
    }

    #[test]
    fn rows_remember_their_source_line() {
        let out = rendered("a\n\nb\n\n```\nx\ny\n```\n", 20);
        let rows: Vec<usize> = out.rows.iter().map(|row| row.line).collect();
        assert_eq!(rows, vec![0, 0, 2, 2, 5, 6]);
    }

    #[test]
    fn geometry_drops_the_outline_from_a_narrow_pane() {
        assert_eq!(geometry(60, true).outline, None);
        let wide = geometry(120, true);
        assert_eq!(wide.outline, Some((90, 30)));
        assert_eq!(wide.track_x, 89);
        assert_eq!(wide.text_width, 85);
        assert_eq!(geometry(300, false).text_width, MEASURE);
    }

    fn view(text: &str) -> MarkdownView {
        let mut view = MarkdownView::new(0);
        view.doc = Markdown::read(text);
        view.rendered = layout(&view.doc, 40, None, &mut HashMap::new());
        view
    }

    #[test]
    fn headings_are_stepped_through_and_jumped_to() {
        let text = "# A\n\n".to_string() + &"x\n\n".repeat(30) + "## B\n\n" + &"y\n\n".repeat(30);
        let mut view = view(&text);
        assert_eq!(view.current_heading(), Some(0));
        assert_eq!(view.step_heading(1), Some(1));
        assert_eq!(view.top, view.rendered.outline[1].row);
        assert_eq!(view.current_heading(), Some(1));
        assert_eq!(view.step_heading(1), None);
        assert_eq!(view.step_heading(-1), Some(0));
        assert_eq!(view.top, 0);
        assert_eq!(view.heading_for_anchor("b"), Some(1));
    }

    #[test]
    fn a_new_width_keeps_the_heading_at_the_top() {
        let text = "# A\n\n".to_string() + &"long words here\n\n".repeat(30) + "## B\n\nend\n";
        let document = Document::from_text(&text, None);
        let mut view = MarkdownView::new(0);
        view.sync(&document, 120);
        view.jump_to_heading(1);
        // Hiding the outline widens the text, which is a new layout.
        view.outline = false;
        view.sync(&document, 120);
        assert_eq!(view.current_heading(), Some(1));
        assert_eq!(view.top, view.rendered.outline[1].row);
    }

    #[test]
    fn a_link_is_found_under_its_column() {
        let view = view("go [here](#b) now\n");
        assert_eq!(view.link_at(0, 0), None);
        assert_eq!(view.link_at(0, 3), Some("#b"));
        assert_eq!(view.link_at(0, 6), Some("#b"));
        assert_eq!(view.link_at(0, 8), None);
    }

    #[test]
    fn local_paths_resolve_against_the_document() {
        let base = Path::new("/docs");
        assert_eq!(
            local_path("img/a%20b.png#x", Some(base)),
            Some(PathBuf::from("/docs/img/a b.png"))
        );
        assert_eq!(local_path("https://x.dev/a.png", Some(base)), None);
        assert_eq!(local_path("#top", Some(base)), None);
    }
}
