//! UI: the help screen (SPEC §6).
//!
//! A pager over the keymap, drawn over the whole body. Like the diff viewer it
//! decides colours and columns and nothing else: what the rows *say* is
//! `docs::sections()`, laid out by `app::help` against the pane's width
//! (ADR-038).

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::focus::FocusTarget;
use crate::app::help::HelpLine;
use crate::app::App;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(help) = app.help.as_ref() else {
        return;
    };
    let focused = app.focus == FocusTarget::Help;
    // The panes underneath are drawn first, so the ground has to be taken back
    // before anything is written on it.
    frame.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.border_for(focused))
        .style(Style::new().bg(theme.background))
        .title(Span::styled(help.title(), theme.panel_title))
        .title_bottom(
            Line::from(Span::styled(
                help.position(area.width.saturating_sub(2) as usize),
                Style::new().fg(theme.dim),
            ))
            .right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let lines: Vec<Line> = help
        .lines(inner.width as usize)
        .into_iter()
        .skip(help.scroll)
        .take(inner.height as usize)
        .map(|line| styled(&line, theme))
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

/// One line, coloured by what it is. Rows are two spans so the key column
/// stands out from the sentence next to it — the same split the generated
/// table has, drawn instead of piped.
fn styled(line: &HelpLine, theme: &Theme) -> Line<'static> {
    match line {
        HelpLine::Blank => Line::default(),
        HelpLine::Heading(text) => Line::from(Span::styled(format!(" {text}"), theme.panel_title)),
        HelpLine::Note(text) => {
            Line::from(Span::styled(format!(" {text}"), Style::new().fg(theme.dim)))
        }
        HelpLine::Row { keys, action } => Line::from(vec![
            Span::styled(format!(" {keys}"), Style::new().fg(theme.menu_shortcut)),
            Span::styled(action.clone(), Style::new().fg(theme.foreground)),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::help::HelpState;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = crate::ui::layout::compute(frame.area(), app);
                crate::ui::render(frame, app, &rects, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn the_screen_covers_the_body_and_shows_the_keymap() {
        let mut app = App::fixture();
        app.help = Some(HelpState::new(FocusTarget::Editor));
        app.focus = FocusTarget::Help;
        let rows = draw(&app, 80, 24);
        let screen = rows.join("\n");

        assert!(screen.contains("Keyboard shortcuts"), "{screen}");
        assert!(screen.contains("Ctrl+Q"), "{screen}");
        assert!(screen.contains("Anywhere"), "{screen}");
        // The menu bar stays above it; the panes under it do not show through.
        assert!(rows[0].contains("File"), "{:?}", rows[0]);
        assert!(
            !screen.contains("fn main() {"),
            "the editor is drawn under the screen: {screen}"
        );
        assert!(rows[23].contains("Ln 1, Col 1"), "{:?}", rows[23]);
    }

    #[test]
    fn scrolling_moves_the_window() {
        let mut app = App::fixture();
        let mut help = HelpState::new(FocusTarget::Editor);
        help.scroll = 12;
        app.help = Some(help);
        app.focus = FocusTarget::Help;
        // Row 0 is the menu bar and row 1 the screen's top border; the first
        // line of text is row 2.
        let scrolled = draw(&app, 80, 24)[2].clone();
        app.help.as_mut().unwrap().scroll = 0;
        assert_ne!(scrolled, draw(&app, 80, 24)[2]);
    }
}
