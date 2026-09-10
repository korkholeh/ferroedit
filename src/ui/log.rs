//! UI: the read-only log viewer (ADR-068).
//!
//! The editor pane, drawn for a tab that holds a history instead of a document:
//! one row per commit, in four columns — the abbreviated name, the date, the
//! author and the subject. The columns are what make a history scannable, and
//! they are laid out against the widest value on screen so that a list of
//! commits by one person does not reserve half the pane for a name.
//!
//! Nothing here decides *which* commits: the list arrives already read and
//! already filtered (ARCHITECTURE invariant 4), so this file decides columns
//! and colours and nothing else.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::focus::FocusTarget;
use crate::app::log::LogState;
use crate::app::App;
use crate::commands::Command;
use crate::event::keyboard;
use crate::ui::explorer::selection_style;
use crate::ui::theme::Theme;
use crate::ui::{field, scrollbar};

const ELLIPSIS: char = '…';

/// What separates two pieces of the legend on the bottom border.
const SEPARATOR: &str = "  ·  ";

/// The most cells the author column takes, however long the names are. Past it
/// a name is cut: the subject is what a reader is scanning for.
const MAX_AUTHOR: usize = 18;

/// `2026-09-10`, plus the two spaces after every column.
const DATE_WIDTH: usize = 10;
const GAP: usize = 2;

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(log) = app.log() else {
        return;
    };
    let focused = matches!(app.focus, FocusTarget::Log | FocusTarget::LogSearch);
    // The editor is drawn underneath, so the ground has to be taken back
    // before anything is written on it.
    frame.render_widget(Clear, area);
    let position = log.position();
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.border_for(focused))
        .style(Style::new().bg(theme.background))
        .title(Span::styled(log.title(), theme.panel_title))
        .title_bottom(
            Line::from(Span::styled(position.clone(), Style::new().fg(theme.dim))).right_aligned(),
        );
    // The keys, on the bottom border beside the readout (ADR-071). The pane's
    // own keys are bare letters, which the menu deliberately does not
    // advertise, so this is where a reader finds out about them.
    let room = (area.width as usize).saturating_sub(position.width() + 2);
    let legend = legend(log, room, theme);
    if !legend.is_empty() {
        block = block.title_bottom(Line::from(legend).left_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // The field takes the top row of the pane while it is open, the way the
    // browser's filter takes the top of its box (ADR-051).
    let rows = match search_row(app, inner) {
        Some(row) => {
            render_field(frame, app, log, row, theme);
            Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1)
        }
        None => inner,
    };
    if rows.height == 0 {
        return;
    }

    if let Some(message) = empty_message(log) {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {message}"),
                Style::new().fg(theme.dim),
            ))),
            rows,
        );
        return;
    }

    scrollbar::render(
        frame,
        Rect::new(rows.right() - 1, rows.y, 1, rows.height),
        theme,
        focused,
        log.len(),
        rows.height as usize,
        log.scroll,
    );
    // One column of the pane is the scrollbar's, as it is everywhere else.
    let width = (rows.width as usize).saturating_sub(1);
    let widths = Columns::of(log, width);

    let lines: Vec<Line> = log
        .rows()
        .enumerate()
        .skip(log.scroll)
        .take(rows.height as usize)
        .map(|(index, commit)| {
            if index == log.selected() {
                // The selected row is one span, so the highlight covers the
                // gaps between the columns as well as the columns.
                Line::from(Span::styled(
                    widths.row(commit),
                    selection_style(theme, focused),
                ))
            } else {
                Line::from(widths.spans(commit, theme))
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows);
}

/// The keys of the pane, as spans for its bottom border.
///
/// Read out of the keymap rather than written here, so the legend cannot
/// advertise a key that is not bound — the same rule the menu's shortcut
/// column follows (SPEC §25, ADR-028). Pieces are dropped from the end while
/// they do not fit, so a narrow terminal keeps the first and most useful ones
/// instead of losing the line altogether.
fn legend(log: &LogState, room: usize, theme: &Theme) -> Vec<Span<'static>> {
    let key = |command: &Command| keyboard::binding_for(command).map(|b| b.label);
    // The arrows are the one pair not looked up: they are their own label, and
    // `Up / Down` spelled out is three times the width for no more meaning.
    let mut pieces: Vec<(&'static str, &'static str)> = if log.searching {
        vec![
            ("↑↓", "move"),
            (key(&Command::LogSearchSubmit).unwrap_or(""), "search git"),
            (key(&Command::LogSearchClose).unwrap_or(""), "cancel"),
        ]
    } else {
        vec![
            ("↑↓", "move"),
            (key(&Command::LogShowCommit).unwrap_or(""), "diff"),
            (key(&Command::LogSearchOpen).unwrap_or(""), "search"),
            (key(&Command::LogRefresh).unwrap_or(""), "refresh"),
            (key(&Command::LogClose).unwrap_or(""), "close"),
        ]
    };
    pieces.retain(|(key, _)| !key.is_empty());

    let width = |pieces: &[(&str, &str)]| {
        pieces
            .iter()
            .map(|(key, what)| key.width() + 1 + what.width())
            .sum::<usize>()
            + SEPARATOR.width() * pieces.len().saturating_sub(1)
            // A space at each end, so the text does not touch the corners.
            + 2
    };
    while !pieces.is_empty() && width(&pieces) > room {
        pieces.pop();
    }
    if pieces.is_empty() {
        return Vec::new();
    }

    let mut spans = vec![Span::raw(" ")];
    for (index, (key, what)) in pieces.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(SEPARATOR, Style::new().fg(theme.dim)));
        }
        spans.push(Span::styled(key, Style::new().fg(theme.diff_hunk)));
        spans.push(Span::styled(format!(" {what}"), Style::new().fg(theme.dim)));
    }
    spans.push(Span::raw(" "));
    spans
}

