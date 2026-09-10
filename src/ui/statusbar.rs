//! UI: status bar.
//!
//! The right-hand readout is built as a list of pieces with a drop order
//! rather than as one format string, because the bar is the one row whose
//! contents outgrow a narrow terminal. At 60 columns the fixed readout used to
//! clip the notification; now the pieces nobody is reading — the encoding, the
//! focus label — leave first and the sentence gets its room back (ADR-039).
//!
//! Most of those pieces are also questions (ADR-058): the cursor position, the
//! line ending, the encoding and the grammar each open the dialog that changes
//! them, and a table adds two more of its own — the delimiter and the quote
//! character (SPEC §65). That is why `zones` is public — `ui::layout` asks where
//! each piece landed, and the mouse hit-tests against those rects, so what is
//! drawn and what is clickable are computed by the same code and cannot drift
//! apart.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::notifications::NotificationKind;
use crate::app::table::TableView;
use crate::app::App;
use crate::commands::Command;
use crate::editor::charset::Charset;
use crate::editor::compression::Compression;
use crate::editor::document::Document;
use crate::ui::theme::Theme;

/// A piece of the readout that answers a click (ADR-058).
///
/// Every piece whose dialog is unambiguous is here: the ones that describe the
/// file, and the branch, whose picker is the one thing a branch name could
/// sensibly open. The focus label and the selection count are left out — they
/// report where you already are, and a click that took them somewhere would be
/// a guess, and so is the table's own `Row 3/128`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusZone {
    Cursor,
    Encoding,
    LineEnding,
    Language,
    Branch,
    /// The two the CSV view adds, and the only two that come and go with what
    /// the pane is showing (SPEC §65): a file read as text has no delimiter to
    /// ask about.
    CsvDelimiter,
    CsvQuote,
}

impl StatusZone {
    /// What clicking this piece runs. Each one is the dialog that changes the
    /// thing the piece reports, and each is also in a menu: a feature reachable
    /// only with a mouse is one a terminal user does not have.
    pub fn command(self) -> Command {
        match self {
            Self::Cursor => Command::GotoLinePrompt,
            Self::Encoding => Command::EncodingPrompt,
            Self::LineEnding => Command::LineEndingPrompt,
            Self::Language => Command::LanguagePrompt,
            Self::Branch => Command::GitBranchPrompt,
            Self::CsvDelimiter => Command::CsvDelimiterPrompt,
            Self::CsvQuote => Command::CsvQuotePrompt,
        }
    }
}

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    frame.render_widget(Block::new().style(theme.status_bar), area);

    let left = left(app);
    // The readout gives way to the sentence beside it, not to a constant: a
    // notification longer than the floor is still the thing the user has to
    // read, and the readout is the same on almost every file (ADR-039).
    let text = joined(&readout(app, area.width, left.width()));

    // The two halves are separate widgets over one row, so the row is split
    // rather than overdrawn: on a narrow terminal the notification is clipped
    // instead of being written over by the readout.
    let right_width = (text.width() as u16).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);

    frame.render_widget(Paragraph::new(left.line(theme)), left_area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            theme.status_bar.fg(theme.chrome_dim),
        )))
        .right_aligned(),
        right_area,
    );
}

/// Where each clickable piece of the readout landed, for the mouse to hit-test
/// against (ADR-058).
///
/// It runs the same two functions `render` does, in the same order, against the
/// same area — the rects therefore describe the row that was actually drawn.
/// A readout too wide for the row yields nothing: it is clipped on screen, and
/// a click on a piece the user cannot fully see should not be a command.
pub fn zones(app: &App, area: Rect) -> Vec<(StatusZone, Rect)> {
    if area.height == 0 {
        return Vec::new();
    }
    let pieces = readout(app, area.width, left(app).width());
    let text = joined(&pieces);
    let width = text.width() as u16;
    if width > area.width {
        return Vec::new();
    }
    // The readout is right-aligned in a rect of its own width, so it starts at
    // the row's right edge less that width — plus the leading space `joined`
    // put in front of the first piece.
    let mut x = area.right() - width + 1;
    let mut zones = Vec::new();
    for piece in &pieces {
        let piece_width = piece.text.width() as u16;
        if let Some(zone) = piece.zone {
            zones.push((zone, Rect::new(x, area.y, piece_width, 1)));
        }
        x += piece_width + GAP.width() as u16;
    }
    zones
}

/// The left half is never given less than this, even when it has less to say.
/// A bar that is all readout has stopped being a status bar.
///
/// Twenty is `Opened src/main.rs` and a space at either end: the shortest
/// sentence the editor actually says about a file it has just opened. A longer
/// one asks for its own width instead.
const MIN_LEFT: u16 = 20;

/// Separator between the readout's pieces.
const GAP: &str = "   ";

/// The marker that says the buffer has unsaved changes.
const DIRTY: &str = " ●";

