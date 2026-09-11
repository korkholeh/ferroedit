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
//!
//! Secondary text is held to a contrast ratio and not merely to "a different
//! colour" (ADR-083): the check at the bottom of this file computes WCAG
//! contrast against the xterm-256 palette, which is what a comment three
//! shades off its ground fails and a `assert_ne!` does not.

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
    /// The ground of the bars: menu, status, tab strip.
    pub chrome: Color,
    /// The ground of the find/replace bar.
    ///
    /// Its own tone rather than `chrome`, because the bar is wedged between
    /// the editor's ground above it and the status bar's chrome below it: a
    /// bar cut from the same grey as the status bar reads as one two-row bar,
    /// and the fields in it read as part of the editor. A scheme with no third
    /// tone to spend falls back to `chrome`, as the tab strip does.
    pub find_bar: Color,
    /// The tab strip's ground: a shade off `chrome`, and not `background`
    /// (ADR-072). The bar under the menu is neither the menu nor the pane, and
    /// a strip that is either of them makes one of the two boundaries vanish.
    /// The sixteen-colour schemes have no third tone to spend here and fall
    /// back to `background`.
    pub tab_strip: Color,
    pub on_chrome: Color,
    /// Text on the chrome that is secondary — an inactive tab's name. Dim
    /// enough that the tab in front still reads as the one in front.
    pub on_chrome_dim: Color,
    /// Secondary text on the bars that is *read* rather than glanced at: the
    /// status readout, the find bar's hit count.
    ///
    /// Its own role rather than `on_chrome_dim`, which is the colour of a name
    /// the user is deliberately not looking at. A readout dim enough to keep a
    /// tab behind the one in front is a readout nobody can read — which is
    /// exactly what grey-on-grey status text was.
    pub chrome_readout: Color,
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
    /// The label of a menu entry the state has greyed out (ADR-084).
    ///
    /// A third tier and not `on_popup_dim`, which is held to 4:1 because the
    /// shortcut column is *read*: the dark theme's is 250 against a label's
    /// 252, two steps apart on the greyscale, and a greyed entry drawn in it
    /// was indistinguishable from a live one. This one is deliberately quiet —
    /// half the contrast of a label at most — because a row that cannot be
    /// pressed should be recognised as such before it is read.
    pub on_popup_disabled: Color,
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
    /// The ground of the selected row of a pane that does *not* have focus.
    ///
    /// A tone of the pane's own ground rather than the chrome, because the
    /// text drawn on it is the text the pane draws everywhere else — a git
    /// status letter, a directory name — and those colours were chosen against
    /// that ground. The Retro scheme is what made this a role: its panes are
    /// blue and its chrome is light grey, so a selected row cut from the chrome
    /// put yellow on grey and lost the row it was marking.
    pub selection_dim: Color,
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
    /// The ground under the part of a replaced line that actually changed
    /// (ADR-082) — one for the line it was taken out of and one for the line it
    /// went into.
    ///
    /// Grounds and not foregrounds: the line already *has* a foreground, which
    /// is what says whether it was added or removed, and a mark that changed it
    /// would be answering a question that has been answered. Dark enough that
    /// the `+` and `−` colours are still read on them.
    pub added_word: Color,
    pub removed_word: Color,

    /// The three colours a notification is drawn in.
    ///
    /// Bar colours, all three: a notification is the left half of the status
    /// bar, the find bar says `0/0` in the warning colour, a stale tab is
    /// marked in it on the tab strip, and the notice a too-small terminal gets
    /// is painted on the chrome for exactly this reason. So they are chosen
    /// against `chrome` and not against the editor's ground — which in the
    /// Retro scheme are a light grey and a dark blue, and no one colour reads
    /// on both.
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
    /// A menu entry the state has greyed out (ADR-084).
    pub menu_disabled: Color,
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
    /// The label in front of a field the caret is *not* in.
    pub search_label: Color,
    /// The label in front of the field the caret is in: the highlight bar,
    /// with the text colour that goes on it.
    ///
    /// A bar and not a colour, for the reason `search_option_on` is one — and
    /// because the accent, which this was first, is a *frame* colour on the
    /// editor's ground: the Retro scheme's is a cyan that is 1.2:1 on the light
    /// grey its find bar is cut from. Every theme already guarantees that its
    /// highlight and the text on it can be read together.
    pub search_label_focused: Style,
    pub search_field: Style,
    /// The field the caret is in: underlined as well as grounded, because a
    /// colour difference alone is what the bar was already failing to say.
    pub search_field_focused: Style,
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
    /// The two grounds that mark the changed part of a replaced line
    /// (ADR-082). Backgrounds only: the line keeps the colour that says which
    /// side of the change it is.
    pub diff_word_added: Style,
    pub diff_word_removed: Style,

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
            menu_disabled: p.on_popup_disabled,
            popup_border: p.popup_border,
            highlight: p.highlight,
            chrome_dim: p.chrome_readout,

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
            selection_unfocused: Style::new().bg(p.selection_dim),
            directory: p.directory,

            syntax: p.syntax,

            search_bar: Style::new().bg(p.find_bar).fg(p.on_chrome),
            // On the bar, not on the editor's ground: `dim` is the colour of a
            // panel title over the pane, and a label drawn in it on a lighter
            // bar was the greyest thing on screen.
            search_label: p.chrome_readout,
            search_label_focused: Style::new()
                .bg(p.highlight)
                .fg(p.on_highlight)
                .add_modifier(Modifier::BOLD),
            search_field: Style::new().bg(p.field).fg(p.on_field),
            search_field_focused: Style::new()
                .bg(p.field)
                .fg(p.on_field)
                .add_modifier(Modifier::UNDERLINED),
            search_match: Style::new().bg(p.match_bg),
            search_button: Style::new().bg(p.popup).fg(p.on_popup),
            search_option_on: Style::new()
                .bg(p.highlight)
                .fg(p.on_highlight)
                .add_modifier(Modifier::BOLD),

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
            diff_word_added: Style::new().bg(p.added_word),
            diff_word_removed: Style::new().bg(p.removed_word),

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
        // 5.7:1 on the ground. It was 245 — 4.4:1 — which is the floor for
        // text nobody has to read, and panel titles and comments are read.
        dim: Color::Indexed(247),
        border: Color::Indexed(240),
        accent: Color::Indexed(75),
        highlight: Color::Indexed(75),
        on_highlight: Color::Indexed(235),
        title: Color::Indexed(75),
        chrome: Color::Indexed(238),
        // A step over the bars, four over the ground: the one row between the
        // editor and the status bar that has to read as neither.
        find_bar: Color::Indexed(239),
        // One step under the bars, two over the ground.
        tab_strip: Color::Indexed(237),
        on_chrome: Color::Indexed(252),
        on_chrome_dim: Color::Indexed(248),
        // 5.1:1 on the bars against the tab colour's 4.1:1 — the readout is
        // read and a tab behind the one in front is not.
        chrome_readout: Color::Indexed(250),
        popup: Color::Indexed(237),
        on_popup: Color::Indexed(252),
        popup_border: Color::Indexed(240),
        on_popup_dim: Color::Indexed(250),
        // 2.9:1 against a label's 7.4 and a shortcut's 6.0.
        on_popup_disabled: Color::Indexed(244),
        // Under the editor's ground and not equal to it. It was 235 — the
        // ground itself — which made a dialog's field a well on the popup and
        // the find bar's fields an unbroken continuation of the text above
        // them: two rows of editor with a word `Find:` in front of them.
        field: Color::Indexed(233),
        on_field: Color::Indexed(252),
        selection: Color::Indexed(24),
        // The chrome's own grey: three steps over the ground, which marks the
        // row without competing with the focused pane's blue.
        selection_dim: Color::Indexed(238),
        // Two steps off the 235 ground: enough to find the caret's line at a
        // glance, not enough to read as a selection.
        current_line: Some(Color::Indexed(237)),
        match_bg: Color::Indexed(58),
        directory: Color::Indexed(110),
        line_number: Color::Indexed(245),
        added: Color::Indexed(114),
        removed: Color::Indexed(203),
        modified: Color::Indexed(215),
        renamed: Color::Indexed(140),
        conflict: Color::Indexed(196),
        // Grey by role, but a grey that survives the row being selected: at
        // 245 an untracked file on an unfocused selection was 2.8:1.
        untracked: Color::Indexed(248),
        hunk: Color::Indexed(80),
        // Deep green and deep red: the `+` and `−` colours read on both, and
        // both read as a mark on the 235 ground rather than as a second theme.
        added_word: Color::Indexed(22),
        removed_word: Color::Indexed(52),
        info: Color::Indexed(75),
        warning: Color::Indexed(215),
        // A step lighter than the red a removed line is drawn in: this one is
        // read on the bars, which are lighter than the ground that one is on.
        error: Color::Indexed(210),
        syntax: SyntaxTheme {
            text: Color::Indexed(252),
            // 5:1 on the ground. A comment is secondary, not unavailable.
            comment: Color::Indexed(246),
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
        dim: Color::Indexed(240),
        border: Color::Indexed(249),
        accent: Color::Indexed(25),
        highlight: Color::Indexed(25),
        on_highlight: Color::Indexed(231),
        title: Color::Indexed(25),
        chrome: Color::Indexed(252),
        // Two steps under the bars: on a light scheme the bar that is not the
        // status bar is the darker of the two.
        find_bar: Color::Indexed(250),
        tab_strip: Color::Indexed(251),
        on_chrome: Color::Indexed(236),
        on_chrome_dim: Color::Indexed(240),
        chrome_readout: Color::Indexed(238),
        popup: Color::Indexed(253),
        on_popup: Color::Indexed(236),
        popup_border: Color::Indexed(249),
        on_popup_dim: Color::Indexed(240),
        // 2.8:1 against a label's 9.4 and a shortcut's 5.1.
        on_popup_disabled: Color::Indexed(244),
        field: Color::Indexed(231),
        on_field: Color::Indexed(236),
        selection: Color::Indexed(153),
        selection_dim: Color::Indexed(252),
        current_line: Some(Color::Indexed(253)),
        match_bg: Color::Indexed(222),
        directory: Color::Indexed(25),
        line_number: Color::Indexed(242),
        added: Color::Indexed(28),
        removed: Color::Indexed(124),
        modified: Color::Indexed(130),
        renamed: Color::Indexed(90),
        conflict: Color::Indexed(160),
        untracked: Color::Indexed(240),
        hunk: Color::Indexed(30),
        // Pale green and pale red, which is the same idea the other way up.
        added_word: Color::Indexed(194),
        removed_word: Color::Indexed(224),
        info: Color::Indexed(25),
        // Darker than the orange a modified file is marked with, for the
        // reason the dark scheme's error is lighter: the bars are not the
        // ground.
        warning: Color::Indexed(94),
        error: Color::Indexed(124),
        syntax: SyntaxTheme {
            text: Color::Indexed(236),
            comment: Color::Indexed(240),
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
        // The bars' own grey: sixteen colours leave nothing between it and
        // black to spend on a bar of its own, and a find bar drawn in black
        // would be the editor rather than a bar. The field on it is `Gray`,
        // which is what keeps the two rows apart here.
        find_bar: Color::DarkGray,
        // Nothing sits between `DarkGray` and black, so the strip takes the
        // ground: the tabs are still off the menu bar, which is the boundary
        // this scheme can afford to keep.
        tab_strip: Color::Black,
        on_chrome: Color::White,
        on_chrome_dim: Color::Gray,
        // No third light tone: the readout is the same grey the tabs are, and
        // the terminal's own palette decides how far off white that is.
        chrome_readout: Color::Gray,
        // Black, not the bars' grey: a drop-down that is the same colour as
        // the bar it hangs from does not read as a box over it, and the rule
        // between two groups of a menu — drawn in `border` — disappears.
        popup: Color::Black,
        on_popup: Color::White,
        popup_border: Color::DarkGray,
        on_popup_dim: Color::Gray,
        // The fourth tone this scheme has: White label, Gray shortcut,
        // DarkGray for a row that cannot be pressed.
        on_popup_disabled: Color::DarkGray,
        // The third dark tone, because a field is drawn on the grey of the
        // find bar and on the black of a dialog and has to be neither.
        field: Color::Gray,
        on_field: Color::Black,
        selection: Color::Blue,
        selection_dim: Color::DarkGray,
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
        // Sixteen colours and no free tone: the mark is the bars' own grey,
        // which the green and the red are both read on.
        added_word: Color::DarkGray,
        removed_word: Color::DarkGray,
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
        // As in the dark scheme: no tone to spend on a bar of its own.
        find_bar: Color::Gray,
        // As in the dark scheme: no tone between `Gray` and white to spend.
        tab_strip: Color::White,
        on_chrome: Color::Black,
        on_chrome_dim: Color::DarkGray,
        chrome_readout: Color::DarkGray,
        popup: Color::Gray,
        on_popup: Color::Black,
        popup_border: Color::DarkGray,
        on_popup_dim: Color::DarkGray,
        // The shortcut's colour again: a sixteen-colour scheme has no fourth
        // grey, and a Black label beside a DarkGray one is already the
        // difference this has to say.
        on_popup_disabled: Color::DarkGray,
        field: Color::White,
        on_field: Color::Black,
        selection: Color::LightBlue,
        selection_dim: Color::Gray,
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
        added_word: Color::Gray,
        removed_word: Color::Gray,
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
        // Four steps *over* the grey of the bars, in the other direction from
        // the tab strip: the find bar has a bar above it and a bar below it,
        // so it has to be lighter than both rather than merely different.
        find_bar: Color::Indexed(252),
        // Two steps under the grey of the bars: this scheme's chrome is one
        // flat tone, and a single step of it does not read as an edge.
        tab_strip: Color::Indexed(246),
        on_chrome: Color::Indexed(16),
        // 4.4:1 on the tab strip. It was 238, which on this scheme's one flat
        // grey left an inactive tab's name at 3.2.
        on_chrome_dim: Color::Indexed(236),
        chrome_readout: Color::Indexed(235),
        popup: Color::Indexed(248),
        on_popup: Color::Indexed(16),
        on_popup_dim: Color::Indexed(236),
        // 3.0:1 against a label's 8.8 and a shortcut's 5.6.
        on_popup_disabled: Color::Indexed(240),
        // Black: a drop-down of this scheme is a grey box with a black frame
        // sitting on the blue, and the cyan `border` — which is what the
        // frames *on* the blue are — is barely there against the grey.
        popup_border: Color::Indexed(16),
        field: Color::Indexed(18),
        on_field: Color::Indexed(228),
        selection: Color::Indexed(31),
        // Blue and not the chrome's grey. This scheme's panes are blue and
        // everything drawn in them — the yellow of a modified file, the cyan
        // of a directory — was chosen against blue: on the grey the row was
        // marked and the name on it was gone. Two steps over the ground and
        // one over the caret's line, so the three still read apart.
        selection_dim: Color::Indexed(20),
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
        // The same two darks the dark scheme uses: this scheme's ground is
        // blue, and a green or red mark on it is a mark either way.
        added_word: Color::Indexed(22),
        removed_word: Color::Indexed(52),
        // Dark, because this scheme's bars are a light grey and its ground
        // is a dark blue: the cyan and the yellow that shine on the blue were
        // 1.9:1 and 2.2:1 on the grey the status bar is cut from, which is the
        // one place a notification is ever drawn.
        info: Color::Indexed(19),
        // The darkest yellow the palette has, and still the weakest of the
        // three at 2.8:1 — a yellow dark enough to reach four has stopped
        // being yellow, and a warning that is not yellow is not a warning.
        warning: Color::Indexed(58),
        error: Color::Indexed(88),
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
                (t.menu_disabled, p.popup, "a greyed menu entry"),
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
                    p.selection_dim,
                    "a directory on an unfocused selection",
                ),
                (p.field, p.find_bar, "the find bar's field on the bar"),
                (t.chrome_dim, p.find_bar, "the hit count on the find bar"),
                (t.foreground, p.background, "a file name in the sidebar"),
                (t.line_number, p.background, "a line number"),
                (p.on_chrome, p.tab_strip, "the tab in front"),
                (p.on_chrome_dim, p.tab_strip, "a tab behind it"),
            ] {
                assert_ne!(text, ground, "{}: {what} is invisible", kind.label());
            }
        }
    }

    /// The xterm-256 colour at an index, as the eight-bit channels a terminal
    /// draws it with.
    ///
    /// Sixteen system colours, a 6×6×6 cube whose levels are *not* evenly
    /// spaced, and a twenty-four step grey ramp. Only the cube and the ramp
    /// are answered here: the first sixteen are whatever the user's terminal
    /// says they are, which is the whole point of the simple themes and the
    /// reason the contrast check below leaves them out.
    fn channels(colour: Color) -> Option<[f64; 3]> {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let index = match colour {
            Color::Indexed(index) if index >= 16 => index,
            _ => return None,
        };
        let rgb = if index < 232 {
            let i = index - 16;
            [
                LEVELS[(i / 36) as usize],
                LEVELS[((i / 6) % 6) as usize],
                LEVELS[(i % 6) as usize],
            ]
        } else {
            let grey = 8 + 10 * (index - 232);
            [grey, grey, grey]
        };
        Some(rgb.map(f64::from))
    }

    /// WCAG relative luminance.
    fn luminance(colour: Color) -> Option<f64> {
        let [r, g, b] = channels(colour)?;
        let linear = |c: f64| {
            let c = c / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        Some(0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b))
    }

    /// The WCAG contrast ratio between two colours, or `None` when either of
    /// them is one of the sixteen the terminal owns.
    fn contrast(text: Color, ground: Color) -> Option<f64> {
        let (a, b) = (luminance(text)?, luminance(ground)?);
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        Some((hi + 0.05) / (lo + 0.05))
    }

    /// A greyed entry has to stop looking like a live one.
    ///
    /// This is the bug the role exists for: a greyed entry was first drawn in
    /// `on_popup_dim`, which on the dark theme is 250 against a label's 252 —
    /// two steps of the greyscale, and both checks above called it correct.
    /// Half the contrast of a label is the gap at which the difference is
    /// seen rather than looked for.
    #[test]
    fn a_greyed_menu_entry_is_visibly_quieter_than_a_live_one() {
        for kind in [ThemeKind::Dark, ThemeKind::Light, ThemeKind::Retro] {
            let p = palette(kind);
            let t = Theme::new(kind);
            let live = contrast(p.on_popup, p.popup).expect("an indexed theme");
            let greyed = contrast(t.menu_disabled, p.popup).expect("an indexed theme");
            assert!(
                greyed * 2.0 <= live,
                "{}: a greyed entry is {:.1}:1 where a label is {:.1}:1",
                kind.label(),
                greyed,
                live
            );
        }
    }

    /// Secondary text has to stay *text*.
    ///
    /// `no_theme_draws_text_in_the_colour_underneath_it` only asks that the two
    /// colours differ, which a comment three shades off its ground passes while
    /// being unreadable — and that is exactly what the dark theme's comments,
    /// panel titles and status readout were. This asks for a ratio instead.
    ///
    /// 4:1 rather than WCAG's 4.5: a terminal cell is drawn at whatever size
    /// and weight the user's font has, the ratio is computed against the
    /// xterm-256 palette rather than against what the terminal actually
    /// renders, and the pairs below are all *secondary* text that must stay
    /// clearly behind the primary text beside it. The three indexed themes are
    /// checked; the two simple ones are the terminal's own sixteen colours and
    /// nothing here can say what they are.
    ///
    /// A selected row in a pane without focus, and a notification on the
    /// chrome, are held to 2.8:1 instead. Its
    /// ground is deliberately a shade of the pane's own — that is what makes it
    /// a *quiet* mark rather than a second focused selection — so every colour
    /// on it loses a little of the contrast it has on the ground beside it.
    /// Three is the floor at which the row is still read; the Retro theme's
    /// yellow on grey was 2.2, which is where this check came from.
    #[test]
    fn secondary_text_is_read_rather_than_merely_different() {
        const ON_GROUND: f64 = 4.0;
        const ON_A_QUIET_SELECTION: f64 = 2.8;
        const ON_A_GREYED_ROW: f64 = 2.5;
        for kind in [ThemeKind::Dark, ThemeKind::Light, ThemeKind::Retro] {
            let p = palette(kind);
            let t = Theme::new(kind);
            for (text, ground, floor, what) in [
                (t.dim, p.background, ON_GROUND, "a panel title"),
                (p.syntax.comment, p.background, ON_GROUND, "a comment"),
                (t.chrome_dim, p.chrome, ON_GROUND, "the status readout"),
                (t.chrome_dim, p.find_bar, ON_GROUND, "the hit count"),
                (t.search_label, p.find_bar, ON_GROUND, "a find bar label"),
                (t.menu_shortcut, p.popup, ON_GROUND, "a menu shortcut"),
                // Not `ON_GROUND`: a greyed entry is meant to recede, and one
                // held to four is one nobody can tell from a live entry. Two
                // and a half is the floor at which the label is still read.
                (t.menu_disabled, p.popup, ON_A_GREYED_ROW, "a greyed entry"),
                (
                    p.on_chrome_dim,
                    p.tab_strip,
                    ON_GROUND,
                    "a tab behind the one in front",
                ),
                (t.line_number, p.background, ON_GROUND, "a line number"),
                (t.directory, p.background, ON_GROUND, "a directory"),
                (t.git_modified, p.background, ON_GROUND, "a modified file"),
                (
                    t.git_untracked,
                    p.background,
                    ON_GROUND,
                    "an untracked file",
                ),
                (
                    t.directory,
                    p.selection_dim,
                    ON_A_QUIET_SELECTION,
                    "a directory, row selected",
                ),
                (
                    t.git_modified,
                    p.selection_dim,
                    ON_A_QUIET_SELECTION,
                    "a modified file, row selected",
                ),
                (
                    t.git_untracked,
                    p.selection_dim,
                    ON_A_QUIET_SELECTION,
                    "an untracked file, row selected",
                ),
                (
                    t.foreground,
                    p.selection_dim,
                    ON_A_QUIET_SELECTION,
                    "a file name, row selected",
                ),
                // The status bar's own sentence. Held to the lower floor for a
                // reason of its own: a notification says what it is in words
                // as well as in colour, and the Retro scheme's chrome is a
                // light grey that no yellow both reads on and stays yellow on.
                // The check is still worth having — the three were 1.9, 2.2
                // and 1.3 on that grey.
                (
                    t.info,
                    p.chrome,
                    ON_A_QUIET_SELECTION,
                    "an info notification",
                ),
                (t.warning, p.chrome, ON_A_QUIET_SELECTION, "a warning"),
                (t.error, p.chrome, ON_A_QUIET_SELECTION, "an error"),
            ] {
                let Some(ratio) = contrast(text, ground) else {
                    continue;
                };
                assert!(
                    ratio >= floor,
                    "{}: {what} is {ratio:.2}:1 ({text:?} on {ground:?})",
                    kind.label()
                );
            }
        }
    }

    /// The find bar is a bar, and the fields on it are fields.
    ///
    /// Two separate claims, and the dark theme was failing both at once: its
    /// bar was cut from the same grey as the status bar under it, and its
    /// fields were the editor's own ground — so the two rows read as two more
    /// rows of the file with the word `Find:` in front of them. The bar needs
    /// a tone that is neither the pane above nor the bar below, and the field
    /// needs one that is none of the three grounds it is ever drawn on.
    #[test]
    fn the_find_bar_and_its_fields_are_not_the_grounds_around_them() {
        for kind in ThemeKind::ALL.iter().copied() {
            let p = palette(kind);
            let label = kind.label();
            assert_ne!(
                p.find_bar, p.background,
                "{label}: the find bar is the editor's own ground"
            );
            for (ground, what) in [
                (p.popup, "a dialog"),
                (p.find_bar, "the find bar"),
                (p.chrome, "the bars"),
            ] {
                assert_ne!(p.field, ground, "{label}: a field is the colour of {what}");
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
