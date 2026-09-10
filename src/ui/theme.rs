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
    /// The colour of a frame that has focus, and of a mark on the editor's
    /// ground that has to catch the eye. Always a line or a glyph, never a
    /// ground — text on it is `highlight`'s job.
    pub accent: Color,
    /// The bar that says "this is the row you are on": an open menu's title,
    /// the item under the cursor in its drop-down, a selected row, the button
    /// Enter would press, a find option that is on.
    ///
    /// Its own role rather than `accent`, because the two are only the same
    /// colour by coincidence: `accent` is a *line* on the editor's ground and
    /// this is a *ground* under black text, and a scheme whose highlight is a
    /// green bar has no use for green frames.
    pub highlight: Color,
    /// Text drawn on that bar.
    pub on_highlight: Color,
    /// A dialog's title, drawn on `popup`. `accent` for most themes — but a
    /// theme whose accent is the colour of its frames *on the ground* cannot
    /// put that colour on a grey box and still be read.
    pub title: Color,
    /// The ground of the bars: menu, status, tab strip, find bar.
    pub chrome: Color,
    /// The tab strip's ground: a shade off `chrome`, and not `background`
    /// (ADR-072). The bar under the menu is neither the menu nor the pane, and
    /// a strip that is either of them makes one of the two boundaries vanish.
    /// The sixteen-colour schemes have no third tone to spend here and fall
    /// back to `background`.
    pub tab_strip: Color,
    pub on_chrome: Color,
    /// Text on the chrome that is secondary — an inactive tab's name.
    pub on_chrome_dim: Color,
    /// The ground of a box that floats over the rest: a dialog, a drop-down.
    pub popup: Color,
    pub on_popup: Color,
    /// The frame of such a box, and the rules drawn inside it. Its own role
    /// rather than `border`, which is drawn on the editor's ground: a theme
    /// whose popups are light and whose ground is dark needs the two frames
    /// to be different colours or one of them disappears.
    pub popup_border: Color,
    /// Secondary text on a popup — a menu's shortcut column. Its own role
    /// rather than `on_chrome_dim`, because a theme is free to make its
    /// drop-downs darker than its bars and then the two dims differ.
    pub on_popup_dim: Color,
    /// The ground of a text field, so an empty one reads as a field.
    ///
    /// A field is drawn both inside a dialog and on the find bar, so this is
    /// the one colour that has to stand apart from `popup` and `chrome` both.
    pub field: Color,
    /// What is typed into a field. `foreground` for most themes, but a theme
    /// whose only free dark tone is a light grey needs the text on it dark.
    pub on_field: Color,
    /// The editor's selection. A background only — the syntax colours stay.
    pub selection: Color,
    /// The ground of the line the caret is on, when the palette has a tone
    /// close enough to `background` to mark a whole line without shouting.
    ///
    /// `None` is an honest answer: the sixteen ANSI colours have nothing
    /// between black and grey, and a line drawn on the grey of the bars is a
    /// selection bar rather than a hint about where the caret is (ADR-067).
    pub current_line: Option<Color>,
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
    /// The frame of a floating box — a drop-down, a dialog — and the rules
    /// drawn inside it. Not `border`, which is the frame of a pane on the
    /// editor's ground: a theme whose boxes are light and whose ground is
    /// dark needs the two to be different colours.
    pub popup_border: Color,
    /// The bar under the cursor, as a colour rather than a style: the thumb
    /// of a dialog's scrollbar is drawn in it.
    pub highlight: Color,
    /// Secondary text drawn *on* the chrome — a status-bar readout, the hit
    /// count in the find bar. Not `dim`, which is the same thing on the
    /// editor's ground: a theme whose bars are lighter than its ground needs
    /// the two to be different colours, and the retro theme's are the same grey.
    pub chrome_dim: Color,

    pub tab_active: Style,
    pub tab_inactive: Style,
    /// The strip itself, tabs and all: a shade off the chrome the menu is cut
    /// from, and not the pane's ground either (ADR-072). Two rows of the same
    /// grey stacked on each other left the Retro scheme with a slab across the
    /// top of the window that read as one bar.
    pub tab_strip: Style,
    /// The rule between one tab and the next.
    pub tab_separator: Color,
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
    /// The ground of the line the caret is on, on the themes that have one.
    /// A background only, for the same reason: the syntax colours, the
    /// selection and the search hits are all drawn over it (ADR-067).
    pub current_line: Option<Style>,
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
            menu_title_open: Style::new().bg(p.highlight).fg(p.on_highlight),
            menu_popup: Style::new().bg(p.popup).fg(p.on_popup),
            menu_item_selected: Style::new().bg(p.highlight).fg(p.on_highlight),
            // A shortcut is drawn in the menu's drop-down, so it is dim
            // against `popup` rather than against the editor's ground.
            menu_shortcut: p.on_popup_dim,
            popup_border: p.popup_border,
            highlight: p.highlight,
            chrome_dim: p.on_chrome_dim,

            // The tab in front carries the editor's own ground, so the file
            // being edited reads as continuous with the pane under it; the
            // ones behind sit on the strip (ADR-072).
            tab_active: Style::new()
                .bg(p.background)
                .fg(p.foreground)
                .add_modifier(Modifier::BOLD),
            tab_inactive: Style::new().bg(p.tab_strip).fg(p.on_chrome_dim),
            tab_strip: Style::new().bg(p.tab_strip).fg(p.on_chrome_dim),
            tab_separator: p.on_chrome_dim,
            tab_dirty: p.modified,
            tab_close: p.on_chrome_dim,

            dialog: Style::new().bg(p.popup).fg(p.on_popup),
            dialog_title: Style::new().fg(p.title).add_modifier(Modifier::BOLD),
            dialog_input: Style::new().bg(p.field).fg(p.on_field),
            dialog_button: Style::new().bg(p.popup).fg(p.on_popup),
            dialog_button_selected: Style::new()
                .bg(p.highlight)
                .fg(p.on_highlight)
                .add_modifier(Modifier::BOLD),

            panel_title: Style::new().fg(p.dim).add_modifier(Modifier::BOLD),
            selection: Style::new().bg(p.highlight).fg(p.on_highlight),
            selection_unfocused: Style::new().bg(p.chrome),
            directory: p.directory,

            syntax: p.syntax,

            search_bar: Style::new().bg(p.chrome).fg(p.on_chrome),
            search_label: p.dim,
            search_field: Style::new().bg(p.field).fg(p.on_field),
            search_match: Style::new().bg(p.match_bg),
            search_button: Style::new().bg(p.popup).fg(p.on_popup),
            search_option_on: Style::new().bg(p.highlight).fg(p.on_highlight),

            line_number: p.line_number,
            editor_selection: Style::new().bg(p.selection),
            current_line: p.current_line.map(|bg| Style::new().bg(bg)),
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
        ThemeKind::Retro => retro(),
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
        highlight: Color::Indexed(75),
        on_highlight: Color::Indexed(235),
        title: Color::Indexed(75),
        chrome: Color::Indexed(238),
        // One step under the bars, two over the ground.
        tab_strip: Color::Indexed(237),
        on_chrome: Color::Indexed(252),
        on_chrome_dim: Color::Indexed(245),
        popup: Color::Indexed(237),
        on_popup: Color::Indexed(252),
        popup_border: Color::Indexed(240),
        on_popup_dim: Color::Indexed(245),
        field: Color::Indexed(235),
        on_field: Color::Indexed(252),
        selection: Color::Indexed(24),
        // Two steps off the 235 ground: enough to find the caret's line at a
        // glance, not enough to read as a selection.
        current_line: Some(Color::Indexed(237)),
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
        highlight: Color::Indexed(25),
        on_highlight: Color::Indexed(231),
        title: Color::Indexed(25),
        chrome: Color::Indexed(252),
        tab_strip: Color::Indexed(251),
        on_chrome: Color::Indexed(236),
        on_chrome_dim: Color::Indexed(243),
        popup: Color::Indexed(253),
        on_popup: Color::Indexed(236),
        popup_border: Color::Indexed(249),
        on_popup_dim: Color::Indexed(243),
        field: Color::Indexed(231),
        on_field: Color::Indexed(236),
        selection: Color::Indexed(153),
        current_line: Some(Color::Indexed(253)),
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
        foreground: Color::White,
        // Not `DarkGray`: the bars and the dialogs are drawn on it, and dim
        // text on them has to stay text.
        dim: Color::Gray,
        border: Color::DarkGray,
        accent: Color::Blue,
        highlight: Color::Blue,
        on_highlight: Color::White,
        title: Color::Blue,
        chrome: Color::DarkGray,
        // Nothing sits between `DarkGray` and black, so the strip takes the
        // ground: the tabs are still off the menu bar, which is the boundary
        // this scheme can afford to keep.
        tab_strip: Color::Black,
        on_chrome: Color::White,
        on_chrome_dim: Color::Gray,
        // Black, not the bars' grey: a drop-down that is the same colour as
        // the bar it hangs from does not read as a box over it, and the rule
        // between two groups of a menu — drawn in `border` — disappears.
        popup: Color::Black,
        on_popup: Color::White,
        popup_border: Color::DarkGray,
        on_popup_dim: Color::Gray,
        // The third dark tone, because a field is drawn on the grey of the
        // find bar and on the black of a dialog and has to be neither.
        field: Color::Gray,
        on_field: Color::Black,
        selection: Color::Blue,
        // Nothing sits between black and `DarkGray`, and the grey is the
        // ground of the bars: this scheme marks the caret's line with the bold
        // number in the gutter and nothing else (ADR-067).
        current_line: None,
        match_bg: Color::Magenta,
        // Cyan rather than blue: a directory is read on the black ground and
        // again on the grey of a selected row the sidebar does not have focus
        // in, and blue is only legible on the first of the two.
        directory: Color::LightCyan,
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
            text: Color::White,
            comment: Color::DarkGray,
            string: Color::Green,
            number: Color::Cyan,
            constant: Color::Cyan,
            keyword: Color::Yellow,
            operator: Color::Gray,
            function: Color::LightBlue,
            type_name: Color::LightCyan,
            variable: Color::White,
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
        highlight: Color::Blue,
        on_highlight: Color::White,
        title: Color::Blue,
        chrome: Color::Gray,
        // As in the dark scheme: no tone between `Gray` and white to spend.
        tab_strip: Color::White,
        on_chrome: Color::Black,
        on_chrome_dim: Color::DarkGray,
        popup: Color::Gray,
        on_popup: Color::Black,
        popup_border: Color::DarkGray,
        on_popup_dim: Color::DarkGray,
        field: Color::White,
        on_field: Color::Black,
        selection: Color::LightBlue,
        current_line: None,
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