/// The sentence on the left: a live notification, or the tab's own name.
///
/// It is a value rather than a `Line` because its *width* is wanted twice — the
/// readout is trimmed against it, and `zones` needs that trimming without a
/// theme to build spans with.
struct Left {
    text: String,
    /// Set while a notification is showing, which is what colours the text.
    kind: Option<NotificationKind>,
    dirty: bool,
}

impl Left {
    fn width(&self) -> u16 {
        let dirty = if self.dirty { DIRTY.width() } else { 0 };
        (self.text.width() + dirty) as u16
    }

    fn line(&self, theme: &Theme) -> Line<'static> {
        let style = match self.kind {
            Some(kind) => theme.status_bar.fg(kind_color(theme, kind)),
            None => theme.status_bar,
        };
        let mut spans = vec![Span::styled(self.text.clone(), style)];
        if self.dirty {
            spans.push(Span::styled(DIRTY, theme.status_bar.fg(theme.tab_dirty)));
        }
        Line::from(spans)
    }
}

/// A live notification takes over the left half; the file/cursor readout on the
/// right stays put so it never moves under the user's eye.
fn left(app: &App) -> Left {
    match app.notifications.current() {
        Some(notification) => Left {
            text: format!(" {}", notification.message),
            kind: Some(notification.kind),
            dirty: false,
        },
        // The tab's own title, so a diff tab names the diff rather than
        // reporting no file open.
        None => Left {
            text: format!(
                " {}",
                app.active_tab
                    .and_then(|i| app.tabs.get(i))
                    .map_or_else(|| "—".to_string(), |tab| tab.title())
            ),
            kind: None,
            dirty: app.active().is_some_and(|t| t.document.is_dirty()),
        },
    }
}

/// What is left for the readout once the sentence beside it has had its room.
fn budget(width: u16, left: u16) -> usize {
    width.saturating_sub(left.max(MIN_LEFT)) as usize
}

/// A piece of the readout, how readily it goes, and what a click on it opens.
///
/// Lower drops later: the cursor position is what the bar is *for* and is never
/// dropped, and the encoding is the piece that is the same on every file the
/// MVP opens, so it is the first to go.
struct Piece {
    text: String,
    drop_order: u8,
    /// `None` for a piece that reports something this dialog-less bar cannot
    /// change.
    zone: Option<StatusZone>,
}

fn piece(text: String, drop_order: u8) -> Piece {
    Piece {
        text,
        drop_order,
        zone: None,
    }
}

fn clickable(text: String, drop_order: u8, zone: StatusZone) -> Piece {
    Piece {
        text,
        drop_order,
        zone: Some(zone),
    }
}

/// The pieces joined, with the leading and trailing spaces that keep the
/// readout off a clipped notification and off the right edge.
fn joined(pieces: &[Piece]) -> String {
    format!(
        " {} ",
        pieces
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join(GAP)
    )
}

/// Drops whole drop-order groups, least important first, until the rest fits.
///
/// The group and not the piece: two things dropped together were judged equally
/// worth keeping, and a readout that shed one of a pair would look like a bug.
/// The pieces at order zero are never dropped — on a terminal too narrow even
/// for them, the row is clipped instead, which is the honest outcome.
fn trim(mut pieces: Vec<Piece>, budget: usize) -> Vec<Piece> {
    loop {
        if joined(&pieces).width() <= budget {
            return pieces;
        }
        match pieces.iter().map(|p| p.drop_order).max() {
            Some(0) | None => return pieces,
            Some(worst) => pieces.retain(|p| p.drop_order != worst),
        }
    }
}

