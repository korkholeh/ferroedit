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
use crate::ui::{field, legend, scrollbar};

const ELLIPSIS: char = '…';

/// The most cells the author column takes, however long the names are. Past it
/// a name is cut: the subject is what a reader is scanning for.
const MAX_AUTHOR: usize = 18;

/// The fewest it takes while it is drawn at all.
///
/// Eight cells is a first name or an initial and a surname — enough to tell
/// one committer from another, which is what the column is for. Below it the
/// column is dropped rather than shrunk further: three letters and an ellipsis
/// is a column that costs four cells to say nothing (ADR-080).
const MIN_AUTHOR: usize = 8;

/// The subject's floor, and the reason the other three columns give way.
///
/// A commit message is written to be read at fifty cells and the convention
/// caps its subject at seventy-two. Thirty is well under both — it is not
/// "enough", it is the width below which the secondary columns have stopped
/// being worth their cells, and it is what makes them go one at a time.
const MIN_SUBJECT: usize = 30;

/// `2026-09-10`, plus the two spaces in front of every column.
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
                    widths.row(commit, theme),
                    selection_style(theme, focused),
                ))
            } else {
                Line::from(widths.spans(commit, theme))
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), rows);
}

/// The keys of the pane, as spans for its bottom border (ADR-071).
///
/// The measuring and the dropping are `ui::legend`'s, which the image viewer
/// uses too; what is here is which keys this pane has.
fn legend(log: &LogState, room: usize, theme: &Theme) -> Vec<Span<'static>> {
    let key = |command: &Command| keyboard::binding_for(command).map(|b| b.label);
    // The arrows are the one pair not looked up: they are their own label, and
    // `Up / Down` spelled out is three times the width for no more meaning.
    let pieces: Vec<legend::Piece> = if log.searching {
        vec![
            ("↑↓", "move"),
            (key(&Command::LogSearchSubmit).unwrap_or(""), "search git"),
            (key(&Command::LogSearchClose).unwrap_or(""), "cancel"),
        ]
    } else {
        // In the order they are given up, least useful last: the legend drops
        // from the end, and the two keys ADR-080 added would otherwise have
        // pushed `Esc close` off a pane of ordinary width.
        vec![
            ("↑↓", "move"),
            (key(&Command::LogShowCommit).unwrap_or(""), "diff"),
            (key(&Command::LogShowMessage).unwrap_or(""), "message"),
            (
                key(&Command::LogToggleColumns).unwrap_or(""),
                if log.show_columns { "wide" } else { "columns" },
            ),
            (key(&Command::LogSearchOpen).unwrap_or(""), "search"),
            (key(&Command::LogClose).unwrap_or(""), "close"),
            (key(&Command::LogRefresh).unwrap_or(""), "refresh"),
        ]
    };
    legend::spans(&pieces, room, theme)
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

/// The width of each column, resolved against the widest value on screen and
/// against what the pane has to give (ADR-080).
///
/// Zero means the column is not drawn at all, gap included. The subject is
/// what everything else is resolved *for*: the three columns in front of it
/// locate a commit, and the subject is the only one that says what it did, so
/// it is the one that keeps its width and they are the ones that give way.
struct Columns {
    oid: usize,
    date: usize,
    author: usize,
    subject: usize,
}

impl Columns {
    fn of(log: &LogState, width: usize) -> Self {
        if !log.show_columns {
            // The whole pane, less the one gap in front of it.
            return Self {
                oid: 0,
                date: 0,
                author: 0,
                subject: width.saturating_sub(GAP),
            };
        }
        let widest = |f: fn(&crate::git::Commit) -> &str, cap: usize| {
            log.rows().map(|c| f(c).width()).max().unwrap_or(0).min(cap)
        };
        let mut oid = widest(|c| c.short.as_str(), 40).max(7);
        let mut date = DATE_WIDTH;
        let mut author = widest(|c| c.author.as_str(), MAX_AUTHOR);

        // One gap in front of every column that is drawn, the subject
        // included: a column that is gone takes its gap with it.
        let left = |oid: usize, date: usize, author: usize| {
            let drawn = 1 + usize::from(oid > 0) + usize::from(date > 0) + usize::from(author > 0);
            width.saturating_sub(oid + date + author + GAP * drawn)
        };
        // The author gives its cells back first — a cut name is still a name —
        // and then the columns go one at a time, least useful first: the date
        // before the author, and the abbreviated name last, because it is the
        // one thing on the row that names the commit to git.
        while author > MIN_AUTHOR && left(oid, date, author) < MIN_SUBJECT {
            author -= 1;
        }
        for column in 0..3 {
            if left(oid, date, author) >= MIN_SUBJECT {
                break;
            }
            match column {
                0 => date = 0,
                1 => author = 0,
                _ => oid = 0,
            }
        }
        let subject = left(oid, date, author);
        Self {
            oid,
            date,
            author,
            subject,
        }
    }

