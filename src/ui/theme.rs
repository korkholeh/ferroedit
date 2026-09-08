//! UI: the colour table every widget draws from.
//!
//! Colours are 256-colour indexed or one of the sixteen ANSI ones rather than
//! truecolor on purpose: Terminal.app is a supported target and approximates
//! RGB badly, while both of those look the same everywhere including tmux and
//! plain ssh (ADR-007).
//!
//! A theme is built from a `Palette` — two dozen colours with roles — rather
//! than written out field by field, so a new scheme is the twenty-four values
//! that differ and not a copy of the forty styles that do not. No widget
//! hardcodes a colour, which is what makes that possible (SPEC §43).

use ratatui::style::{Color, Modifier, Style};

use crate::config::ThemeKind;
use crate::syntax::highlighter::StyleKind;

/// The colours a theme is made of, by the job each one does.
///
/// Roles rather than names: `chrome` is "the ground the menu bar, the status
/// bar and an inactive tab share", and a theme answers what that is. A field
/// per widget would be the forty-field struct this exists to avoid.
#[derive(Debug, Clone)]
pub struct Palette {
    /// The editor's ground and the text on it.
    pub background: Color,
    pub foreground: Color,
    /// Text that is present but secondary: panel titles, shortcut hints.
    pub dim: Color,
    pub border: Color,
    /// The one colour that says "this is the thing you are on" — a focused
    /// border, a selected row, an open menu title.
    pub accent: Color,
    /// Text drawn *on* the accent.
    pub on_accent: Color,
    /// The ground of the bars: menu, status, tab strip, find bar.
    pub chrome: Color,
    pub on_chrome: Color,
    /// Text on the chrome that is secondary — an inactive tab's name.
    pub on_chrome_dim: Color,
    /// The ground of a box that floats over the rest: a dialog, a drop-down.
    pub popup: Color,
    pub on_popup: Color,
    /// The ground of a text field, so an empty one reads as a field.
    pub field: Color,
    /// The editor's selection. A background only — the syntax colours stay.
    pub selection: Color,
    /// Every search hit that is not the current one.
    pub match_bg: Color,
    pub directory: Color,
    pub line_number: Color,

    pub added: Color,
    pub removed: Color,
    pub modified: Color,
    pub renamed: Color,
    pub conflict: Color,
    pub untracked: Color,
    /// The `@@ … @@` line of a diff: neither of the two colours a `+` or a `−`
    /// line can be (SPEC §36).
    pub hunk: Color,

    pub info: Color,
    pub warning: Color,
    pub error: Color,

    pub syntax: SyntaxTheme,
}

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
    /// The dark theme, which is what an editor that was never configured
    /// shows.
    fn default() -> Self {
        Self::new(ThemeKind::default())
    }
}

impl Theme {
    pub fn new(kind: ThemeKind) -> Self {
        Self::build(palette(kind))
    }

    /// Turns twenty-four colours into the styles the widgets ask for.
    ///
    /// Every rule about *how* a colour is used lives here — that a selected
    /// menu item is the accent with `on_accent` on it, that a syntax colour
    /// never sets a background — so a new theme cannot get one of them wrong.
    pub fn build(p: Palette) -> Self {
        Self {
            background: p.background,
            foreground: p.foreground,
            dim: p.dim,
            border: p.border,
            border_focused: p.accent,

            menu_bar: Style::new().bg(p.chrome).fg(p.on_chrome),
            menu_title_open: Style::new().bg(p.accent).fg(p.on_accent),
            menu_popup: Style::new().bg(p.popup).fg(p.on_popup),
            menu_item_selected: Style::new().bg(p.accent).fg(p.on_accent),
            menu_shortcut: p.dim,

            tab_active: Style::new()
                .bg(p.background)
                .fg(p.foreground)
                .add_modifier(Modifier::BOLD),
            tab_inactive: Style::new().bg(p.chrome).fg(p.on_chrome_dim),
            tab_dirty: p.modified,
            tab_close: p.on_chrome_dim,

            dialog: Style::new().bg(p.popup).fg(p.on_popup),
            dialog_title: Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
            dialog_input: Style::new().bg(p.field).fg(p.foreground),
            dialog_button: Style::new().bg(p.popup).fg(p.on_popup),
            dialog_button_selected: Style::new()
                .bg(p.accent)
                .fg(p.on_accent)
                .add_modifier(Modifier::BOLD),

            panel_title: Style::new().fg(p.dim).add_modifier(Modifier::BOLD),
            selection: Style::new().bg(p.accent).fg(p.on_accent),
            selection_unfocused: Style::new().bg(p.chrome),
            directory: p.directory,

            syntax: p.syntax,

            search_bar: Style::new().bg(p.chrome).fg(p.on_chrome),
            search_label: p.dim,
            search_field: Style::new().bg(p.field).fg(p.foreground),
            search_match: Style::new().bg(p.match_bg),
            search_button: Style::new().bg(p.popup).fg(p.on_popup),
            search_option_on: Style::new().bg(p.accent).fg(p.on_accent),

            line_number: p.line_number,
            editor_selection: Style::new().bg(p.selection),
            status_bar: Style::new().bg(p.chrome).fg(p.on_chrome),

            git_modified: p.modified,
            git_added: p.added,
            git_deleted: p.removed,
            git_untracked: p.untracked,
            git_renamed: p.renamed,
            git_conflict: p.conflict,
            diff_hunk: p.hunk,

            info: p.info,
            warning: p.warning,
            error: p.error,
        }
    }
}