/// The DOS-era full-screen editor: a blue ground, yellow text, grey boxes with
/// black on them for everything that floats above it, and the green bar that
/// marks the menu item under the cursor.
fn retro() -> Palette {
    Palette {
        background: Color::Indexed(18),
        foreground: Color::Indexed(228),
        dim: Color::Indexed(250),
        border: Color::Indexed(45),
        // Cyan frames on the blue, as the panes of this scheme always had.
        accent: Color::Indexed(51),
        // Green, and black on it: the bar under the cursor in a menu, on the
        // button Enter would press and down a list is the one colour of this
        // scheme nobody who has seen it forgets. It is not the frame colour,
        // which is why `highlight` is not `accent` here.
        highlight: Color::Indexed(34),
        on_highlight: Color::Indexed(16),
        // Black: a dialog's title is drawn on the grey box, where the cyan
        // that reads so well on the blue is barely a colour at all.
        title: Color::Indexed(16),
        // 248 is the grey the hardware palette actually had, and the bars,
        // the drop-downs and the dialogs are all cut from it.
        chrome: Color::Indexed(248),
        // Two steps under the grey of the bars: this scheme's chrome is one
        // flat tone, and a single step of it does not read as an edge.
        tab_strip: Color::Indexed(246),
        on_chrome: Color::Indexed(16),
        on_chrome_dim: Color::Indexed(238),
        popup: Color::Indexed(248),
        on_popup: Color::Indexed(16),
        on_popup_dim: Color::Indexed(238),
        // Black: a drop-down of this scheme is a grey box with a black frame
        // sitting on the blue, and the cyan `border` — which is what the
        // frames *on* the blue are — is barely there against the grey.
        popup_border: Color::Indexed(16),
        field: Color::Indexed(18),
        on_field: Color::Indexed(228),
        selection: Color::Indexed(31),
        // A blue one shade off the ground's, which is what the hardware
        // palette had between 18 and the cyan.
        current_line: Some(Color::Indexed(19)),
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

    /// Text has to be a different colour from what it is drawn on.
    ///
    /// The pairs are the ones a widget actually puts together, and the check
    /// exists because `dim` used to be all three of them: it is the colour of
    /// the status readout, of a menu shortcut and of a panel title, which are
    /// drawn on the chrome, on a popup and on the editor's ground in turn. A
    /// theme whose bars are the same grey as its dim text loses the readout
    /// entirely, and nothing but a check says so before someone runs it.
    #[test]
    fn no_theme_draws_text_in_the_colour_underneath_it() {
        for kind in ThemeKind::ALL.iter().copied() {
            let p = palette(kind);
            let t = Theme::new(kind);
            for (text, ground, what) in [
                (t.chrome_dim, p.chrome, "the status readout on the chrome"),
                (t.menu_shortcut, p.popup, "a menu shortcut on a drop-down"),
                (p.on_popup, p.popup, "a menu item"),
                // The rule between two groups of a menu is the border colour
                // drawn on the popup, and a menu whose groups run together is
                // how this whole check started.
                (t.popup_border, p.popup, "the rule between two menu groups"),
                (p.on_highlight, p.highlight, "the row under the cursor"),
                (p.title, p.popup, "a dialog's title"),
                (
                    t.popup_border,
                    p.background,
                    "a dialog's frame over the ground",
                ),
                (p.on_field, p.field, "text in an input field"),
                (p.field, p.popup, "a dialog's input field"),
                (p.field, p.chrome, "the find bar's input field"),
                (t.dim, p.background, "a panel title on the ground"),
                (t.directory, p.background, "a directory in the sidebar"),
                (
                    t.directory,
                    p.chrome,
                    "a directory on an unfocused selection",
                ),
                (t.foreground, p.background, "a file name in the sidebar"),
                (t.line_number, p.background, "a line number"),
                (p.on_chrome, p.tab_strip, "the tab in front"),
                (p.on_chrome_dim, p.tab_strip, "a tab behind it"),
            ] {
                assert_ne!(text, ground, "{}: {what} is invisible", kind.label());
            }
        }
    }

    /// A ground that is the ground under it marks nothing, and one the text
    /// cannot be read on marks too much: the line the caret is on has to be a
    /// tone of its own that the foreground and the syntax colours survive
    /// (ADR-067).
    #[test]
    fn the_caret_line_ground_is_neither_the_background_nor_the_text() {
        for kind in ThemeKind::ALL.iter().copied() {
            let p = palette(kind);
            let Some(ground) = p.current_line else {
                continue;
            };
            assert_ne!(ground, p.background, "{}: an invisible mark", kind.label());
            for (colour, what) in [
                (p.foreground, "the text"),
                (p.syntax.comment, "a comment"),
                (p.syntax.string, "a string"),
                (p.line_number, "the line number"),
                (p.selection, "the selection"),
                (p.match_bg, "a search hit"),
            ] {
                assert_ne!(
                    colour,
                    ground,
                    "{}: {what} is lost on the caret's line",
                    kind.label()
                );
            }
        }
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
            // These palettes have no tone between the ground and the grey of
            // the bars, so they mark the caret's line in the gutter only.
            assert!(theme.current_line.is_none(), "{}", kind.label());
        }
    }
}
