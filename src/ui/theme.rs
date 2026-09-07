//! UI: the colour table every widget draws from.
//!
//! Colours are 256-colour indexed rather than truecolor on purpose: Terminal.app
//! is a supported target and approximates RGB badly, while indexed colours look
//! the same everywhere including tmux and plain ssh (ADR-007).
//!
//! One default dark theme is enough for the MVP (SPEC §43). The point of this
//! struct is that no widget hardcodes a colour, so a second theme is a data
//! change rather than a sweep through `ui/`.

use ratatui::style::{Color, Modifier, Style};

use crate::syntax::highlighter::StyleKind;

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub dim: Color,
    pub border: Color,
    pub border_focused: Color,

    pub menu_bar: Style,
    pub menu_title_open: Style,
    pub menu_popup: Style,
    pub menu_item_selected: Style,
    pub menu_shortcut: Color,

    pub tab_active: Style,
    pub tab_inactive: Style,
    pub tab_dirty: Color,
    pub tab_close: Color,

    pub dialog: Style,
    pub dialog_title: Style,
    pub dialog_button: Style,
    pub dialog_button_selected: Style,
    /// The text field of an input dialog. A background of its own, so an empty
    /// field is visible as a field rather than as a gap.
    pub dialog_input: Style,

    pub panel_title: Style,
    pub selection: Style,
    pub selection_unfocused: Style,
    pub directory: Color,

    pub syntax: SyntaxTheme,

    /// The find/replace bar: its ground, its two fields, its buttons, and the
    /// background every hit that is not the current one is painted with.
    pub search_bar: Style,
    pub search_label: Color,
    pub search_field: Style,
    pub search_match: Style,
    pub search_button: Style,
    pub search_option_on: Style,

    pub line_number: Color,
    /// Selected text in the editor. Only the background is set, so the
    /// foreground stays whatever the syntax highlighter chose in Phase 7.
    pub editor_selection: Style,
    pub status_bar: Style,

    pub git_modified: Color,
    pub git_added: Color,
    pub git_deleted: Color,
    pub git_untracked: Color,
    pub git_renamed: Color,
    /// A conflicted file, which is the one git state the user has to act on
    /// before anything else works — so it is the one that is not orange.
    pub git_conflict: Color,
    /// The `@@ … @@` line of a diff. Its own colour rather than a reused one:
    /// it is the only line of the output that says *where* in the file the
    /// hunk under it is, and a reader scanning for it should not have to read
    /// the text to find it (SPEC §36).
    pub diff_hunk: Color,

    pub info: Color,
    pub warning: Color,
    pub error: Color,
}

impl Default for Theme {
    fn default() -> Self {
        let background = Color::Indexed(235);
        let foreground = Color::Indexed(252);
        let accent = Color::Indexed(75);
        let chrome = Color::Indexed(238);

        Self {
            background,
            foreground,
            dim: Color::Indexed(245),
            border: Color::Indexed(240),
            border_focused: accent,

            menu_bar: Style::new().bg(chrome).fg(foreground),
            menu_title_open: Style::new().bg(accent).fg(Color::Indexed(235)),
            menu_popup: Style::new().bg(Color::Indexed(237)).fg(foreground),
            menu_item_selected: Style::new().bg(accent).fg(Color::Indexed(235)),
            menu_shortcut: Color::Indexed(245),

            tab_active: Style::new()
                .bg(background)
                .fg(foreground)
                .add_modifier(Modifier::BOLD),
            tab_inactive: Style::new().bg(chrome).fg(Color::Indexed(245)),
            tab_dirty: Color::Indexed(215),
            tab_close: Color::Indexed(245),

            dialog: Style::new().bg(Color::Indexed(237)).fg(foreground),
            dialog_title: Style::new().fg(accent).add_modifier(Modifier::BOLD),
            dialog_input: Style::new().bg(Color::Indexed(235)).fg(foreground),
            dialog_button: Style::new().bg(Color::Indexed(237)).fg(foreground),
            dialog_button_selected: Style::new()
                .bg(accent)
                .fg(Color::Indexed(235))
                .add_modifier(Modifier::BOLD),

            panel_title: Style::new()
                .fg(Color::Indexed(245))
                .add_modifier(Modifier::BOLD),
            selection: Style::new().bg(accent).fg(Color::Indexed(235)),
            selection_unfocused: Style::new().bg(Color::Indexed(238)),
            directory: Color::Indexed(110),

            syntax: SyntaxTheme::default(),

            search_bar: Style::new().bg(chrome).fg(foreground),
            search_label: Color::Indexed(245),
            search_field: Style::new().bg(Color::Indexed(235)).fg(foreground),
            // A dim olive: visible against the editor's ground at every one of
            // the syntax foregrounds, and unmistakably not the selection's blue
            // — the current hit has to stand out from the other ones.
            search_match: Style::new().bg(Color::Indexed(58)),
            search_button: Style::new().bg(Color::Indexed(240)).fg(foreground),
            search_option_on: Style::new().bg(accent).fg(Color::Indexed(235)),

            line_number: Color::Indexed(242),
            editor_selection: Style::new().bg(Color::Indexed(24)),
            status_bar: Style::new().bg(chrome).fg(foreground),

            git_modified: Color::Indexed(215),
            git_added: Color::Indexed(114),
            git_deleted: Color::Indexed(203),
            git_untracked: Color::Indexed(245),
            git_renamed: Color::Indexed(140),
            git_conflict: Color::Indexed(196),
            // Cyan: neither of the two colours a `+` or a `−` line can be, so
            // the hunk headers stand out as the milestones they are.
            diff_hunk: Color::Indexed(80),

            info: Color::Indexed(75),
            warning: Color::Indexed(215),
            error: Color::Indexed(203),
        }
    }
}

