# FerroEdit

> A modern desktop-like text editor that happens to run in a terminal.

FerroEdit is a non-modal, mouse-friendly terminal editor for people who do not want to
learn Vim. Menus, tabs, a file explorer, syntax highlighting and Git — in one static
binary, over SSH, with no runtime dependencies.

```text
┌──────────────────────────────────────────────────────────────┐
│ File  Edit  Selection  Search  View  Git  Help               │
├────────────────┬─────────────────────────────────────────────┤
│ Explorer       │ main.rs │ editor.rs │ README.md             │
│                ├─────────────────────────────────────────────┤
│ ▼ src          │  1  fn main() {                             │
│   main.rs      │  2      println!("hello");                  │
│   editor.rs    │  3  }                                       │
│   git.rs       │                                             │
├────────────────┤                                             │
│ Git            │                                             │
│  M src/main.rs │                                             │
│  ? notes.txt   │                                             │
├────────────────┴─────────────────────────────────────────────┤
│ main.rs  Ln 2, Col 14  UTF-8  LF  Rust        branch: main   │
└──────────────────────────────────────────────────────────────┘
```

## Status

**Pre-alpha — Phase 8 (search and replace).** `ferroedit file.txt` opens, edits
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

Opening a file from *outside* the workspace is still the command line's job: `Ctrl+O`
and Save As need a dialog that browses rather than one that takes a name. See
[docs/PROGRESS.md](docs/PROGRESS.md) and
[docs/ROADMAP.md](docs/ROADMAP.md).

```bash
cargo run -- .                    # a project: the tree, tabs, editing
cargo run -- /tmp/scratch.txt     # type, Ctrl+S to save, Ctrl+Q to quit
```

## Installation

No release binaries yet. Build from source:

```bash
git clone https://github.com/korkholeh/ferroedit
cd ferroedit
cargo build --release
```

The binary lands in `target/release/ferroedit`.

### Static Linux builds

```bash
cross build --release --target x86_64-unknown-linux-musl
cross build --release --target aarch64-unknown-linux-musl
```

Supported targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`aarch64-apple-darwin`, `x86_64-apple-darwin`. Windows is not part of the MVP.

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
- The system `git` binary, for the Git panel. FerroEdit ships no Git implementation
  of its own — your existing credentials, SSH config and commit signing keep working.

## Features (MVP scope)

- Non-modal editing with the shortcuts you already know
- Mouse: click, drag-select, scroll, menus, tabs, explorer
- Tabs, a `.gitignore`-aware file explorer, and file operations
- Syntax highlighting
- Search and replace
- Git: status, stage/unstage, commit, pull, push, branches, merge, diff
- One static binary, SSH-friendly

## Keyboard shortcuts

See [docs/SHORTCUTS.md](docs/SHORTCUTS.md) — generated from the keymap tables, so it
cannot drift from the code. Regenerate it with `ferroedit --dump-shortcuts >
docs/SHORTCUTS.md`, or `FERROEDIT_UPDATE_DOCS=1 cargo test`; a plain `cargo test` fails
when the checked-in copy is stale (ADR-028).

## Development

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Docs: [ARCHITECTURE](docs/ARCHITECTURE.md) · [PLAN](docs/PLAN.md) ·
[ROADMAP](docs/ROADMAP.md) · [PROGRESS](docs/PROGRESS.md) ·
[DECISIONS](docs/DECISIONS.md)

## License

MIT
