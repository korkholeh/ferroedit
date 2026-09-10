//! `docs/SHORTCUTS.md`, and the help screen, generated from the tables they
//! document (ADR-028, ADR-038).
//!
//! Phase 9's anti-drift rule: the keymap and the menu bar are data, so the
//! document describing them is rendered from that data rather than written
//! beside it. A binding added without a line here is impossible; a line here
//! for a binding that was removed is impossible too.
//!
//! Two ways out of the process: `ferroedit --dump-shortcuts` writes the file to
//! stdout, and the test at the bottom fails when the checked-in copy differs —
//! rewriting it when `FERROEDIT_UPDATE_DOCS` is set. Everything the tables
//! cannot know (typing, the mouse, terminal limits) is prose held in this file,
//! so the generated document is the whole document and not a fragment somebody
//! has to splice.
//!
//! Phase 14 added a second reader: `app::help` renders the same sections into
//! the help screen. Both go through `sections()`, so a binding shown on screen
//! and a binding written to the file are the same row of the same table.

use std::fmt::Write as _;

use crate::app::focus::FocusTarget;
use crate::commands::{Command, MENUS};
use crate::event::keyboard::{menu_binding, Binding, BINDINGS, INPUT_BINDINGS};

/// The sections, in the order they are rendered: which focus a table is for,
/// its heading, and the sentence under it.
///
/// `None` is the global table — the bindings that resolve whatever has focus.
const SECTIONS: &[(Option<FocusTarget>, &str, &str)] = &[
    (
        None,
        "Anywhere",
        "These resolve whatever has focus, except behind a dialog: a modal window \
         binds its own keys and nothing else (SPEC §40).",
    ),
    (
        Some(FocusTarget::Editor),
        "Editor",
        "The document pane. Every motion here also has a `Shift` form that extends \
         the selection instead of moving the cursor.",
    ),
    (
        Some(FocusTarget::Explorer),
        "Explorer",
        "The file tree (SPEC §18, §20). The operations act on the selected row, not \
         on the file being edited.",
    ),
    (
        Some(FocusTarget::GitPanel),
        "Git panel",
        "The changed-files list (SPEC §31, §33, §35). Everything that writes \
         to the repository runs on a worker thread, so none of these keys \
         blocks a frame (ADR-033).",
    ),
    (
        Some(FocusTarget::Diff),
        "Diff viewer",
        "The read-only unified diff (SPEC §36). It opens in a tab of its own, \
         beside the files being edited, and closes like one — so its keys are a \
         pager's and nothing here types.",
    ),
    (
        Some(FocusTarget::Log),
        "Log viewer",
        "A commit history — the repository's, one file's, or one range of lines' \
         (ADR-068). It opens in a tab of its own beside the diffs, and it is a \
         *list*: the arrows move a selection, and `Enter` opens the commit it is \
         on as a diff.",
    ),
    (
        Some(FocusTarget::Image),
        "Image viewer",
        "A PNG or a JPEG, drawn in coloured half-block characters — two rows of \
         pixels to a terminal cell (ADR-078). It opens in a tab of its own \
         beside the files being edited, and it is a *window*: the arrows move \
         the window over the picture and the zoom keys change how much of it \
         fits.",
    ),
    (
        Some(FocusTarget::LogSearch),
        "Log search field",
        "The field `/` opens over a history. Typing narrows the commits already \
         read, which is instant; `Enter` hands the text to `git log --grep`, which \
         searches the whole message and the whole history.",
    ),
    (
        Some(FocusTarget::Help),
        "Help screen",
        "This screen (SPEC §6). A pager over the same tables, so its keys are the \
         diff viewer's — two read-only panes that scrolled differently would be \
         two things to remember instead of one.",
    ),
    (
        Some(FocusTarget::Search),
        "Find bar",
        "Open from `Ctrl+F` or `Ctrl+H`, and *not* modal: `Ctrl+S` still saves and \
         the menu still opens while the caret is in it.",
    ),
    (
        Some(FocusTarget::Menu),
        "Menu",
        "While a menu is open. The items themselves are the menu bar's own, \
         along the top of the screen.",
    ),
    (
        Some(FocusTarget::Dialog),
        "Dialog",
        "A message and a row of buttons, or a list to choose from (SPEC §33). \
         `Left` and `Right` are the button row's axis and `Up` and `Down` are \
         the list's; a dialog with no list ignores the latter two.",
    ),
];

