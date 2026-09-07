//! UI: tab bar.

use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::ui::layout::LayoutRects;
use crate::ui::theme::Theme;

/// Unsaved changes (SPEC §11). Its cell is reserved on every tab, modified or
/// not, so a tab does not change width the moment the file is edited.
const DIRTY: &str = "●";
const CLEAN: &str = " ";
/// The file underneath moved and the buffer has not followed it (ADR-043). It
/// takes the same cell as the dirty marker rather than one of its own: a stale
/// tab is nearly always a modified one, and the stronger of the two facts is
/// the one worth a cell in a tab bar that is already short of them.
const STALE: &str = "!";
/// The close button. `×` rather than a heavier glyph because it has to read as
/// a control at one cell in a 256-colour palette.
const CLOSE: &str = "×";

pub fn render(frame: &mut Frame, app: &App, rects: &LayoutRects, theme: &Theme) {
    frame.render_widget(Block::new().style(theme.tab_inactive), rects.tab_bar);

    for (index, tab) in app.tabs.iter().enumerate() {
        let Some(rect) = rects.tabs.get(index) else {
            continue;
        };
        if rect.width == 0 {
            continue;
        }
        let active = app.active_tab == Some(index);
        let style = if active {
            theme.tab_active
        } else {
            theme.tab_inactive
        };
        let (marker, colour) = match (tab.stale.is_some(), tab.document.is_dirty()) {
            (true, _) => (STALE, Some(theme.warning)),
            (false, true) => (DIRTY, Some(theme.tab_dirty)),
            (false, false) => (CLEAN, None),
        };
        let spans = vec![
            Span::styled(format!(" {} ", tab.document.title()), style),
            Span::styled(marker, colour.map_or(style, |c| style.fg(c))),
            Span::styled(" ", style),
            Span::styled(CLOSE, style.fg(theme.tab_close)),
            Span::styled(" ", style),
        ];
        // A clipped tab is truncated by the Paragraph rather than by us: the
        // rect is already the visible part of it.
        frame.render_widget(Paragraph::new(Line::from(spans)).style(style), *rect);
    }

    // "There are more tabs this way." Not clickable: the tab bar scrolls to
    // whichever tab is active, so there is nothing for a click here to do that
    // Ctrl+Tab does not already do.
    for (rect, arrow) in [
        (rects.tab_overflow_left, "‹"),
        (rects.tab_overflow_right, "›"),
    ] {
        if let Some(rect) = rect {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    arrow,
                    theme.tab_inactive.fg(theme.foreground),
                ))),
                rect,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::layout;
    use ratatui::layout::Rect;

    /// The tab bar row of a rendered frame.
    fn tab_bar_row(app: &App, width: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 24)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), app);
                crate::ui::render(frame, app, &rects, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let bar = layout::compute(Rect::new(0, 0, width, 24), app).tab_bar;
        (bar.x..bar.right())
            .map(|x| buffer[(x, bar.y)].symbol())
            .collect()
    }

    #[test]
    fn a_tab_shows_its_name_a_dirty_marker_and_a_close_button() {
        let app = App::fixture();
        let row = tab_bar_row(&app, 100);
        assert!(row.contains("main.rs"));
        assert!(row.contains(CLOSE), "every tab has a close button");
        assert!(
            row.contains(DIRTY),
            "the second fixture tab has unsaved changes: {row}"
        );
    }

    #[test]
    fn a_clean_tab_reserves_the_marker_cell_instead_of_dropping_it() {
        let mut app = App::fixture();
        let clean = tab_bar_row(&app, 100);
        app.tabs[0].document.insert_char('x');
        let dirty = tab_bar_row(&app, 100);
        assert_eq!(
            clean.chars().count(),
            dirty.chars().count(),
            "the bar is the same width either way"
        );
        // The tab after the one that changed has not moved.
        let at = |row: &str, needle: &str| row.find(needle).map(|i| row[..i].chars().count());
        assert_eq!(at(&clean, "editor.rs"), at(&dirty, "editor.rs"));
    }

    /// A file that moved under the buffer takes the marker cell, because it is
    /// the stronger of the two facts about the tab (ADR-043).
    #[test]
    fn a_stale_tab_says_so_where_the_dirty_marker_goes() {
        let mut app = App::fixture();
        app.tabs[1].stale = Some(crate::app::tabs::Stale::Changed);
        let row = tab_bar_row(&app, 100);
        assert!(row.contains(STALE), "{row}");
        assert!(
            !row.contains(DIRTY),
            "the only modified tab is the stale one: {row}"
        );
        assert_eq!(
            row.chars().count(),
            tab_bar_row(&App::fixture(), 100).chars().count(),
            "and the bar is the same width either way"
        );
    }

    #[test]
    fn a_bar_too_narrow_for_every_tab_shows_where_the_rest_are() {
        let mut app = App::fixture_with_tabs(12);
        let row = tab_bar_row(&app, 60);
        assert!(!row.contains('‹'), "the first tab is on screen");
        assert!(row.contains('›'), "but the last ones are not: {row}");

        app.active_tab = Some(11);
        let row = tab_bar_row(&app, 60);
        assert!(row.contains("file11.rs"), "the active tab is shown: {row}");
        assert!(row.contains('‹'), "and the earlier tabs are off the left");
        assert!(!row.contains('›'));
    }
}
