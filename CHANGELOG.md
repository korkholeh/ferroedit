# Changelog

Every released version of FerroEdit, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions
follow [semantic versioning](https://semver.org/spec/v2.0.0.html).

Unreleased work is collected under **Unreleased**; `scripts/release.sh` cuts a
version, so that heading becomes the new version's on the way out (see
[README](README.md#cutting-a-release)).

## [Unreleased]

### Changed

- **The README's screenshots are captures of 0.1.10.** Every image was retaken against
  the current chrome — the shorter View menu with its theme picker, the tab strip on the
  editor's own ground, the panes that keep their width — so the pictures no longer show
  a menu and a palette that were renamed two releases ago.

## [0.1.10] — 2026-09-11

### Added

- **A folder that is not a Git repository gets a way to become one.** The Git panel draws
  a `[ git init ]` button under the `Not a Git repository` line — pressed with the mouse,
  or with `Enter` while the panel has focus — and the Git menu carries the same thing as
  `Initialize Repository` (ADR-084).

### Changed

- **The Git panel says when its repository is above the workspace.** Opening a
  subdirectory of a repository — or a folder that merely happens to sit inside one —
  gave a panel listing paths the explorer beside it had never heard of, with nothing
  saying why. A dim row under the title now names the repository (` in Projects/`), and
  the panel costs no row at all in the ordinary case where the workspace *is* the
  repository (ADR-085).
- **The Git menu greys out what the current folder has nothing to run.** Outside a
  repository every entry but `Refresh` and `Initialize Repository` is drawn dim: it keeps
  its row, its label and its key, the selection steps over it, and a click on it does
  nothing. `Initialize Repository` greys out the other way round, once there is a
  repository (ADR-084). A greyed row has a colour of its own — the shortcut column's dim
  is held to a reading contrast and was two steps off a live label, which is no
  difference at all.

## [0.1.9] — 2026-09-11

### Added

- **A diff says which part of a replaced line changed.** Where a run of removed lines is
  followed by a run of added lines of the same length, the two are paired off and the part
  each one does *not* share with its partner is marked with a background — deep green on
  the addition, deep red on the removal. The `+`/`−` colours and prefixes are unchanged, an
  unpaired line keeps the whole-line colouring it had, and very long lines and very large
  blocks fall back to it as well (ADR-082).
- **The whole message of a commit**, with `m` in the log viewer. The subject column is cut
  to whatever the pane had room for; this is where the rest of it and the body are read.
  `Enter` still opens the diff. `c` hides the hash, date and author outright and gives the
  subject the pane (ADR-080).
- **Numbers in a table line up on their last digit.** A CSV column whose every non-blank
  value is a plain number is right-aligned. Versions, dates, times, grouped numbers,
  percentages and identifiers written with a leading zero are all left alone — the check is
  deliberately narrow, because right-aligning an identifier lines up the wrong end of it
  (ADR-081). The name of the column the cursor is in is lifted out of the header's dim.
- **The image viewer says what its keys are**, on its bottom border, the way the log viewer
  has since ADR-071.

### Changed

- **Secondary text is readable.** Comments, panel titles, line numbers, menu shortcuts and
  the status readout were between 2.8:1 and 3.3:1 against what they were drawn on; they are
  now held to 4:1 by a test that computes the ratio rather than merely checking that two
  colours differ (ADR-083). In the Retro theme a selected file in an unfocused git panel
  was a yellow letter on a light grey at 2.2:1, and its notification colours were as low as
  1.3:1.
- **The find and replace bar reads as a bar.** It has a ground of its own — neither the
  editor's above it nor the status bar's below — the two fields are wells on it rather than
  more rows of the file, and the row being typed into is marked by its label sitting on the
  highlight bar. `[Aa]` says which way it is pointing with a mark inside the brackets as
  well as with a colour, `[All]` is now `[Replace all]`, and the two buttons have room
  between them.
- **A selected row in the sidebar is marked to the pane's edge**, in both panels, instead of
  stopping where the name does.
- **The View menu is eighteen rows instead of twenty-seven**, and so fits a 24-row
  terminal, where the bottom of it used to be cut off and unreachable. The five theme
  entries are `Theme…`, the three focus entries are `Focus Pane…`, and the delimiter and
  the quote character are behind `Table Format…`, which shows what they currently are.
  Every command they stood for is still there (ADR-079).
- **The log viewer gives its width to the subjects.** The author column shrinks first, then
  the date is dropped, then the author, and the abbreviated name last — the subject keeps
  thirty cells for as long as anything can be given up for it (ADR-080).
- **A picture opens on the picture.** The metadata column starts closed and `m` opens it;
  the compact set — the name, the format, the size and the scale — is the frame's own
  title, which it always was. The column is now three groups under headings, the status bar
  stops repeating the format and the dimensions, and the readouts say `whole image` where
  they used to report a corner the window was not at and a region larger than the file
  (ADR-080).

### Fixed

- The first key pressed in a diff, a history or a picture opened from the command line was
  swallowed: focus followed the tab in front only after a command had run, and opening a
  file from the command line is not one. `m` on `ferroedit photo.png` is what found it
  (ADR-080).

## [0.1.8] — 2026-09-10

### Added

- **PNG and JPEG files open on the picture.** A `.png`, `.jpg` or `.jpeg` opens in an image
  viewer — a read-only tab beside the diffs and the histories — instead of being refused as
  a binary file. The picture is drawn in coloured half-block characters, so a terminal cell
  carries two pixels and the picture is not stretched to double height: truecolor where the
  terminal advertises it, and the nearest xterm-256 colour everywhere else (ADR-078).
- A **metadata column** down the left of the pane: the file's name and size, the format,
  the dimensions in pixels and megapixels, the aspect ratio, the colour layout, whether
  there is an alpha channel, how many bytes on disk each pixel cost, and where the window
  currently sits. `m` shows and hides it, and a pane too narrow to spare the room drops it
  rather than the picture.
- **Zoom and pan.** `+` and `-` step through fixed levels from 1/16 to 16×, `0` fits the
  whole picture in the pane and keeps it fitted across a resize, `1` is actual size, and
  the zoom holds the middle of the pane still. The arrows pan by an eighth of what is on
  screen — the same distance at every scale — `PageUp`/`PageDown` by a whole pane and
  `Home` recentres. With a mouse: the wheel pans, `Shift`+wheel pans sideways,
  `Ctrl`+wheel zooms, and dragging moves the picture under the pointer. `F5` re-reads the
  file and keeps the zoom and the position.
- Shrinking averages the pixels each half-cell covers and magnifying takes the nearest one,
  so a photograph reduced forty-fold still looks like the photograph and an icon at 800%
  shows square pixels. Transparency is composited onto a checkerboard.
- The format is decided by the file's first bytes, not its name, so a `.png` that is really
  a JPEG opens as the JPEG it is. A large image is read a slice at a time with the same
  progress box a large text file gets. Anything over 64 megapixels is refused with a
  sentence saying so, before anything is allocated on the strength of the header.

## [0.1.7] — 2026-09-10

### Added

- **A large file now says it is opening.** A file of 4 MB or more is read a slice at a
  time — 8 ms of reading per frame — and a box in the middle of the editor pane carries a
  spinner, the file's name and how much has arrived so far (`⠸ Opening big.txt — 43% of
  188 MB`) over a progress bar. The window keeps drawing throughout instead of going still
  until the file is in a tab, which on a cold cache or a network share is where the whole
  wait was. Smaller files open in one go with no box to flicker, and an idle editor is
  still never woken to redraw (ADR-077).
- **Gzipped files open on their text.** A file whose bytes say gzip — `dump.sql.gz`, a
  rotated `.log.gz` — is unpacked as it is read, so looking inside one no longer means
  unpacking it to a temporary file first. The grammar comes from the name inside the
  container, so a `.sql.gz` is highlighted as SQL, and the charset is sniffed from what
  came out rather than assumed (ADR-074).
- The buffer is **read-only**: the editor unpacks and never packs, so typing, Cut, Paste,
  Replace and `Ctrl+S` are refused and say so. Everything that reads still works —
  motion, selection, Copy, Find, wrapping, `F5`, Reopen with Encoding. *Save As* is the
  way out: it writes the text as an ordinary file, and the tab becomes one.
- Unpacking stops at 64 MB. A longer stream opens on its beginning, cut back to a whole
  line, and says so both when the tab opens and on the status bar for as long as it is
  open; *Save As* is refused while a buffer is cut.

### Changed

- **The find bar waits for `Enter` on files over 5 000 lines.** Finding every hit walks
  the whole file, which on a 200 000-line log is ~73 ms per keystroke — a field that stops
  taking input. Above the threshold the query is run when you ask for it: the count reads
  `Enter` until then, nothing is highlighted, and a line when the bar opens says why.
  `Enter`, Find Next/Previous and both Replaces work as they always did, whatever the
  file's size. Smaller files are unchanged (ADR-075).
- **A search long enough to see now shows a spinner.** The walk is sliced across frames —
  8 ms of work each, then a redraw — so the editor keeps taking input while it runs and
  `Esc` still closes the bar. The count's cells show a spinner and the hits found so far
  (`⠹ 1274`) until it lands, then go back to `3/17`. Measured over a 2 000 000-line log
  the search drew 106 frames and the window never stopped repainting. A file small enough
  to search inside one frame shows no spinner at all — a one-frame flicker is worse than
  nothing — and an idle editor is still never woken to redraw (ADR-076).

## [0.1.6] — 2026-09-10

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
  scheme, where the two rows were the same flat grey. The tabs behind sit on that strip,
  dim and divided from each other by a `│` rule; the tab in front carries the editor's own
  ground, so the file being edited runs into the pane below it. The file tree draws a rule
  under the menu bar as well, the way the git panel below it always has, with its title on
  the line (ADR-072).

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

[Unreleased]: https://github.com/korkholeh/ferroedit/compare/v0.1.10...HEAD
[0.1.10]: https://github.com/korkholeh/ferroedit/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/korkholeh/ferroedit/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/korkholeh/ferroedit/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/korkholeh/ferroedit/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/korkholeh/ferroedit/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/korkholeh/ferroedit/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/korkholeh/ferroedit/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/korkholeh/ferroedit/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/korkholeh/ferroedit/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/korkholeh/ferroedit/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/korkholeh/ferroedit/releases/tag/v0.1.0