/// One row of a key table: every key bound to a command, and what it does.
///
/// The keys are the labels the keymap carries, so a row can never advertise a
/// key that is not bound — that is the whole point of generating both readers
/// from the table instead of writing either by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpRow {
    pub keys: Vec<&'static str>,
    pub action: String,
}

impl HelpRow {
    /// ``Ctrl+Tab / Ctrl+PageDown`` — the key column, in both readers.
    pub fn key_label(&self) -> String {
        self.keys.join(" / ")
    }
}

/// One key table: the heading it is drawn under, the sentence explaining when
/// its keys apply, and its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub heading: &'static str,
    pub note: &'static str,
    pub rows: Vec<HelpRow>,
}

/// Every key table, in reading order, with the empty ones dropped.
///
/// The one shared source for `docs/SHORTCUTS.md` and the help screen. A section
/// with no bindings is left out rather than drawn as a heading over nothing.
pub fn sections() -> Vec<HelpSection> {
    let mut sections: Vec<HelpSection> = SECTIONS
        .iter()
        .map(|(focus, heading, note)| HelpSection {
            heading,
            note,
            rows: rows_for(BINDINGS, *focus),
        })
        .collect();
    sections.push(HelpSection {
        heading: "Dialog with a text field",
        note: INPUT_DIALOG_NOTE,
        rows: rows_for(INPUT_BINDINGS, Some(FocusTarget::Dialog)),
    });
    sections.retain(|section| !section.rows.is_empty());
    sections
}

const HEADER: &str = "\
# Keyboard shortcuts

<!-- GENERATED FILE. Do not edit by hand.
     Rendered from `src/event/keyboard.rs` and `src/commands/mod.rs` by
     `src/docs.rs`; run `ferroedit --dump-shortcuts > docs/SHORTCUTS.md`, or
     `FERROEDIT_UPDATE_DOCS=1 cargo test`, after changing a binding (ADR-028). -->

A binding is scoped to the pane that has focus, so every table below says where its
keys work. A key with no entry for the focused pane falls through to the *Anywhere*
table; a key with no entry there does nothing.
";

const INPUT_DIALOG_NOTE: &str = "\
A dialog that asks for a name, and the two whose field filters a list under it: the Open \
browser (ADR-051) and the syntax and encoding pickers (ADR-058, ADR-059). These replace the plain dialog's keys while the field is open, which is why \
`Left` moves a caret here and a button selection there (ADR-019); `Up` and `Down` are \
still the list's, since the caret has no use for them.";

const TYPING: &str = "\
## Typing

Printable characters are not in the tables — there is one row per binding and a million \
printable characters — but they resolve to commands like everything else (SPEC §25):

| Where | Action |
|---|---|
| Editor | Insert the character at the cursor |
| Find bar | Type into the field with the caret |
| Dialog with a text field | Type into the field |

A `Ctrl` or `Alt` modifier that is not bound types nothing: an unbound combination is a \
command that does not exist, not a letter.
";

const MOUSE: &str = "\
## Mouse