/// The row the search field is drawn in, when it is open.
///
/// It is the top row of the pane's inside, and it is `None` when the field is
/// closed — which is what keeps the list a row taller while nobody is typing.
pub fn search_row(app: &App, inner: Rect) -> Option<Rect> {
    let open = app.log().is_some_and(|log| log.searching);
    (open && inner.height > 1).then(|| Rect::new(inner.x, inner.y, inner.width, 1))
}

fn render_field(frame: &mut Frame, app: &App, log: &LogState, row: Rect, theme: &Theme) {
    let label = " Search: ";
    let label_width = label.width().min(row.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            label,
            Style::new().fg(theme.search_label),
        )))
        .style(theme.search_bar),
        Rect::new(row.x, row.y, label_width as u16, 1),
    );
    let rest = Rect::new(
        row.x + label_width as u16,
        row.y,
        row.width.saturating_sub(label_width as u16),
        1,
    );
    frame.render_widget(Paragraph::new("").style(theme.search_field), rest);
    field::render(
        frame,
        &log.field,
        rest,
        theme.search_field,
        app.focus == FocusTarget::LogSearch,
    );
}

/// The line drawn instead of a list, when there is no list to draw.
///
/// A history that is empty and one whose filter matches nothing are different
/// states and get different sentences: the second one is undone by a keystroke
/// and the first one is not.
fn empty_message(log: &LogState) -> Option<String> {
    if !log.is_empty() {
        return None;
    }
    if log.has_no_commits() {
        return Some(match &log.query {
            Some(query) => format!("No commit matches \"{query}\""),
            None => "No commits yet".to_string(),
        });
    }
    Some(format!(
        "Nothing here matches \"{}\" — Enter asks git for the rest of the history",
        log.field.value.trim()
    ))
}

/// The width of each column, resolved against the widest value on screen.
struct Columns {
    oid: usize,
    author: usize,
    subject: usize,
}

impl Columns {
    fn of(log: &LogState, width: usize) -> Self {
        let widest = |f: fn(&crate::git::Commit) -> &str, cap: usize| {
            log.rows().map(|c| f(c).width()).max().unwrap_or(0).min(cap)
        };
        let oid = widest(|c| c.short.as_str(), 40).max(7);
        let author = widest(|c| c.author.as_str(), MAX_AUTHOR);
        // Everything the three fixed columns and the four gaps around them did
        // not take — one in front of the row and one between each pair. It can
        // be nothing at all in a very narrow pane, and `fit` answers with an
        // empty string rather than panicking.
        let subject = width.saturating_sub(oid + DATE_WIDTH + author + GAP * 4);
        Self {
            oid,
            author,
            subject,
        }
    }

