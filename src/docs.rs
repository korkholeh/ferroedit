//! `docs/SHORTCUTS.md`, generated from the tables it documents (ADR-028).
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

use std::fmt::Write as _;

use crate::app::focus::FocusTarget;
use crate::commands::{Command, MENUS};
use crate::event::keyboard::{binding_for, Binding, BINDINGS, INPUT_BINDINGS};

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
        "The read-only unified diff (SPEC §36), drawn over the editor pane. It \
         closes as soon as another pane takes focus, so its keys are a pager's \
         and nothing here types.",
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
        "While a menu is open. The items themselves are further down, under \
         *Menu bar*.",
    ),
    (
        Some(FocusTarget::Dialog),
        "Dialog",
        "A message and a row of buttons, or a list to choose from (SPEC §33). \
         `Left` and `Right` are the button row's axis and `Up` and `Down` are \
         the list's; a dialog with no list ignores the latter two.",
    ),
];

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
A dialog that asks for a name or a path. These replace the plain dialog's keys while \
the field is open, which is why `Left` moves a caret here and a button selection there \
(ADR-019).";

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
checkout. Clicking in the editor places the cursor, dragging selects, and a double-click selects \
the word under the pointer. The wheel scrolls the pane under the pointer without moving \
the cursor. Clicking a tab switches to it; clicking its `×`, or middle-clicking it, \
closes it. Clicking a row in the explorer opens the file or folds the directory in one \
click (ADR-020). Clicking a row of the find bar puts the caret in that row's field, and \
its `[Aa]`, `[Replace]` and `[All]` are clickable — the fallback for terminals that \
swallow `Alt`.
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

    for (focus, heading, note) in SECTIONS {
        let rows = rows_for(BINDINGS, *focus);
        if rows.is_empty() {
            continue;
        }
        let _ = write!(out, "\n## {heading}\n\n{note}\n\n");
        write_table(&mut out, &rows);
    }

    let input_rows = rows_for(INPUT_BINDINGS, Some(FocusTarget::Dialog));
    if !input_rows.is_empty() {
        let _ = write!(
            out,
            "\n## Dialog with a text field\n\n{INPUT_DIALOG_NOTE}\n\n"
        );
        write_table(&mut out, &input_rows);
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
fn rows_for(
    table: &'static [Binding],
    focus: Option<FocusTarget>,
) -> Vec<(Vec<&'static str>, String)> {
    let mut rows: Vec<(Vec<&'static str>, String)> = Vec::new();
    let mut commands: Vec<&'static Command> = Vec::new();
    for binding in table.iter().filter(|b| b.focus == focus) {
        match commands.iter().position(|c| *c == &binding.command) {
            Some(index) => {
                let labels = &mut rows[index].0;
                if !labels.contains(&binding.label) {
                    labels.push(binding.label);
                }
            }
            None => {
                commands.push(&binding.command);
                rows.push((vec![binding.label], binding.command.description()));
            }
        }
    }
    rows
}

fn write_table(out: &mut String, rows: &[(Vec<&str>, String)]) {
    out.push_str("| Keys | Action |\n|---|---|\n");
    for (labels, description) in rows {
        let keys: Vec<String> = labels.iter().map(|label| format!("`{label}`")).collect();
        let _ = writeln!(out, "| {} | {description} |", keys.join(" / "));
    }
}

/// The menu bar, with the key each entry advertises.
///
/// The shortcut column is `shortcut_for`, the same lookup the renderer does, so
/// this table cannot claim a key the menu does not show. An entry whose feature
/// has not landed says so rather than being left out — the gaps are the phase
/// plan, and hiding them would make the document look finished.
fn write_menus(out: &mut String) {
    out.push_str(
        "\n## Menu bar\n\nEvery item is a command, and the *Shortcut* column is the same \
         lookup the menu itself renders from, so an entry cannot advertise a key that is \
         not bound (ADR-008). *Where* is the pane that key works in: an item reachable \
         from any menu may have a key that is not — `Alt+C` needs the find bar, and `F2` \
         the explorer.\n\n| Menu | Item | Shortcut | Where |\n|---|---|---|---|\n",
    );
    for menu in MENUS {
        for item in menu.items {
            let binding = binding_for(&item.command);
            let shortcut = binding.map_or_else(|| "—".to_string(), |b| format!("`{}`", b.label));
            let scope = match binding {
                None => "—",
                Some(b) => b.focus.map_or("anywhere", section_name),
            };
            let pending = match item.command {
                Command::Unimplemented(_) => " *(not implemented yet)*",
                _ => "",
            };
            let _ = writeln!(
                out,
                "| {} | {}{pending} | {shortcut} | {scope} |",
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

    /// Phase 9's acceptance: every menu entry resolves to a real command except
    /// the ones whose feature is a later phase, and those are named here so
    /// that a new gap cannot be added quietly.
    #[test]
    fn the_only_unimplemented_menu_entries_are_the_ones_still_owed() {
        let pending: Vec<&str> = MENUS
            .iter()
            .flat_map(|menu| menu.items)
            .filter(|item| matches!(item.command, Command::Unimplemented(_)))
            .map(|item| item.label)
            .collect();
        assert_eq!(
            pending,
            vec!["Shortcuts"],
            "the Git entries landed in Phase 11; the help screen is Phase 14"
        );
    }

    #[test]
    fn keys_bound_to_one_command_share_a_row() {
        let rows = rows_for(BINDINGS, None);
        let (keys, description) = rows
            .iter()
            .find(|(_, description)| description == "Next tab")
            .expect("next tab is a global binding");
        assert_eq!(keys, &vec!["Ctrl+Tab", "Ctrl+PageDown"]);
        assert_eq!(description, "Next tab");
    }
}