Clicking a row of a dialog's list selects it without choosing it — unlike the explorer's \
rows (ADR-020), the confirm button is right there and choosing a branch by accident is a \
checkout. The Open browser is the exception: a click on the row that is *already* \
selected walks into it, so browsing costs one click a step, and the wheel over its rows \
moves the selection through them (ADR-051). Clicking in the editor places the cursor, dragging selects, and a double-click selects \
the word under the pointer. Over a table (SPEC §65) a click selects the cell instead, dragging selects a block of \
cells, a click on a column's name takes the whole column and one on a record's number \
takes the whole record; clicking away from a cell being typed into saves it, and the \
wheel scrolls the grid while `Shift`+wheel moves it sideways. Over a picture (ADR-078) \
the wheel pans it, `Shift`+wheel pans it sideways, `Ctrl`+wheel zooms about the middle of \
the pane, and dragging moves the picture under the pointer. The wheel scrolls the pane \
under the pointer without moving the cursor. Clicking a tab switches to it; clicking its `×`, or middle-clicking it, \
closes it. When there are more tabs than fit, the wheel over the tab strip scrolls it \
sideways and its `‹` and `›` arrows step it one tab at a time. Clicking a row in the explorer opens the file or folds the directory in one \
click (ADR-020). Clicking a row of the find bar puts the caret in that row's field, and \
its `[Aa]`, `[Replace]` and `[All]` are clickable — the fallback for terminals that \
swallow `Alt`. On the status bar, most of the right-hand readout is questions \
(ADR-058): `Ln 12, Col 8` opens Go to Line, the charset name the encoding picker \
(ADR-059), `LF` / `CRLF` the line-ending picker, the grammar's name the syntax picker, \
the branch name the branch picker, and — while a table is showing — `Delim` and `Quote` \
the two pickers that say how the file is split into columns. Each is a menu entry too, so none of them needs a \
mouse. What is left — the notification, the focus label, the selection count — reports \
where you already are and stays inert.
";

const LIMITS: &str = "\
## Terminal limits

- `Ctrl+Z` is undo, not suspend: raw mode disables `ISIG`, so it arrives as a key event.
  There is no suspend-to-shell in the MVP.
- `Ctrl+Q` needs `IXON` off, which raw mode already handles.
- `Ctrl+Shift+Z` and `Ctrl+Shift+S` are deliberately unbound: a terminal without the
  kitty keyboard protocol cannot tell either from its unshifted form, and a key that
  redoes in one terminal and undoes in another is worse than one that always works
  (ADR-008). Redo is `Ctrl+Y`; Save As is a menu entry.
- `Ctrl+Tab` and `Ctrl+Shift+Tab` are not deliverable by every terminal, so
  `Ctrl+PageDown` and `Ctrl+PageUp` are bound to the same commands rather than as
  second-class fallbacks.
- `Alt` is not delivered at all by macOS Terminal.app without \"Use Option as Meta\";
  every `Alt` binding is also a menu item and a clickable label.
- `Cmd+…` is out of MVP scope (SPEC §2.2).
";

/// The whole of `docs/SHORTCUTS.md`.
pub fn shortcuts_markdown() -> String {
    let mut out = String::from(HEADER);

    for section in sections() {
        let _ = write!(out, "\n## {}\n\n{}\n\n", section.heading, section.note);
        write_table(&mut out, &section.rows);
    }

    let _ = write!(out, "\n{TYPING}");
    write_menus(&mut out);
    let _ = write!(out, "\n{MOUSE}");
    let _ = write!(out, "\n{LIMITS}");
    out
}

/// One row per command, with every key bound to it.
///
/// Grouping by command rather than by key is what turns the two entries for
/// "next tab" into one row reading ``Ctrl+Tab` / `Ctrl+PageDown``, which is how
/// a reader wants to see them and how the menu already shows them.
fn rows_for(table: &'static [Binding], focus: Option<FocusTarget>) -> Vec<HelpRow> {
    let mut rows: Vec<HelpRow> = Vec::new();
    let mut commands: Vec<&'static Command> = Vec::new();
    for binding in table.iter().filter(|b| b.focus == focus) {
        match commands.iter().position(|c| *c == &binding.command) {
            Some(index) => {
                let keys = &mut rows[index].keys;
                if !keys.contains(&binding.label) {
                    keys.push(binding.label);
                }
            }
            None => {
                commands.push(&binding.command);
                rows.push(HelpRow {
                    keys: vec![binding.label],
                    action: binding.command.description(),
                });
            }
        }
    }
    rows
}

fn write_table(out: &mut String, rows: &[HelpRow]) {
    out.push_str("| Keys | Action |\n|---|---|\n");
    for row in rows {
        let keys: Vec<String> = row.keys.iter().map(|key| format!("`{key}`")).collect();
        let _ = writeln!(out, "| {} | {} |", keys.join(" / "), row.action);
    }
}