    /// The whole row as one string — what the selected row is drawn as.
    fn row(&self, commit: &crate::git::Commit) -> String {
        let gap = " ".repeat(GAP);
        format!(
            "{gap}{}{gap}{}{gap}{}{gap}{}",
            fit(&commit.short, self.oid),
            fit(&commit.date, DATE_WIDTH),
            fit(&commit.author, self.author),
            fit(&commit.subject, self.subject),
        )
    }

    /// The same row in colour, for every row that is not the selected one.
    ///
    /// The colours are borrowed rather than new: the abbreviated name is the
    /// locator of a commit the way `@@ … @@` is the locator of a hunk, and the
    /// author is a name the way a directory is.
    fn spans(&self, commit: &crate::git::Commit, theme: &Theme) -> Vec<Span<'static>> {
        let gap = || Span::raw(" ".repeat(GAP));
        vec![
            gap(),
            Span::styled(
                fit(&commit.short, self.oid),
                Style::new().fg(theme.diff_hunk),
            ),
            gap(),
            Span::styled(fit(&commit.date, DATE_WIDTH), Style::new().fg(theme.dim)),
            gap(),
            Span::styled(
                fit(&commit.author, self.author),
                Style::new().fg(theme.directory),
            ),
            gap(),
            Span::styled(
                fit(&commit.subject, self.subject),
                Style::new().fg(theme.foreground),
            ),
        ]
    }
}

/// A value in exactly `width` cells: padded when it is short, cut with an
/// ellipsis when it is long.
///
/// Cells rather than characters, so a Cyrillic or CJK name lines up with the
/// column beside it — the same rule the CSV grid follows.
fn fit(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if value.width() <= width {
        return format!("{value}{}", " ".repeat(width - value.width()));
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in value.chars() {
        let cell = ch.to_string().width();
        if used + cell > width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += cell;
    }
    out.push(ELLIPSIS);
    used += 1;
    // A wide character next to the ellipsis can leave the cell a column short.
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::log::LogState;
    use crate::git::{Commit, LogScope};

    fn commit(short: &str, author: &str, subject: &str) -> Commit {
        Commit {
            oid: format!("{short:0<40}"),
            short: short.into(),
            author: author.into(),
            date: "2026-09-10".into(),
            subject: subject.into(),
        }
    }

    fn log() -> LogState {
        LogState::new(
            LogScope::Repository,
            vec![
                commit("aaaaaaa", "Ada", "feat: the newest thing"),
                commit("bbbbbbb", "Grace Brewster Murray Hopper", "fix: a bug"),
            ],
            None,
        )
    }

    #[test]
    fn a_row_is_exactly_the_panes_width() {
        let log = log();
        let widths = Columns::of(&log, 60);
        for commit in log.rows() {
            assert_eq!(widths.row(commit).width(), 60, "{}", commit.short);
        }
    }

    #[test]
    fn a_long_name_is_cut_rather_than_pushing_the_subject_off() {
        let log = log();
        let widths = Columns::of(&log, 60);
        assert_eq!(widths.author, MAX_AUTHOR);
        let row = widths.row(log.rows().nth(1).unwrap());
        assert!(row.contains(ELLIPSIS), "{row}");
        assert!(row.contains("fix: a bug"), "{row}");
    }

    /// A pane too narrow for the fixed columns leaves the subject nothing, and
    /// that is a row with no subject rather than a panic.
    #[test]
    fn a_pane_with_no_room_for_a_subject_does_not_panic() {
        let log = log();
        for width in 0..30 {
            let widths = Columns::of(&log, width);
            for commit in log.rows() {
                let _ = widths.row(commit);
            }
        }
    }

    #[test]
    fn an_empty_history_and_an_unmatched_filter_say_different_things() {
        let empty = LogState::new(LogScope::Repository, Vec::new(), None);
        assert_eq!(empty_message(&empty).as_deref(), Some("No commits yet"));

        let searched = LogState::new(LogScope::Repository, Vec::new(), Some("zzz".into()));
        assert!(empty_message(&searched)
            .unwrap()
            .contains("No commit matches \"zzz\""));

        let mut filtered = log();
        filtered.field.insert_str("nothing like this");
        filtered.refilter(10);
        assert!(empty_message(&filtered).unwrap().contains("asks git"));

        assert_eq!(empty_message(&log()), None);
    }

    #[test]
    fn a_value_is_padded_or_cut_to_the_cell_it_is_given() {
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 0), "");
        // A wide character next to the ellipsis still leaves the column whole.
        assert_eq!(fit("日本語テキスト", 5).width(), 5);
    }
}