/// The colours of one theme.
fn palette(kind: ThemeKind) -> Palette {
    match kind {
        ThemeKind::Dark => dark(),
        ThemeKind::Light => light(),
        ThemeKind::DarkSimple => dark_simple(),
        ThemeKind::LightSimple => light_simple(),
        ThemeKind::Borland => borland(),
    }
}

/// Indexed greys and one blue accent, chosen against a 235 ground.
fn dark() -> Palette {
    Palette {
        background: Color::Indexed(235),
        foreground: Color::Indexed(252),
        dim: Color::Indexed(245),
        border: Color::Indexed(240),
        accent: Color::Indexed(75),
        on_accent: Color::Indexed(235),
        chrome: Color::Indexed(238),
        on_chrome: Color::Indexed(252),
        on_chrome_dim: Color::Indexed(245),
        popup: Color::Indexed(237),
        on_popup: Color::Indexed(252),
        field: Color::Indexed(235),
        selection: Color::Indexed(24),
        match_bg: Color::Indexed(58),
        directory: Color::Indexed(110),
        line_number: Color::Indexed(242),
        added: Color::Indexed(114),
        removed: Color::Indexed(203),
        modified: Color::Indexed(215),
        renamed: Color::Indexed(140),
        conflict: Color::Indexed(196),
        untracked: Color::Indexed(245),
        hunk: Color::Indexed(80),
        info: Color::Indexed(75),
        warning: Color::Indexed(215),
        error: Color::Indexed(203),
        syntax: SyntaxTheme {
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
        },
    }
}

/// The same layout on a near-white ground. The accents are darkened rather
/// than reused: a 75 blue that reads on 235 is invisible on 255.
fn light() -> Palette {
    Palette {
        background: Color::Indexed(255),
        foreground: Color::Indexed(236),
        dim: Color::Indexed(243),
        border: Color::Indexed(249),
        accent: Color::Indexed(25),
        on_accent: Color::Indexed(231),
        chrome: Color::Indexed(252),
        on_chrome: Color::Indexed(236),
        on_chrome_dim: Color::Indexed(243),
        popup: Color::Indexed(253),
        on_popup: Color::Indexed(236),
        field: Color::Indexed(231),
        selection: Color::Indexed(153),
        match_bg: Color::Indexed(222),
        directory: Color::Indexed(25),
        line_number: Color::Indexed(246),
        added: Color::Indexed(28),
        removed: Color::Indexed(124),
        modified: Color::Indexed(130),
        renamed: Color::Indexed(90),
        conflict: Color::Indexed(160),
        untracked: Color::Indexed(243),
        hunk: Color::Indexed(30),
        info: Color::Indexed(25),
        warning: Color::Indexed(130),
        error: Color::Indexed(124),
        syntax: SyntaxTheme {
            text: Color::Indexed(236),
            comment: Color::Indexed(243),
            string: Color::Indexed(28),
            number: Color::Indexed(130),
            constant: Color::Indexed(166),
            keyword: Color::Indexed(90),
            operator: Color::Indexed(240),
            function: Color::Indexed(26),
            type_name: Color::Indexed(94),
            variable: Color::Indexed(236),
            tag: Color::Indexed(30),
            attribute: Color::Indexed(94),
            punctuation: Color::Indexed(240),
            invalid: Color::Indexed(124),
        },
    }
}

/// Sixteen ANSI colours and nothing else, so the terminal's own palette is
/// what the editor is drawn in.
fn dark_simple() -> Palette {
    Palette {
        background: Color::Black,
        foreground: Color::Gray,
        dim: Color::DarkGray,
        border: Color::DarkGray,
        accent: Color::Blue,
        on_accent: Color::White,
        chrome: Color::DarkGray,
        on_chrome: Color::White,
        on_chrome_dim: Color::Gray,
        popup: Color::DarkGray,
        on_popup: Color::White,
        field: Color::Black,
        selection: Color::Blue,
        match_bg: Color::Magenta,
        directory: Color::LightBlue,
        line_number: Color::DarkGray,
        added: Color::Green,
        removed: Color::Red,
        modified: Color::Yellow,
        renamed: Color::Magenta,
        conflict: Color::LightRed,
        untracked: Color::DarkGray,
        hunk: Color::Cyan,
        info: Color::Cyan,
        warning: Color::Yellow,
        error: Color::LightRed,
        syntax: SyntaxTheme {
            text: Color::Gray,
            comment: Color::DarkGray,
            string: Color::Green,
            number: Color::Cyan,
            constant: Color::Cyan,
            keyword: Color::Yellow,
            operator: Color::Gray,
            function: Color::LightBlue,
            type_name: Color::LightCyan,
            variable: Color::Gray,
            tag: Color::LightBlue,
            attribute: Color::Cyan,
            punctuation: Color::Gray,
            invalid: Color::LightRed,
        },
    }
}