/// The editor's colours, one per `StyleKind`.
///
/// A separate struct rather than fifteen more fields on `Theme`: these are the
/// only colours chosen by a parser rather than by a widget, and keeping them
/// together is what lets `syntax::highlighter` stay ignorant of ratatui.
#[derive(Debug, Clone)]
pub struct SyntaxTheme {
    pub text: Color,
    pub comment: Color,
    pub string: Color,
    pub number: Color,
    pub constant: Color,
    pub keyword: Color,
    pub operator: Color,
    pub function: Color,
    pub type_name: Color,
    pub variable: Color,
    pub tag: Color,
    pub attribute: Color,
    pub punctuation: Color,
    pub invalid: Color,
}

impl Default for SyntaxTheme {
    fn default() -> Self {
        // Indexed values chosen against the editor's background (235), and kept
        // clear of the chrome colours above so a keyword can never be confused
        // with a selected tab (ADR-007).
        Self {
            text: Color::Indexed(252),
            comment: Color::Indexed(243),
            string: Color::Indexed(150),
            number: Color::Indexed(173),
            constant: Color::Indexed(209),
            keyword: Color::Indexed(176),
            operator: Color::Indexed(245),
            function: Color::Indexed(111),
            type_name: Color::Indexed(180),
            variable: Color::Indexed(252),
            tag: Color::Indexed(110),
            attribute: Color::Indexed(180),
            punctuation: Color::Indexed(245),
            invalid: Color::Indexed(203),
        }
    }
}

impl SyntaxTheme {
    /// How a run of one kind is drawn.
    ///
    /// Foreground only, deliberately: the selection sets a background, and a
    /// syntax colour that also set one would paint over it (Phase 3).
    pub fn style(&self, kind: StyleKind) -> Style {
        let colour = match kind {
            StyleKind::Text => self.text,
            StyleKind::Comment => self.comment,
            StyleKind::String => self.string,
            StyleKind::Number => self.number,
            StyleKind::Constant => self.constant,
            StyleKind::Keyword => self.keyword,
            StyleKind::Operator => self.operator,
            StyleKind::Function => self.function,
            StyleKind::Type => self.type_name,
            StyleKind::Variable => self.variable,
            StyleKind::Tag => self.tag,
            StyleKind::Attribute => self.attribute,
            StyleKind::Punctuation => self.punctuation,
            // Underlined as well as red: the one kind that means "this is
            // broken" should survive a terminal whose palette flattens reds.
            StyleKind::Invalid => {
                return Style::new()
                    .fg(self.invalid)
                    .add_modifier(Modifier::UNDERLINED)
            }
        };
        Style::new().fg(colour)
    }
}

impl Theme {
    /// Border colour for a panel, given whether it currently has focus.
    pub fn border_for(&self, focused: bool) -> Style {
        Style::new().fg(if focused {
            self.border_focused
        } else {
            self.border
        })
    }
}