    /// The columns of one commit, widest-first, skipping the ones that are not
    /// drawn. The colour of each is a role it already had elsewhere: the
    /// abbreviated name locates a commit the way `@@ … @@` locates a hunk, and
    /// an author is a name the way a directory is.
    fn cells(&self, commit: &crate::git::Commit, theme: &Theme) -> Vec<(String, Style)> {
        [
            (self.oid, &commit.short, theme.diff_hunk),
            (self.date, &commit.date, theme.dim),
            (self.author, &commit.author, theme.directory),
            (self.subject, &commit.subject, theme.foreground),
        ]
        .into_iter()
        .filter(|(width, _, _)| *width > 0)
        .map(|(width, value, colour)| (fit(value, width), Style::new().fg(colour)))
        .collect()
    }

    /// The whole row as one string — what the selected row is drawn as.
    fn row(&self, commit: &crate::git::Commit, theme: &Theme) -> String {
        let gap = " ".repeat(GAP);
        self.cells(commit, theme)
            .into_iter()
            .map(|(text, _)| format!("{gap}{text}"))
            .collect()
    }

    /// The same row in colour, for every row that is not the selected one.
    fn spans(&self, commit: &crate::git::Commit, theme: &Theme) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for (text, style) in self.cells(commit, theme) {
            spans.push(Span::raw(" ".repeat(GAP)));
            spans.push(Span::styled(text, style));
        }
        spans
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
            body: String::new(),
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
        let theme = Theme::default();
        let log = log();
        for width in [40, 60, 100, 200] {
            let widths = Columns::of(&log, width);
            for commit in log.rows() {
                assert_eq!(
                    widths.row(commit, &theme).width(),
                    width,
                    "at {width}: {}",
                    commit.short
                );
            }
        }
    }

    /// A wide pane keeps every column and caps the author's; the subject takes
    /// what is left, which is most of it.
    #[test]
    fn a_wide_pane_keeps_every_column_and_caps_the_author() {
        let theme = Theme::default();
        let log = log();
        let widths = Columns::of(&log, 120);
        assert_eq!(widths.author, MAX_AUTHOR);
        assert!(widths.subject >= 60, "{}", widths.subject);
        let row = widths.row(log.rows().nth(1).unwrap(), &theme);
        assert!(row.contains(ELLIPSIS), "the long name is cut: {row}");
        assert!(row.contains("fix: a bug"), "{row}");
    }

    /// The columns give way one at a time as the pane narrows, and the subject
    /// keeps its floor for as long as there is one to keep (ADR-080).
    ///
    /// The author shrinks first — a cut name is still a name — then the date
    /// goes, then the author, and the abbreviated name is the last to leave:
    /// it is the one thing on the row that names the commit to git.
    #[test]
    fn the_subject_keeps_its_width_and_the_columns_give_way_in_turn() {
        let log = log();
        let wide = Columns::of(&log, 120);
        let middling = Columns::of(&log, 70);
        let narrow = Columns::of(&log, 50);

        assert!(middling.author < wide.author, "the author shrinks first");
        assert!(middling.subject >= MIN_SUBJECT);
        assert_eq!(narrow.date, 0, "the date is the first column to go");
        assert!(narrow.oid > 0, "and the hash is the last");
        assert!(narrow.subject >= MIN_SUBJECT, "{}", narrow.subject);

        // Never wider than it is asked for, and never widening as the pane
        // narrows.
        for width in MIN_SUBJECT..200 {
            let columns = Columns::of(&log, width);
            assert!(columns.author == 0 || columns.author >= MIN_AUTHOR);
        }
    }

    /// The columns can be switched off outright, which gives the subject the
    /// whole pane (ADR-080).
    #[test]
    fn hiding_the_columns_gives_the_subject_the_pane() {
        let theme = Theme::default();
        let mut log = log();
        log.show_columns = false;
        let widths = Columns::of(&log, 60);
        assert_eq!((widths.oid, widths.date, widths.author), (0, 0, 0));
        assert_eq!(widths.subject, 60 - GAP);
        let row = widths.row(log.rows().next().unwrap(), &theme);
        assert!(row.contains("feat: the newest thing"), "{row}");
        assert_eq!(row.width(), 60);
    }

    /// A pane too narrow for the fixed columns leaves the subject nothing, and
    /// that is a row with no subject rather than a panic.
    #[test]
    fn a_pane_with_no_room_for_a_subject_does_not_panic() {
        let theme = Theme::default();
        let log = log();
        for width in 0..30 {
            let widths = Columns::of(&log, width);
            for commit in log.rows() {
                let _ = widths.row(commit, &theme);
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