/// The menu bar, with the key each entry advertises.
///
/// The shortcut column is `menu_binding`, the same lookup the renderer does, so
/// this table cannot claim a key the menu does not show — including the bare
/// letters the menu deliberately keeps quiet about (ADR-071), which the
/// per-pane tables above still list under the focus they need. Every entry
/// resolves to a real command since Phase 14; there is no longer a placeholder
/// to mark.
fn write_menus(out: &mut String) {
    out.push_str(
        "\n## Menu bar\n\nEvery item is a command, and the *Shortcut* column is the same \
         lookup the menu itself renders from, so an entry cannot advertise a key that is \
         not bound (ADR-008). *Where* is the pane that key works in: an item reachable \
         from any menu may have a key that is not — `Alt+C` needs the find bar, and `F2` \
         the explorer.\n\n| Menu | Item | Shortcut | Where |\n|---|---|---|---|\n",
    );
    for menu in MENUS {
        for item in menu.entries() {
            let binding = menu_binding(&item.command);
            let shortcut = binding.map_or_else(|| "—".to_string(), |b| format!("`{}`", b.label));
            let scope = match binding {
                None => "—",
                Some(b) => b.focus.map_or("anywhere", section_name),
            };
            let _ = writeln!(
                out,
                "| {} | {} | {shortcut} | {scope} |",
                menu.title, item.label
            );
        }
    }
}

/// The heading a focus's bindings are documented under, for cross-references.
fn section_name(focus: FocusTarget) -> &'static str {
    SECTIONS
        .iter()
        .find(|(section, _, _)| *section == Some(focus))
        .map_or("—", |(_, heading, _)| *heading)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn checked_in() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/SHORTCUTS.md")
    }

    /// The one test that makes the generator worth having: the file in the
    /// repository is the file the tables produce.
    ///
    /// `FERROEDIT_UPDATE_DOCS=1 cargo test` rewrites it instead of failing,
    /// which is the whole regeneration workflow.
    #[test]
    fn the_checked_in_document_is_the_generated_one() {
        let generated = shortcuts_markdown();
        let path = checked_in();
        if std::env::var_os("FERROEDIT_UPDATE_DOCS").is_some() {
            std::fs::write(&path, &generated).expect("docs/SHORTCUTS.md is writable");
            return;
        }
        let checked_in = std::fs::read_to_string(&path).expect("docs/SHORTCUTS.md exists");
        assert_eq!(
            checked_in, generated,
            "docs/SHORTCUTS.md is stale — run `FERROEDIT_UPDATE_DOCS=1 cargo test` \
             (or `ferroedit --dump-shortcuts > docs/SHORTCUTS.md`)"
        );
    }

    #[test]
    fn every_binding_appears_in_the_document() {
        let document = shortcuts_markdown();
        for binding in BINDINGS.iter().chain(INPUT_BINDINGS) {
            assert!(
                document.contains(&format!("`{}`", binding.label)),
                "{} is bound but not documented",
                binding.label
            );
        }
    }

    #[test]
    fn keys_bound_to_one_command_share_a_row() {
        let rows = rows_for(BINDINGS, None);
        let row = rows
            .iter()
            .find(|row| row.action == "Next tab")
            .expect("next tab is a global binding");
        assert_eq!(row.keys, vec!["Ctrl+Tab", "Ctrl+PageDown"]);
        assert_eq!(row.key_label(), "Ctrl+Tab / Ctrl+PageDown");
    }

    /// The help screen and the file read the same tables, so a section that is
    /// in one is in the other.
    #[test]
    fn every_section_with_bindings_is_offered_to_both_readers() {
        let document = shortcuts_markdown();
        let sections = sections();
        assert!(
            sections
                .iter()
                .any(|s| s.heading == "Dialog with a text field"),
            "the input dialog's table is one of the sections"
        );
        for section in sections {
            assert!(
                document.contains(&format!("## {}", section.heading)),
                "{} is a section with rows but no heading in the document",
                section.heading
            );
            assert!(!section.rows.is_empty(), "empty sections are dropped");
        }
    }
}
