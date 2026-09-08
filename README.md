<p align="center">
  <img src="assets/hero.png" alt="FerroEdit — a desktop-like text editor that runs in a terminal" width="900">
</p>

<h1 align="center">FerroEdit</h1>

<p align="center">
  <em>A modern desktop-like text editor that happens to run in a terminal.</em>
</p>

<p align="center">
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-5fafff?style=flat-square">
  <img alt="Rust 1.75+" src="https://img.shields.io/badge/rust-1.75%2B-ffaf5f?style=flat-square">
  <img alt="Status: pre-alpha" src="https://img.shields.io/badge/status-pre--alpha-d787d7?style=flat-square">
  <img alt="Platforms" src="https://img.shields.io/badge/macOS%20%C2%B7%20Linux-87d787?style=flat-square">
</p>

FerroEdit is a non-modal, mouse-friendly terminal editor for people who do not want to
learn Vim. Menus, tabs, a file explorer, syntax highlighting and Git — in one static
binary, over SSH, with no runtime dependencies.

## Status

**Pre-alpha — Phase 14 (Polish, in progress).** `ferroedit file.txt` opens, edits
and saves a real file: rope-backed buffer, grapheme-correct cursor movement, a viewport
that scrolls both ways, and line endings written back the way they were found.
`ferroedit new.txt` starts an empty buffer and creates the file on the first `Ctrl+S`.

Selection works the way it does everywhere else: Shift with any motion, `Ctrl+A`, mouse
drag, double-click for a word, and `Ctrl+C`/`Ctrl+X`/`Ctrl+V`. Copying goes to the
terminal over OSC52, so it reaches your own clipboard even across SSH, with an internal
register behind it; pasting from the terminal arrives as one operation rather than as a
burst of keystrokes.

`Ctrl+Z` and `Ctrl+Y` undo and redo semantically: a typed word comes back as a word
rather than a letter at a time, replacing a selection undoes as one step, and a paste
of a thousand lines is one step too. History costs what was edited, never a copy of the
document.

Tabs are real: name several files on the command line and switch between them with
`Ctrl+Tab`, `Ctrl+PageUp`/`Ctrl+PageDown` or the mouse. A `●` marks unsaved changes,
`Ctrl+W`, the tab's `×` and middle click all close a tab, and closing — or quitting —
with unsaved work asks first rather than discarding it. The bar scrolls when more
files are open than fit.

The explorer is real: `ferroedit .` opens a project and the tree reads each directory
only when you expand it, with git-ignored and hidden files left out (`View → Show
Hidden Files` brings them back). `Enter` or one click opens a file, `Right` and `Left`
walk the tree, and `Ctrl+N`, `F2` and `Delete` create, rename and delete — deleting
asks first, and renaming follows any tab that is showing the file.

Code is coloured. Rust, Python, Go, JavaScript, TypeScript, JSON, YAML, TOML, Markdown,
HTML, CSS, shell and Dockerfiles all highlight, chosen by file name first and extension
second, with a shebang as the last resort. Only the lines on screen are ever parsed,
and the parser's state is checkpointed every 64 lines, so typing at the end of a 3.3 MB
file costs about as much as typing at the top of an empty one. A file over 5 MB, or one
with a line over 4 KB, is shown as plain text and says so in the status bar.

`Ctrl+F` opens a find bar under the editor, seeded with whatever is selected; `Ctrl+H`
adds a replacement row. Every hit in the file is highlighted at once and the current one
is selected, `Enter` and `F3` step through them and wrap, `Alt+C` toggles match case,
and `Alt+A` replaces every hit as a single undo step. `F3` works with the bar closed.
Everything on the bar is clickable, and all of it is on the Search menu too — which is
the way out for terminals that swallow Alt.

Every menu entry and every shortcut runs a real command — there is no longer a menu item
wired to a placeholder — and both [docs/SHORTCUTS.md](docs/SHORTCUTS.md) and the `F1`
help screen are generated from the keymap itself rather than written beside it. `Ctrl+O` opens a path from outside the workspace and File → Save As…
writes the current buffer somewhere else, keeping its undo history.

Long lines are read whichever way suits the file. `Alt+Z` (or View → Word Wrap) breaks
them at the width of the pane, on word boundaries, with the continuation rows under a
blank gutter and `Up`/`Down`/`Home`/`End` moving by what is drawn rather than by whole
lines; with it off, `Alt+Left`/`Alt+Right`, View → Scroll Left/Right, `Shift`+wheel and a
horizontal wheel move the window sideways instead — the menu entries are the way out for
terminals that never deliver `Alt`. The choice is remembered between runs. `Ctrl+G` jumps to a line
by number.

FerroEdit watches the workspace, so a `git checkout`, a rebase or a file written in
another terminal shows up in the tree and the panel by itself rather than on `F5`. The
watch is filtered — what git ignores is ignored here too — so a build writing thousands
of files costs nothing.

The Git panel is real, and it is the system `git` you already have: FerroEdit runs
`git status --porcelain=v2`, never a bundled reimplementation, so your config, your
ignore rules and your rename detection are the ones that apply. The sidebar shows the
branch, how far ahead of or behind its upstream it is, and every changed file in git's
own two-column form — `M` for modified, `A` for added, `D` for deleted, `R` for renamed,
`U` for a conflict and `?` for untracked. It refreshes itself after a save, a create, a
rename or a delete, and `F5` re-reads it after something changed the tree from outside.
A directory that is not a repository simply says so.

