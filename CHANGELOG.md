# Changelog

Every released version of FerroEdit, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [semantic versioning](https://semver.org/spec/v2.0.0.html).

Unreleased work is collected under **Unreleased**; `scripts/release.sh` cuts a
version, so that heading becomes the new version's on the way out (see
[README](README.md#cutting-a-release)).

## [Unreleased]

### Added

- **A log viewer**, in a tab of its own beside the diffs. *Git ▸ Log* (`l` in the git
  panel) shows the repository's history — the abbreviated name, the date, the author and
  the subject, in columns. `Enter` or `d` opens the commit under the selection as a diff,
  message and all; `F5` re-reads it (ADR-068, ADR-069).
- **Text search over the history.** `/` opens a field in the viewer. Typing narrows the
  commits already read, which is instant; `Enter` hands the text to `git log --grep`, which
  searches the whole commit message and the whole history rather than only what was loaded.
  The title says which of the two is on screen. A click on a row opens its commit, the way
  `Enter` does.
- **The history of the open file.** *Git ▸ File History* (`h` in the git panel) follows the
  file across renames, and a commit opened from it is narrowed to that file.
- **The history of the selected lines.** *Git ▸ Line History* runs `git log -L` over the
  lines the editor's selection covers — the caret's own line when nothing is selected.
- **The repository's git config opens as a file.** *Git ▸ Config* (`g` in the git panel)
  opens `.git/config` in an ordinary editor tab — read, edited and saved by the same code
  as every other file, comments and all (ADR-070).

### Changed

- **The menu bar no longer advertises bare letters.** `a`, `d`, `l` and the rest work in
  the git panel and the log viewer exactly as before, but a menu is read with the caret in
  a document, where pressing one of them types it. The log viewer instead carries a legend
  on its bottom border — `↑↓ move · Enter diff · / search · F5 refresh · Esc close` — read
  out of the keymap, so it cannot drift. `F1` and `docs/SHORTCUTS.md` still list every key
  under the pane it needs (ADR-071).
- **The chrome at the top of the window no longer reads as one slab.** The tab strip is
  drawn in a tone of its own — a shade under the grey the menu bar is cut from, and not the
  editor's ground either — so both of its edges read; it was most visible on the Retro
  scheme, where the two rows were the same flat grey. Every tab sits on that strip and is
  told apart by its text: the file in front is bold and full contrast, the ones behind are
  dim, and a `│` rule divides one tab from the next. The file tree draws a rule under the
  menu bar as well, the way the git panel below it always has, with its title on the line
  (ADR-072).

### Fixed

- **Being killed no longer leaves the terminal broken — nor does the run after it.** An
  editor ended by a signal rather than by `Ctrl+Q` (`kill`, a supervisor, a closed
  terminal window) ran neither its teardown nor its panic hook, so it left raw mode
  behind: no echo, no line editing, and every line of output starting where the last one
  ended instead of at the left margin. Worse, it stuck — the next run saved that broken
  mode as the one to restore and put it back on a perfectly normal quit, which is why the
  damage looked intermittent and unrelated to whatever had actually caused it. FerroEdit
  now restores the terminal on `SIGINT`, `SIGTERM`, `SIGHUP` and `SIGQUIT` before standing
  down, and mends a terminal it finds already in raw mode when it starts — so opening and
  quitting the editor now repairs a terminal an earlier run broke (ADR-073).
- A diff or a history asked for from the editor now finds the file when the workspace was
  opened through a symbolic link — macOS puts its temporary directories behind one, and
  `git rev-parse --show-toplevel` answers with the resolved path.

## [0.1.5] — 2026-09-09

### Added

- **The line the caret is on is marked on the editor's ground**, the way the CSV
  grid already marks the record the cursor is in — the whole pane wide, and every
  row of a wrapped line. A selection and a search hit are drawn over it, so
  neither is hidden. The two simplified ANSI themes have no tone for it and keep
  the bold line number alone (ADR-067).

### Changed

- **Menu entries that carry a state now show it.** A `✓` marks *View ▸ Word Wrap*,
  *Hidden and Ignored Files*, *Table View*, *Search ▸ Match Case* and
  *Help ▸ Check on Start* while they are on, and the theme in use among the five
  in the View menu. The mark is a column of its own, so the labels of a menu
  stay on one left edge and nothing moves when a state flips (ADR-066).

## [0.1.4] — 2026-09-09

### Added

- **A check for new releases.** At start-up — at most once a day — FerroEdit asks
  GitHub whether there is a release newer than the running build, and names it on
  the status bar. *Help ▸ Check for Updates* asks now and answers either way, in
  a dialog with the releases address in it; *Help ▸ Check on Start* turns the
  automatic check off, and the choice is kept in
  `~/.config/ferroedit/config.json` as `check-for-updates`.
- The request is made by `curl` or `wget` — the ones `install.sh` already needs —
  so the binary carries no HTTP client and the static musl builds are unchanged.
  Nothing beyond an ordinary HTTP request leaves the machine, and the editor
  never installs anything itself.

## [0.1.3] — 2026-09-09

### Added

- **The CSV table view.** A `.csv` or `.tsv` tab opens as a grid — the first row
  as the header, one row per record — with the delimiter and the quote character
  as clickable readouts on the status bar. `F4` swaps the pane between the table
  and the text, and the grid is reparsed whenever the buffer or the dialect
  changes.
- **Editing in the table.** `F2` or `Enter` opens a cell, a printable character
  replaces it, `Enter` and `Tab` save and move on, `Esc` gives up. `Insert` adds
  a record and `Ctrl+D` removes one; the header is a row the selection reaches,
  so a column is renamed in the grid. A write touches only the field's own
  characters, quoting where the dialect needs it, as one ordinary undo step.
- **Selecting cells in the table.** `Shift` with a motion key, a drag, a click on
  a record's number or a column's name, `Ctrl+A`, and the Selection menu's
  Select Row / Select Column all make a rectangle of cells, counted on the status
  bar as `Sel 3×2`. Copy writes the block in the file's own dialect, cut empties
  it as one undo step, `Delete` empties it in place.
- The gutter marks the line the caret is on: its number is drawn bold in the
  foreground colour while every other number stays dim.

## [0.1.2] — 2026-09-08

### Added

- A one-line `curl | sh` installer.

### Changed

- The explorer shows ignored and hidden files by default.

## [0.1.1] — 2026-09-08

### Added

- Open is a file browser, and opening a folder moves the workspace to it.
- Word wrap, horizontal scrolling, and Go to Line (`Ctrl+G`).
- Diff tabs, selectable themes, tab scrolling, and rules in the menus.
- Scrollbars in the explorer, the git panel, and the editor.
- Clickable status-bar readouts, and the legacy encodings behind them.
- The macOS binaries are signed and notarized.

## [0.1.0] — 2026-09-07

The first release: the editor through Phase 14.

### Added

- Text editing with undo, a file explorer, tabs, find and replace, syntax
  highlighting, and a configurable theme.
- The git status panel, git actions, branches and merge, and the diff viewer.
- A filesystem watcher, a byte budget for the undo stack, cancellable git jobs,
  the help screen, and a quit that asks about each unsaved file in turn.
- Releases are GitHub Releases, built for four targets.

[Unreleased]: https://github.com/korkholeh/ferroedit/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/korkholeh/ferroedit/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/korkholeh/ferroedit/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/korkholeh/ferroedit/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/korkholeh/ferroedit/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/korkholeh/ferroedit/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/korkholeh/ferroedit/releases/tag/v0.1.0