/// The ANSI sixteen again, on a white ground. The bright variants are gone:
/// `LightYellow` on white is not a colour, it is a rumour.
fn light_simple() -> Palette {
    Palette {
        background: Color::White,
        foreground: Color::Black,
        dim: Color::DarkGray,
        border: Color::DarkGray,
        accent: Color::Blue,
        on_accent: Color::White,
        chrome: Color::Gray,
        on_chrome: Color::Black,
        on_chrome_dim: Color::DarkGray,
        popup: Color::Gray,
        on_popup: Color::Black,
        field: Color::White,
        selection: Color::LightBlue,
        match_bg: Color::LightYellow,
        directory: Color::Blue,
        line_number: Color::DarkGray,
        added: Color::Green,
        removed: Color::Red,
        modified: Color::Blue,
        renamed: Color::Magenta,
        conflict: Color::Red,
        untracked: Color::DarkGray,
        hunk: Color::Blue,
        info: Color::Blue,
        warning: Color::Magenta,
        error: Color::Red,
        syntax: SyntaxTheme {
            text: Color::Black,
            comment: Color::DarkGray,
            string: Color::Green,
            number: Color::Magenta,
            constant: Color::Magenta,
            keyword: Color::Blue,
            operator: Color::Black,
            function: Color::Blue,
            type_name: Color::Magenta,
            variable: Color::Black,
            tag: Color::Blue,
            attribute: Color::Magenta,
            punctuation: Color::DarkGray,
            invalid: Color::Red,
        },
    }
}

/// Turbo Vision: a blue editor, yellow text, cyan accents, and grey boxes with
/// black on them for everything that floats above it.
fn borland() -> Palette {
    Palette {
        background: Color::Indexed(18),
        foreground: Color::Indexed(228),
        dim: Color::Indexed(250),
        border: Color::Indexed(45),
        accent: Color::Indexed(51),
        on_accent: Color::Indexed(18),
        chrome: Color::Indexed(250),
        on_chrome: Color::Indexed(16),
        on_chrome_dim: Color::Indexed(240),
        popup: Color::Indexed(250),
        on_popup: Color::Indexed(16),
        field: Color::Indexed(18),
        selection: Color::Indexed(31),
        match_bg: Color::Indexed(90),
        directory: Color::Indexed(51),
        line_number: Color::Indexed(245),
        added: Color::Indexed(46),
        removed: Color::Indexed(203),
        modified: Color::Indexed(226),
        renamed: Color::Indexed(213),
        conflict: Color::Indexed(196),
        untracked: Color::Indexed(250),
        hunk: Color::Indexed(51),
        info: Color::Indexed(51),
        warning: Color::Indexed(226),
        error: Color::Indexed(203),
        syntax: SyntaxTheme {
            text: Color::Indexed(228),
            comment: Color::Indexed(248),
            string: Color::Indexed(51),
            number: Color::Indexed(51),
            constant: Color::Indexed(51),
            keyword: Color::Indexed(231),
            operator: Color::Indexed(231),
            function: Color::Indexed(228),
            type_name: Color::Indexed(231),
            variable: Color::Indexed(228),
            tag: Color::Indexed(231),
            attribute: Color::Indexed(51),
            punctuation: Color::Indexed(231),
            invalid: Color::Indexed(203),
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every theme is a whole theme: the builder is what fills the forty
    /// styles in, so this is really the check that no palette is missing.
    #[test]
    fn every_theme_builds_and_has_a_ground_of_its_own() {
        let grounds: Vec<Color> = ThemeKind::ALL
            .iter()
            .map(|kind| Theme::new(*kind).background)
            .collect();
        for (i, ground) in grounds.iter().enumerate() {
            for (j, other) in grounds.iter().enumerate() {
                assert!(
                    i == j || ground != other,
                    "{} and {} are the same theme",
                    ThemeKind::ALL[i].label(),
                    ThemeKind::ALL[j].label()
                );
            }
        }
    }

    /// The default is the dark theme, which is what an unconfigured editor
    /// showed before there was anything to configure.
    #[test]
    fn the_default_theme_is_the_dark_one() {
        assert_eq!(
            Theme::default().background,
            Theme::new(ThemeKind::Dark).background
        );
    }

    /// The simplified palettes are the sixteen ANSI colours, so that the
    /// terminal's own scheme is what the editor is drawn in.
    #[test]
    fn the_simple_themes_use_no_indexed_colours() {
        for kind in [ThemeKind::DarkSimple, ThemeKind::LightSimple] {
            let theme = Theme::new(kind);
            for colour in [
                theme.background,
                theme.foreground,
                theme.dim,
                theme.border,
                theme.border_focused,
                theme.directory,
                theme.line_number,
                theme.git_added,
                theme.git_deleted,
                theme.diff_hunk,
                theme.syntax.keyword,
                theme.syntax.string,
            ] {
                assert!(
                    !matches!(colour, Color::Indexed(_)),
                    "{} uses {colour:?}",
                    kind.label()
                );
            }
        }
    }
}