The panel acts, too. `Space` stages the selected file and unstages it again, `a` and `u`
do the same for everything at once, `c` asks for a commit message, `Enter` opens the
file, and Pull and Push are Git menu entries. Everything that writes to the repository
runs on a worker thread, so a push over a slow link shows `Git — Pushing…` in the panel
while the editor keeps drawing and typing; when it finishes, the status bar gets git's
own sentence about it — `[main 5944bbe] add the worker`, `Already up to date.`, or
`Push failed: No configured push destination.` No credential handling is bundled: git
runs with prompts turned off, so it uses your helper and your keys or it says why it
could not.

Branches are a picker: `b` in the panel (or Git → Branch…) lists every local branch and
every remote-tracking one, with `*` on the branch you are on and the highlight starting
there, so Enter without reading changes nothing. Picking a remote row creates the local
branch that tracks it. `New…` is right there in the picker; `m` opens the same list to
merge from.

A merge that conflicts is a state the editor can finish. The panel title reads
`Git — main [merging] (1)` and stays that way until the merge is committed — not just
until the last conflict is gone, which is the moment people get lost. `Enter` on a
conflicted row opens the file with git's markers in it, you resolve it in the editor,
`Ctrl+S`, then `Space` stages it as resolved and `c` commits the merge. Staging a file
that still has `<<<<<<<` in it asks first.

`d` in the panel — or Git → Diff, which diffs the file you are editing — opens the
unified diff over the editor, read-only and coloured: additions green, removals red,
hunk headers cyan. It shows what you have not staged yet, or what you have when there is
nothing left unstaged, and the title says which; `s` swaps sides. It scrolls like a
pager, sideways too for lines wider than the pane, and it follows the file it is showing
— staging what is on screen closes it rather than leaving a diff that is no longer true.
`Esc` puts you back where you were. See [docs/PROGRESS.md](docs/PROGRESS.md) and
[docs/ROADMAP.md](docs/ROADMAP.md).

```bash
cargo run -- .                    # a project: the tree, tabs, editing
cargo run -- /tmp/scratch.txt     # type, Ctrl+S to save, Ctrl+Q to quit
```

## Installation

Download a build from [the latest release](https://github.com/korkholeh/ferroedit/releases/latest).
Each one is a single binary with no runtime dependencies; the Linux builds are statically
linked against musl, so they run on any distribution.

```bash
tar -xzf ferroedit-x86_64-unknown-linux-musl.tar.gz
./ferroedit .
```

Every archive ships a `.sha256` beside it:

```bash
sha256sum -c ferroedit-x86_64-unknown-linux-musl.tar.gz.sha256
```

Targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`aarch64-apple-darwin`, `x86_64-apple-darwin`. Windows is not part of the MVP.

### From source

```bash
git clone https://github.com/korkholeh/ferroedit
cd ferroedit
cargo build --release
```

The binary lands in `target/release/ferroedit`. For the static Linux builds:

```bash
cross build --release --target x86_64-unknown-linux-musl
cross build --release --target aarch64-unknown-linux-musl
```

## Usage

```bash
ferroedit              # open the current directory
ferroedit .            # same
ferroedit src/main.rs  # open a file
ferroedit a.rs b.rs    # open two files, one tab each
ferroedit +42 main.rs  # open a file at line 42
```

## Requirements

- A terminal with mouse support (iTerm2, Ghostty, Terminal.app, kitty, tmux, …).
- The system `git` binary (2.23 or newer, for `git switch`), for the Git panel.
  FerroEdit ships no Git implementation
  of its own — your existing credentials, SSH config and commit signing keep working.

## Features (MVP scope)

- Non-modal editing with the shortcuts you already know
- Mouse: click, drag-select, scroll, menus, tabs, explorer
- Tabs, a `.gitignore`-aware file explorer, and file operations
- Syntax highlighting
- Search and replace, and Go to Line
- Word wrap, or horizontal scrolling when it is off
- Git: status, stage/unstage, commit, pull, push, branches, merge, diff
- One static binary, SSH-friendly

## Keyboard shortcuts

`F1` (or Help → Shortcuts) shows them in the editor. The same tables render
[docs/SHORTCUTS.md](docs/SHORTCUTS.md), so neither can drift from the code. Regenerate it with `ferroedit --dump-shortcuts >
docs/SHORTCUTS.md`, or `FERROEDIT_UPDATE_DOCS=1 cargo test`; a plain `cargo test` fails
when the checked-in copy is stale (ADR-028).

## Development

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

### Cutting a release

`scripts/release.sh` bumps the version in `Cargo.toml`, refreshes `Cargo.lock`, runs the
three commands above and then commits and tags. It pushes nothing: everything it does is
undoable with `git reset` and `git tag -d`, and the push is the first step that is not.

```bash
scripts/release.sh patch --dry-run   # 0.1.0 -> 0.1.1, and stop
scripts/release.sh minor             # bump, gate, commit, tag
git push origin main v0.2.0          # this is what publishes
```

Pushing the tag starts [`release.yml`](.github/workflows/release.yml), which drafts a
GitHub Release, builds the four targets, attaches a tarball and a checksum for each, and
publishes only once all four are in.

Docs: [ARCHITECTURE](docs/ARCHITECTURE.md) · [PLAN](docs/PLAN.md) ·
[ROADMAP](docs/ROADMAP.md) · [PROGRESS](docs/PROGRESS.md) ·
[DECISIONS](docs/DECISIONS.md)

## License

MIT
