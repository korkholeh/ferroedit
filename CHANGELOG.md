# Changelog

Every released version of FerroEdit, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [semantic versioning](https://semver.org/spec/v2.0.0.html).

Unreleased work is collected under **Unreleased**; `scripts/release.sh` cuts a
version, so that heading becomes the new version's on the way out (see
[README](README.md#cutting-a-release)).

## [Unreleased]

Nothing yet.

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

[Unreleased]: https://github.com/korkholeh/ferroedit/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/korkholeh/ferroedit/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/korkholeh/ferroedit/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/korkholeh/ferroedit/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/korkholeh/ferroedit/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/korkholeh/ferroedit/releases/tag/v0.1.0