/// The right-hand readout, trimmed to what is left after the sentence beside
/// it has had what it needs.
fn readout(app: &App, width: u16, left: u16) -> Vec<Piece> {
    // A diff has no cursor, no encoding and no grammar, so the readout it gets
    // is the one a pager needs: where in it the window is, and which side of
    // the change it is showing (SPEC §36). Its pieces are already in importance
    // order, so their place in the list is their drop order.
    if let Some(viewer) = app.diff() {
        let pieces = vec![
            piece(viewer.position().trim().to_string(), 0),
            piece(viewer.source.label(), 1),
            clickable(app.git.branch_label().to_string(), 2, StatusZone::Branch),
            piece(app.focus.label().to_string(), 3),
        ];
        return trim(pieces, budget(width, left));
    }
    // A history is a list and not a document either: where the selection is,
    // and which history it is in (ADR-068).
    if let Some(log) = app.log() {
        let pieces = vec![
            piece(log.position().trim().to_string(), 0),
            piece(log.scope.label(), 1),
            clickable(app.git.branch_label().to_string(), 2, StatusZone::Branch),
            piece(app.focus.label().to_string(), 3),
        ];
        return trim(pieces, budget(width, left));
    }
    let document = app.active().map(|t| &t.document);
    // One-based, and counted in user-perceived characters rather than in chars
    // or cells, because that is the number a human arrives at (SPEC §38).
    let line = document.map_or(1, |d| d.cursor().line + 1);
    let column = document.map_or(1, |d| d.cursor_display_col().0);
    let table = app.active().and_then(|tab| tab.table.as_ref());
    // The grammar the highlighter actually chose, not a guess from the
    // extension: it is the one that also reads file names and shebangs, and a
    // status bar that disagreed with the colours would be worse than none.
    let language = app
        .active()
        .map_or("Plain Text", |tab| tab.highlights.language());
    // A file that is too large to colour still says what it is, with a marker
    // so that "why is this not highlighted?" has an answer on screen and not
    // only in a notification that has already expired.
    let plain = match app.active().and_then(|tab| tab.highlights.disabled()) {
        Some(_) => " (plain)",
        None => "",
    };

    // A table has no caret and no wrapping, so where the user *is* is a cell
    // and not a position, and the two questions the bar asks about it are the
    // ones the parse turns on: what splits the columns, and what quotes the
    // fields (SPEC §65, ADR-062).
    let mut pieces = match table {
        Some(view) => vec![
            // Not clickable: Go to Line has nothing to go to in a grid, and a
            // readout that opened a dialog about lines would be the bar
            // answering a question nobody asked of it.
            piece(view.position(), 0),
            clickable(
                format!("Delim {}", view.dialect.delimiter_label()),
                1,
                StatusZone::CsvDelimiter,
            ),
            clickable(
                format!("Quote {}", view.dialect.quote_label()),
                1,
                StatusZone::CsvQuote,
            ),
        ],
        None => vec![clickable(
            format!("Ln {line}, Col {column}"),
            0,
            StatusZone::Cursor,
        )],
    };
    // Shown only while something is selected: an always-present "Sel 0" would
    // cost three columns of an already crowded bar for no information. A grid
    // counts it in rows and columns, because that is the shape of what is
    // selected there (SPEC §65).
    if let Some(range) = table
        .and_then(TableView::selection)
        .filter(|range| range.is_range())
    {
        pieces.push(piece(
            format!("Sel {}×{}", range.records(), range.columns()),
            1,
        ));
    } else if let Some(count) = document
        .filter(|_| table.is_none())
        .map(|d| d.selected_len())
        .filter(|n| *n > 0)
    {
        pieces.push(piece(format!("Sel {count}"), 1));
    }
    // Shown only while it is on: it says the pane is not in the state it is in
    // by default, and a wrapped file that did not say so reads as a file full
    // of short lines.
    if app.settings.word_wrap && table.is_none() {
        pieces.push(piece("Wrap".to_string(), 2));
    }
    // What the file was packed in, and whether what is on screen is all of it
    // (ADR-074). Shown only for a file that was unpacked, and at drop order 1
    // because it is the piece that says the buffer cannot be saved back — the
    // one thing about the tab that is not true of every other tab.
    if let Some(label) = document
        .map(Document::compression)
        .and_then(Compression::label)
    {
        let cut = if document.is_some_and(Document::is_truncated) {
            " · cut"
        } else {
            ""
        };
        pieces.push(piece(format!("{label} · read-only{cut}"), 1));
    }
    // The charset the file was decoded with, which since ADR-059 is not always
    // UTF-8 — and is the piece that opens the picker that changes it.
    let charset = document.map_or(Charset::UTF8, Document::charset);
    pieces.push(clickable(
        charset.label.to_string(),
        6,
        StatusZone::Encoding,
    ));
    // Always, now that it is a question and not only a warning (ADR-058). It
    // was shown for CRLF alone while nothing could be done about it — a bar
    // that mentions the surprising case only is a bar you cannot ask "and what
    // is it here?", and the answer is what the click is for.
    if let Some(ending) = document.map(|d| d.line_ending()) {
        pieces.push(clickable(
            ending.label().to_string(),
            2,
            StatusZone::LineEnding,
        ));
    }
    // The grammar, unless the pane is a grid: nothing in a table is
    // highlighted, and a readout offering to change how it is coloured would be
    // offering something that does not happen.
    if table.is_none() {
        pieces.push(clickable(
            format!("{language}{plain}"),
            4,
            StatusZone::Language,
        ));
    }
    pieces.push(clickable(
        app.git.branch_label().to_string(),
        3,
        StatusZone::Branch,
    ));
    pieces.push(piece(app.focus.label().to_string(), 5));

    trim(pieces, budget(width, left))
}

fn kind_color(theme: &Theme, kind: NotificationKind) -> ratatui::style::Color {
    match kind {
        NotificationKind::Info => theme.info,
        NotificationKind::Warning => theme.warning,
        NotificationKind::Error => theme.error,
    }
}
