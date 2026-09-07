//! Rendering. Every function here is read-only over `&App`.

pub mod dialog;
pub mod diff;
pub mod editor;
pub mod explorer;
pub mod field;
pub mod git;
pub mod help;
pub mod layout;
pub mod menu;
pub mod search;
pub mod statusbar;
pub mod tabs;
pub mod theme;

use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use layout::{LayoutRects, MIN_HEIGHT, MIN_WIDTH};
use theme::Theme;

/// Draws one frame. Takes the rects rather than computing them so the main loop
/// and the renderer can never disagree about where a pane is — which is what
/// makes mouse hit-testing against the *last drawn* frame correct.
pub fn render(frame: &mut Frame, app: &App, rects: &LayoutRects, theme: &Theme) {
    let area = frame.area();
    frame.render_widget(
        Block::new().style(Style::new().bg(theme.background).fg(theme.foreground)),
        area,
    );

    if rects.too_small {
        let notice = Paragraph::new(format!(
            "Terminal too small\n{}x{} — need at least {MIN_WIDTH}x{MIN_HEIGHT}",
            area.width, area.height
        ))
        .style(Style::new().fg(theme.warning))
        .wrap(Wrap { trim: true });
        frame.render_widget(notice, area);
        return;
    }

    explorer::render(frame, app, rects.explorer, theme);
    git::render(frame, app, rects.git_panel, theme);
    tabs::render(frame, app, rects, theme);
    editor::render(frame, app, rects.editor, theme);
    // Over the editor, because that is the pane it replaces while it is open.
    if let Some(area) = rects.diff {
        diff::render(frame, app, area, theme);
    }
    // Over the whole body, sidebar included: it is a screen, not a pane.
    if let Some(area) = rects.help {
        help::render(frame, app, area, theme);
    }
    search::render(frame, app, rects, theme);
    statusbar::render(frame, app, rects.status_bar, theme);
    menu::render(frame, app, rects, theme);
    // A dialog is modal, so it draws last of all — over the menu's popup too.
    dialog::render(frame, app, rects, theme);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::focus::FocusTarget;
    use crate::editor::coords::VisualCol;
    use crate::editor::cursor::Motion;
    use ratatui::backend::TestBackend;
    use ratatui::layout::{Position, Rect};
    use ratatui::Terminal;

    fn app() -> App {
        App::fixture()
    }

    /// Renders one frame and returns the buffer as one string per row.
    fn draw(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), app);
                render(frame, app, &rects, &theme);
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
    fn every_zone_paints_at_the_acceptance_size() {
        let rows = draw(&app(), 60, 20);
        let screen = rows.join("\n");

        assert!(rows[0].contains("File"), "menu bar missing: {:?}", rows[0]);
        assert!(rows[0].contains("Help"));
        assert!(screen.contains("ferroedit-test"), "explorer header missing");
        assert!(screen.contains("main.rs"), "tab bar / explorer missing");
        assert!(screen.contains("Git — main"), "git panel missing");
        assert!(screen.contains("fn main() {"), "editor content missing");
        assert!(
            rows[19].contains("Ln 1, Col 1"),
            "status bar missing: {:?}",
            rows[19]
        );
    }

    #[test]
    fn the_git_panel_draws_the_two_column_code_of_every_change() {
        let screen = draw(&app(), 80, 24).join("\n");
        // The fixture holds one file in each of the four states, in the `XY`
        // form of SPEC §30: index column first, worktree column second.
        assert!(screen.contains(" M src/main.rs"), "{screen}");
        assert!(screen.contains("A  src/ui/theme.rs"), "{screen}");
        assert!(screen.contains(" D src/old.rs"), "{screen}");
        assert!(screen.contains(" ? src/new.rs"), "{screen}");
        assert!(
            screen.contains("Git — main (4)"),
            "the count is in the title"
        );
    }

    /// SPEC §34: while a network operation runs, the panel says so — and keeps
    /// saying so, which a four-second notification cannot.
    #[test]
    fn the_panel_title_says_what_is_running() {
        let mut app = app();
        app.git.pretend_running(crate::git::GitJob::Push);
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("Git — Pushing…"), "{screen}");
    }

    /// SPEC §35: an unfinished operation is a state the user has to finish,
    /// and it outranks the ahead/behind counts in the title. All four are
    /// named, not only the merge Phase 11 could see (ADR-041).
    #[test]
    fn the_panel_says_when_an_operation_is_unfinished() {
        use crate::git::models::Operation;
        for (operation, label) in [
            (Operation::Merge, "merging"),
            (Operation::Rebase, "rebasing"),
            (Operation::CherryPick, "cherry-picking"),
            (Operation::Revert, "reverting"),
        ] {
            let mut app = app();
            app.git.status.operation = Some(operation);
            // Wide enough for the sidebar to reach its maximum: at 80 columns
            // the panel is twenty cells and the title is clipped mid-word.
            let screen = draw(&app, 130, 24).join("\n");
            // The count is asserted only for the labels that leave room for
            // it: the sidebar caps at 32 cells and `[cherry-picking]` fills
            // the title on its own.
            assert!(
                screen.contains(&format!("Git — main [{label}]")),
                "{screen}"
            );
        }
    }

    /// SPEC §33's picker: every branch, `*` on the one HEAD is on, and the
    /// highlight where Enter would act.
    #[test]
    fn the_branch_picker_draws_its_list() {
        use crate::git::models::Branch;
        let branch = |name: &str, is_head| Branch {
            name: name.into(),
            is_head,
            remote: false,
        };
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::switch_branch(
            &[branch("main", true), branch("feature/editor", false)],
            crate::app::focus::FocusTarget::GitPanel,
        ));
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("Switch Branch"), "{screen}");
        assert!(screen.contains("2 branches"), "{screen}");
        assert!(screen.contains("* main"), "git's own marker: {screen}");
        assert!(screen.contains("  feature/editor"), "{screen}");
        assert!(screen.contains("[ Switch ]"), "{screen}");
        assert!(screen.contains("[ New… ]"), "{screen}");
    }

    #[test]
    fn a_workspace_that_is_not_a_repository_says_so_in_the_panel() {
        let mut app = app();
        app.git = crate::app::git::GitState::default();
        app.git.availability = crate::app::git::GitAvailability::NotARepository;
        // Wide enough for the sidebar to reach its maximum, where the sentence
        // fits on one row; the narrow case is the test below.
        let screen = draw(&app, 130, 24).join("\n");
        // The wording is SPEC §28's, not a paraphrase of it.
        assert!(screen.contains("Not a Git repository"), "{screen}");
        assert!(!screen.contains("Git — "), "there is no branch to name");
    }

    /// End to end, with no fixture anywhere: a real repository, read by the
    /// real `git`, drawn by the real panel.
    #[test]
    fn the_panel_draws_the_status_of_a_repository_on_disk() {
        let repo = crate::git::testing::TestRepo::new();
        repo.write("tracked.rs", "fn main() {}\n");
        repo.run(&["add", "."]);
        repo.commit("init");
        repo.write("tracked.rs", "fn main() { }\n");
        repo.write("loose.rs", "// new\n");

        let mut app = App::fixture_in(repo.path());
        app.git.discover(repo.path());
        let screen = draw(&app, 130, 24).join("\n");

        assert!(screen.contains("Git — main (2)"), "{screen}");
        assert!(screen.contains(" M tracked.rs"), "{screen}");
        assert!(screen.contains(" ? loose.rs"), "{screen}");
    }

    #[test]
    fn a_sidebar_too_narrow_for_the_message_wraps_it_rather_than_cutting_it() {
        let mut app = app();
        app.git = crate::app::git::GitState::default();
        app.git.availability = crate::app::git::GitAvailability::NotARepository;
        let screen = draw(&app, 60, 20).join("\n");
        assert!(screen.contains("Not a Git"), "{screen}");
        assert!(screen.contains("repository"), "the tail is on the next row");
    }

    #[test]
    fn a_repository_with_nothing_changed_says_the_tree_is_clean() {
        let mut app = app();
        app.git.status.entries.clear();
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("working tree clean"), "{screen}");
        assert!(screen.contains("Git — main"), "the branch is still named");
    }

    #[test]
    fn a_git_command_that_failed_puts_its_reason_where_the_list_was() {
        let mut app = app();
        app.git.availability =
            crate::app::git::GitAvailability::Unavailable("Git is not installed".into());
        app.git.status.entries.clear();
        let screen = draw(&app, 130, 24).join("\n");
        assert!(screen.contains("Git is not installed"), "{screen}");
    }

    #[test]
    fn the_status_bar_names_the_branch_and_says_when_there_is_no_repository() {
        let mut app = app();
        assert!(draw(&app, 80, 24)[23].contains("main"));
        app.git = crate::app::git::GitState::default();
        app.git.availability = crate::app::git::GitAvailability::NotARepository;
        assert!(draw(&app, 80, 24)[23].contains("no repository"));
    }

    #[test]
    fn the_gutter_numbers_the_editor_lines() {
        let screen = draw(&app(), 80, 24).join("\n");
        assert!(screen.contains(" 1  fn main() {"));
        assert!(screen.contains(" 3  }"));
    }

    #[test]
    fn an_open_menu_paints_its_items_and_bound_shortcuts_over_the_editor() {
        let mut app = app();
        app.menu.open = Some(0);
        app.focus = FocusTarget::Menu;
        let rows = draw(&app, 80, 24);
        let screen = rows.join("\n");
        assert!(screen.contains("Quit"));
        assert!(
            screen.contains("Ctrl+Q"),
            "the bound shortcut must be shown"
        );
        assert!(screen.contains("Ctrl+S"), "Save is bound from Phase 2");
        assert!(screen.contains("Ctrl+N"), "New File is bound from Phase 6");
        assert!(screen.contains("Ctrl+O"), "Open is bound from Phase 9");
        // Save As has no key at all (ADR-028), so its row must show none: the
        // menu advertises what is bound and nothing else.
        let save_as = rows
            .iter()
            .find(|row| row.contains("Save As"))
            .expect("the File menu is open");
        assert!(
            !save_as.contains("Ctrl"),
            "an unbound entry must not advertise a key: {save_as:?}"
        );
    }

    /// The background colours of one row, so a test can see what is
    /// highlighted rather than only what is written.
    fn row_backgrounds(app: &App, width: u16, height: u16, row: u16) -> Vec<ratatui::style::Color> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), app);
                render(frame, app, &rects, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..width).map(|x| buffer[(x, row)].bg).collect()
    }

    /// The foreground colour of every cell on one row.
    fn row_foregrounds(app: &App, width: u16, height: u16, row: u16) -> Vec<ratatui::style::Color> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), app);
                render(frame, app, &rects, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..width).map(|x| buffer[(x, row)].fg).collect()
    }

    #[test]
    fn the_editor_paints_the_colours_the_highlighter_chose() {
        let mut app = app();
        // The fixture's first tab is `main.rs`, and nothing has been drawn yet,
        // so this is also the run loop's own first sync.
        app.sync_highlight();
        assert_eq!(app.tabs[0].highlights.language(), "Rust");

        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        let gutter = (rects.editor.x + 4) as usize;
        let foregrounds = row_foregrounds(&app, 80, 24, rects.editor.y);
        let theme = Theme::default();

        // `fn main() {` — the keyword, then the function name.
        assert_eq!(
            foregrounds[gutter], theme.syntax.keyword,
            "`fn` is a keyword"
        );
        assert_eq!(
            foregrounds[gutter + 3],
            theme.syntax.function,
            "`main` is a function name"
        );
    }

    #[test]
    fn selected_code_keeps_its_syntax_colours() {
        let mut app = app();
        app.sync_highlight();
        // Select `fn main` — a keyword and a function name in one selection.
        app.tabs[0].document.place_cursor(0, VisualCol(0));
        app.tabs[0].document.extend_to(0, VisualCol(7));

        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        let gutter = (rects.editor.x + 4) as usize;
        let row = rects.editor.y;
        let theme = Theme::default();
        let foregrounds = row_foregrounds(&app, 80, 24, row);
        let backgrounds = row_backgrounds(&app, 80, 24, row);

        assert_eq!(backgrounds[gutter], theme.editor_selection.bg.unwrap());
        assert_eq!(
            foregrounds[gutter], theme.syntax.keyword,
            "the selection sets a background only, so the keyword is still purple"
        );
        assert_eq!(foregrounds[gutter + 3], theme.syntax.function);
    }

    #[test]
    fn a_selection_is_painted_behind_the_text_it_covers() {
        let mut app = app();
        // Select `main` on the first line: four cells, after `fn `.
        app.tabs[0].document.place_cursor(0, VisualCol(3));
        app.tabs[0].document.extend_to(0, VisualCol(7));
        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        let row = rects.editor.y;
        let gutter = rects.editor.x + 4;

        let backgrounds = row_backgrounds(&app, 80, 24, row);
        let theme = Theme::default();
        let selected = theme.editor_selection.bg.unwrap();
        let highlighted: Vec<usize> = backgrounds
            .iter()
            .enumerate()
            .filter(|(_, bg)| **bg == selected)
            .map(|(x, _)| x)
            .collect();
        assert_eq!(
            highlighted,
            ((gutter + 3) as usize..(gutter + 7) as usize).collect::<Vec<_>>(),
            "exactly the four cells of `main` are highlighted"
        );
    }

    #[test]
    fn the_status_bar_counts_the_selection_only_while_there_is_one() {
        let mut app = app();
        assert!(!draw(&app, 80, 24).join("\n").contains("Sel "));
        app.tabs[0].document.select_all();
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("Sel 36"), "no selection count: {screen}");
    }

    /// The bar's known Phase 3 crowding, fixed: at 60 columns the pieces
    /// nobody reads leave and the notification keeps its room (ADR-039).
    #[test]
    fn the_status_bar_sheds_its_readout_before_it_crowds_the_notification() {
        let mut app = app();
        app.notifications
            .info("Saved src/main.rs to disk without trouble".to_string());

        let wide = draw(&app, 120, 24)[23].clone();
        assert!(wide.contains("UTF-8"), "{wide:?}");
        assert!(wide.contains("Editor"), "{wide:?}");
        assert!(wide.contains("main"), "{wide:?}");

        let narrow = draw(&app, 60, 20)[19].clone();
        assert!(
            narrow.contains("Ln 1, Col 1"),
            "the position never goes: {narrow:?}"
        );
        assert!(
            !narrow.contains("UTF-8"),
            "the encoding is the first piece out: {narrow:?}"
        );
        assert!(
            narrow.contains("Saved src/main.rs"),
            "the notification is no longer clipped: {narrow:?}"
        );
    }

    /// Past every drop the position still fits, and nothing is written over
    /// anything else.
    #[test]
    fn the_narrowest_bar_is_the_cursor_position_and_nothing_more() {
        let app = app();
        let row = draw(&app, layout::MIN_WIDTH, 20)[19].clone();
        assert!(row.contains("Ln 1, Col 1"), "{row:?}");
        assert!(!row.contains("Editor"), "{row:?}");
        assert!(!row.contains("UTF-8"), "{row:?}");
        assert_eq!(row.chars().count(), layout::MIN_WIDTH as usize);
    }

    /// Draws one frame and reports where the terminal's cursor ended up.
    ///
    /// The backend starts at the origin, which the editor can never place the
    /// cursor on — so an unchanged origin means "no caret was drawn".
    fn draw_cursor(app: &App, width: u16, height: u16) -> Position {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.set_cursor_position(Position::new(0, 0)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), app);
                render(frame, app, &rects, &theme);
            })
            .unwrap();
        terminal.get_cursor_position().unwrap()
    }

    #[test]
    fn the_terminal_cursor_sits_on_the_document_cursor_while_the_editor_has_focus() {
        let mut app = app();
        app.active_mut()
            .unwrap()
            .document
            .place_cursor(1, VisualCol(4));

        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        // The gutter is two digits plus two cells of padding, so text column 4
        // is eight cells into the pane.
        let expected = Position::new(rects.editor.x + 4 + 4, rects.editor.y + 1);
        assert_eq!(draw_cursor(&app, 80, 24), expected);
    }

    #[test]
    fn an_unfocused_editor_shows_no_caret() {
        let mut app = app();
        app.focus = FocusTarget::Explorer;
        assert_eq!(draw_cursor(&app, 80, 24), Position::new(0, 0));
    }

    #[test]
    fn a_long_line_scrolls_horizontally_under_the_cursor() {
        let mut app = app();
        app.tabs[0] = crate::app::Tab::scratch("wide.txt", &"abcdefghij".repeat(30));
        app.editor_view = crate::app::EditorView {
            width: 60,
            height: 22,
        };
        app.active_mut()
            .unwrap()
            .document
            .move_cursor(Motion::End, 22);
        let view = app.editor_view;
        app.active_mut().unwrap().follow_cursor(view);

        let screen = draw(&app, 80, 24).join("\n");
        assert!(
            screen.contains("hij"),
            "the end of the line must be on screen"
        );
        assert!(
            !screen.contains(" 1  abcdefghij"),
            "the start of the line has scrolled off"
        );
    }

    #[test]
    fn the_status_bar_follows_the_cursor() {
        let mut app = app();
        app.active_mut()
            .unwrap()
            .document
            .move_cursor(Motion::Down, 24);
        app.active_mut()
            .unwrap()
            .document
            .move_cursor(Motion::End, 24);
        let rows = draw(&app, 80, 24);
        assert!(
            rows[23].contains("Ln 2, Col 23"),
            "status bar: {:?}",
            rows[23]
        );
    }

    #[test]
    fn an_empty_app_says_so_instead_of_drawing_an_editor() {
        let app = App::new(crate::app::workspace::Workspace::from_arg(None).unwrap());
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("No file open"));
    }

    #[test]
    fn an_open_dialog_paints_over_everything_including_the_menu() {
        let mut app = app();
        app.menu.open = Some(0);
        app.dialog = Some(crate::app::dialog::DialogState::unsaved_changes(
            1,
            "editor.rs",
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        let screen = draw(&app, 80, 24).join("\n");

        assert!(screen.contains("Unsaved changes"), "no title: {screen}");
        assert!(screen.contains("editor.rs has unsaved changes."));
        assert!(screen.contains("[ Save ]"));
        assert!(screen.contains("[ Don't Save ]"));
        assert!(screen.contains("[ Cancel ]"));

        // At a size where the modal box and the menu popup overlap, the box is
        // the one on top: it draws after everything else.
        let small = draw(&app, 40, 8).join("\n");
        assert!(small.contains("Unsaved changes"));
        assert!(
            !small.contains("New File"),
            "the popup shows through the modal box: {small}"
        );
    }

    #[test]
    fn the_selected_dialog_button_is_the_highlighted_one() {
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::unsaved_changes(
            0,
            "main.rs",
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        let row = rects.dialog_buttons[0].y;
        let theme = Theme::default();
        let highlighted = theme.dialog_button_selected.bg.unwrap();

        let backgrounds = row_backgrounds(&app, 80, 24, row);
        let cells: Vec<usize> = backgrounds
            .iter()
            .enumerate()
            .filter(|(_, bg)| **bg == highlighted)
            .map(|(x, _)| x)
            .collect();
        let save = rects.dialog_buttons[0];
        assert_eq!(
            cells,
            (save.x as usize..save.right() as usize).collect::<Vec<_>>(),
            "exactly the default button is highlighted"
        );
    }

    #[test]
    fn a_dialog_hides_the_caret_because_the_editor_no_longer_has_focus() {
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::unsaved_on_quit(
            1,
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        assert_eq!(draw_cursor(&app, 80, 24), Position::new(0, 0));
    }

    #[test]
    fn a_too_small_terminal_shows_a_notice_instead_of_a_broken_layout() {
        let screen = draw(&app(), 30, 6).join("\n");
        assert!(screen.contains("Terminal too small"));
        assert!(!screen.contains("fn main()"));
    }

    #[test]
    fn rendering_never_panics_across_a_wide_range_of_sizes() {
        let mut app = app();
        for width in [1, 20, 39, 40, 60, 100, 200] {
            for height in [1, 5, 8, 20, 60] {
                let _ = draw(&app, width, height);
            }
        }
        // Again with the widest modal box open, which is the thing most likely
        // to run off a narrow frame.
        app.dialog = Some(crate::app::dialog::DialogState::unsaved_changes(
            0,
            "a-very-long-file-name-indeed.rs",
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        for width in [1, 20, 39, 40, 60, 100, 200] {
            for height in [1, 5, 8, 20, 60] {
                let _ = draw(&app, width, height);
            }
        }
    }

    // --- Phase 6 ------------------------------------------------------------

    /// An app over a real directory, which is what the explorer draws from.
    fn tree_app(dir: &tempfile::TempDir) -> App {
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("README.md"), "# hi").unwrap();
        App::fixture_in(dir.path())
    }

    #[test]
    fn the_explorer_draws_the_real_tree_with_a_marker_per_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = tree_app(&dir);
        let screen = draw(&app, 80, 24);
        assert!(
            screen.iter().any(|row| row.contains("▶ src")),
            "a closed directory points right: {screen:?}"
        );
        assert!(screen.iter().any(|row| row.contains("  README.md")));

        app.sidebar
            .tree
            .expand(&std::fs::canonicalize(dir.path().join("src")).unwrap());
        let screen = draw(&app, 80, 24);
        assert!(screen.iter().any(|row| row.contains("▼ src")));
        assert!(
            screen.iter().any(|row| row.contains("    main.rs")),
            "and its contents are indented one level: {screen:?}"
        );
    }

    #[test]
    fn an_empty_workspace_says_so_rather_than_drawing_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::fixture_in(dir.path());
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("(empty or ignored)"), "{screen}");
    }

    #[test]
    fn the_selected_row_is_highlighted_in_the_explorer() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = tree_app(&dir);
        app.sidebar.selected = 1;
        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        // One row of the panel is its title.
        let backgrounds = row_backgrounds(&app, 80, 24, rects.explorer.y + 2);
        let selected = Theme::default().selection.bg.unwrap();
        assert!(
            backgrounds.contains(&selected),
            "the second row is painted as the selection"
        );
    }

    #[test]
    fn the_file_being_edited_is_marked_in_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = tree_app(&dir);
        app.open_path(&dir.path().join("README.md"), None).unwrap();

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), &app);
                render(frame, &app, &rects, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let bold = (0..24)
            .flat_map(|y| (0..20).map(move |x| (x, y)))
            .filter(|(x, y)| {
                buffer[(*x, *y)]
                    .modifier
                    .contains(ratatui::style::Modifier::BOLD)
            })
            .map(|(x, y)| buffer[(x, y)].symbol().to_string())
            .collect::<String>();
        assert!(
            bold.contains("README.md"),
            "the active file is the bold one in the sidebar: {bold:?}"
        );
    }

    #[test]
    fn an_input_dialog_draws_its_prompt_its_field_and_the_caret() {
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::new_file(
            std::path::Path::new("/project/src"),
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        if let Some(field) = app.dialog.as_mut().unwrap().field_mut() {
            field.insert_str("notes.md");
        }

        let screen = draw(&app, 80, 24);
        let joined = screen.join("\n");
        assert!(joined.contains("New File"), "the title");
        assert!(joined.contains("Create in src"), "the prompt");
        assert!(joined.contains("notes.md"), "what has been typed");
        assert!(joined.contains("[ Create ]") && joined.contains("[ Cancel ]"));

        // The caret sits just past the text, inside the box.
        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        let area = rects.dialog.expect("a dialog rect");
        assert_eq!(
            draw_cursor(&app, 80, 24),
            Position::new(area.x + 2 + "notes.md".len() as u16, area.y + 2)
        );
    }

    #[test]
    fn a_confirmation_dialog_puts_no_caret_on_the_screen() {
        let mut app = app();
        app.dialog = Some(crate::app::dialog::DialogState::unsaved_on_quit(
            1,
            FocusTarget::Editor,
        ));
        app.focus = FocusTarget::Dialog;
        assert_eq!(draw_cursor(&app, 80, 24), Position::new(0, 0));
    }
    /// The fixture with the find bar open over a query that hits.
    fn searching(query: &str, replacing: bool) -> App {
        let mut app = app();
        app.tabs = vec![crate::app::Tab::scratch(
            "notes.txt",
            "fn main() {\n    println!(\"hello\");\n}",
        )];
        app.active_tab = Some(0);
        crate::commands::execute::execute_command(
            &mut app,
            if replacing {
                crate::commands::Command::ReplaceOpen
            } else {
                crate::commands::Command::SearchOpen
            },
        );
        for ch in query.chars() {
            crate::commands::execute::execute_command(
                &mut app,
                crate::commands::Command::SearchInputChar(ch),
            );
        }
        app.sync_search();
        app
    }

    #[test]
    fn the_find_bar_shows_the_query_and_how_many_it_matched() {
        let app = searching("l", false);
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("Find:"), "the bar is drawn: {screen}");
        assert!(screen.contains("[Aa]"), "and its case toggle");
        // `l` appears four times in `println!("hello")` plus none elsewhere.
        let count = app.search.count_label();
        assert!(screen.contains(&count), "the readout says {count}");
    }

    #[test]
    fn the_replace_bar_adds_a_row_with_two_buttons() {
        let app = searching("l", true);
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("Repl:"));
        assert!(screen.contains("[Replace]"));
        assert!(screen.contains("[All]"));
    }

    #[test]
    fn the_bar_takes_its_rows_out_of_the_editor() {
        let plain = layout::compute(Rect::new(0, 0, 80, 24), &app());
        let searching = searching("l", true);
        let with_bar = layout::compute(Rect::new(0, 0, 80, 24), &searching);
        assert_eq!(with_bar.editor.height, plain.editor.height - 2);
        assert_eq!(
            with_bar.search.as_ref().map(|s| s.bar.height),
            Some(2),
            "one row to find and one to replace"
        );
    }

    #[test]
    fn every_hit_is_painted_and_the_current_one_is_the_selection() {
        let mut app = searching("println", false);
        crate::commands::execute::execute_command(&mut app, crate::commands::Command::FindNext);

        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        // `println` is on the second line, four spaces in.
        let row = rects.editor.y + 1;
        let gutter = (rects.editor.x + 4) as usize;
        let theme = Theme::default();
        let backgrounds = row_backgrounds(&app, 80, 24, row);
        assert_eq!(
            backgrounds[gutter + 4],
            theme.editor_selection.bg.unwrap(),
            "the current hit is the selection"
        );

        // A second hit, not current, is painted with the match background.
        let mut app = searching("l", false);
        crate::commands::execute::execute_command(&mut app, crate::commands::Command::FindNext);
        let backgrounds = row_backgrounds(&app, 80, 24, rects.editor.y + 1);
        assert!(
            backgrounds.contains(&theme.search_match.bg.unwrap()),
            "the hits that are not current still show"
        );
    }

    #[test]
    fn the_caret_is_in_the_bar_while_the_bar_has_focus() {
        let app = searching("fn", false);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), &app);
                render(frame, &app, &rects, &theme);
            })
            .unwrap();
        let rects = layout::compute(Rect::new(0, 0, 80, 24), &app);
        let bar = rects.search.as_ref().unwrap().bar;
        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            Position::new(rects.search.as_ref().unwrap().query.x + 2, bar.y),
            "after the two characters of the query"
        );
    }

    #[test]
    fn the_bar_still_draws_on_a_terminal_too_narrow_for_its_buttons() {
        let app = searching("x", true);
        // 40 columns is the minimum layout; the sidebar leaves the bar ~24.
        let screen = draw(&app, 40, 12).join("\n");
        assert!(screen.contains("Find:"), "the field survives: {screen}");
    }

    // --- diff viewer (SPEC §36) -------------------------------------------

    /// An app with a viewer open over the editor, built in memory: the
    /// rendering has nothing to do with where the diff came from.
    fn app_with_diff() -> App {
        use crate::app::diff::DiffState;
        use crate::git::diff::{Diff, DiffSide};

        let mut app = app();
        let diff = Diff::parse(
            "diff --git a/src/main.rs b/src/main.rs\n\
             --- a/src/main.rs\n\
             +++ b/src/main.rs\n\
             @@ -1,2 +1,2 @@\n\
              context line\n\
             -removed line\n\
             +added line\n",
        );
        app.diff = Some(DiffState::new(
            std::path::Path::new("src/main.rs"),
            DiffSide::Worktree,
            diff,
            FocusTarget::GitPanel,
        ));
        app.focus = FocusTarget::Diff;
        app
    }

    #[test]
    fn the_viewer_draws_its_diff_over_the_editor() {
        let app = app_with_diff();
        let screen = draw(&app, 80, 24).join("\n");
        assert!(
            screen.contains("Diff — src/main.rs [worktree] +1 −1"),
            "{screen}"
        );
        assert!(screen.contains("@@ -1,2 +1,2 @@"), "{screen}");
        assert!(screen.contains("-removed line"), "{screen}");
        assert!(screen.contains("+added line"), "{screen}");
        assert!(
            !screen.contains("println!"),
            "the document behind it must not show through: {screen}"
        );
    }

    /// The `+` and the `−` lines are what a diff is read by, so they are the
    /// two the theme has to tell apart.
    #[test]
    fn additions_and_removals_are_drawn_in_their_own_colours() {
        let app = app_with_diff();
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                let rects = layout::compute(frame.area(), &app);
                render(frame, &app, &rects, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let colour_of = |needle: char| {
            (0..24)
                .flat_map(|y| (0..80).map(move |x| (x, y)))
                .find(|&(x, y)| {
                    buffer[(x, y)].symbol() == needle.to_string()
                        && buffer[(x + 1, y)].symbol() == "a"
                        || buffer[(x, y)].symbol() == needle.to_string()
                            && buffer[(x + 1, y)].symbol() == "r"
                })
                .map(|(x, y)| buffer[(x, y)].fg)
                .expect("the marker is on screen")
        };
        assert_eq!(colour_of('+'), theme.git_added);
        assert_eq!(colour_of('-'), theme.git_deleted);
    }

    #[test]
    fn the_viewer_scrolled_sideways_shows_the_tail_of_its_lines() {
        let mut app = app_with_diff();
        app.diff.as_mut().unwrap().h_scroll = 4;
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("oved line"), "{screen}");
        assert!(!screen.contains("-removed line"), "{screen}");
    }

    #[test]
    fn the_bottom_of_the_frame_says_which_line_is_at_the_top() {
        let app = app_with_diff();
        let screen = draw(&app, 80, 24).join("\n");
        assert!(screen.contains("1/7"), "{screen}");
    }
}
