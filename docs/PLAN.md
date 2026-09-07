# FerroEdit — Implementation Plan

Derived from `SPEC.md`. This document is the working plan: architecture, dependencies,
risks, and a phase-by-phase build order sized for incremental Claude Code sessions.

---

## 1. Architecture

### 1.1 Layering rule

```
main.rs        → CLI parse, terminal setup/teardown, panic hook, run loop
app/           → App state + command dispatch (no rendering, no crossterm draw calls)
event/         → terminal events + worker messages → AppEvent → Command
ui/            → pure render functions: fn(&App, &mut Frame) — never mutates App
editor/        → headless core: Rope, Cursor, Selection, History, Search. No TUI types.
filesystem/    → lazy tree, file IO
git/           → GitService (subprocess), porcelain=v2 parser, models
syntax/        → syntect wrapper + per-line highlight cache
config/        → TOML settings, theme
commands/      → Command enum + execute()
```

Hard invariants:

- `editor/` compiles and is fully testable without `ratatui`/`crossterm`.
- `ui/` is read-only over `&App`. Any mutation goes through a `Command`.
- One `Command` enum. Keyboard, menu, mouse, and context menu all produce `Command`;
  `execute_command(&mut App, Command)` is the single mutation entry point.

### 1.2 Module tree

Follows SPEC §7, with additions:

```
src/
  main.rs
  cli.rs                  # arg parsing, `+42 file.rs`
  terminal.rs             # RAII TerminalGuard + panic hook   <-- added
  app/
    mod.rs                # struct App
    focus.rs
    notifications.rs
    workspace.rs
  event/
    mod.rs                # AppEvent enum, input thread
    keyboard.rs           # keymap table: (modifiers, code, focus) -> Command
    mouse.rs              # hit-testing against last layout rects
  commands/
    mod.rs                # enum Command
    execute.rs
  ui/
    mod.rs layout.rs menu.rs tabs.rs editor.rs explorer.rs
    git.rs statusbar.rs dialog.rs diff.rs theme.rs
  editor/
    mod.rs document.rs cursor.rs selection.rs history.rs search.rs
    coords.rs             # byte/char/grapheme/visual-column conversions  <-- added
    clipboard.rs          # ClipboardProvider trait + OSC52 + internal
  filesystem/
    mod.rs tree.rs
  git/
    mod.rs service.rs parser.rs models.rs worker.rs               <-- worker added
  syntax/
    mod.rs highlighter.rs cache.rs
  config/
    mod.rs settings.rs
```

### 1.3 Event loop

Single-consumer channel; input runs on its own thread so background workers can
push results without polling gymnastics.

```
[input thread]  crossterm::event::read()  ─┐
[git worker]    Command output            ─┼──► mpsc::Receiver<AppEvent> ──► handle_event
[fs scanner]    directory listings        ─┘                                     │
                                                                                 ▼
                                                                          execute_command
                                                                                 │
                                                                                 ▼
                                                              terminal.draw(|f| ui::render(f, &app))
```

Render only after the channel drains (`try_recv` loop), so a paste burst or a fast
scroll produces one frame, not N.

### 1.4 Coordinate systems (SPEC §13 — the main correctness risk)

Four distinct types, never `usize` aliases:

| Type | Meaning | Source of truth |
|---|---|---|
| `ByteIdx` | rope byte offset | file IO, syntect |
| `CharIdx` | rope char offset | **canonical cursor storage** |
| `GraphemeIdx` | user-perceived char | movement steps |
| `VisualCol` | display columns | rendering, mouse hit-test, preferred column |

Rules:
- Cursor stores `(line: usize, char_in_line: usize)` plus `preferred_visual_col`.
- Left/Right move by **grapheme cluster** (`unicode-segmentation`), not char.
- Visual column = sum of `unicode_width` over graphemes, with tabs expanded to the
  next `tab_width` stop. Zero-width joiners and combining marks contribute 0.
- Mouse click → `visual_col` → nearest grapheme boundary (round to the cluster
  whose cell range contains the click).

All conversions live in `editor/coords.rs` and are unit-tested against the SPEC §48
fixtures before any UI depends on them.

### 1.5 Undo model

```rust
enum EditOperation {
    Insert { at: CharIdx, text: String },
    Delete { at: CharIdx, text: String },   // text kept for inverse
}
struct Transaction { ops: Vec<EditOperation>, cursor_before: Cursor, cursor_after: Cursor, at: Instant }
```

`History { undo: Vec<Transaction>, redo: Vec<Transaction> }`. Memory is O(edited
bytes), never O(document). Coalescing: append to the open transaction when the new
edit is adjacent, same kind, within ~500ms, and not crossing a word/newline boundary.
Any cursor jump, save, or selection change seals the transaction.

### 1.6 Syntax highlighting

- syntect with `ParseState`/`HighlightState` **checkpointed per line**.
- Cache: `Vec<Option<StateCheckpoint>>` parallel to lines, plus a `dirty_from: usize`
  watermark. An edit on line N invalidates `N..`; re-parse resumes from the last valid
  checkpoint at or before the viewport top, not from line 0.
- Only viewport lines are highlighted per frame. Files over a threshold (e.g. 5 MB or
  a single line over 4 KB) fall back to plain text with a status-bar note.

### 1.7 Async

`std::thread` + `mpsc`. No Tokio — the workload is a handful of subprocess calls and
directory reads; Tokio adds a runtime, a dependency tree, and no simplification here.
Long git ops get a `JobId`, the UI shows `Pushing…`, and the worker replies with
`AppEvent::GitJobDone(JobId, Result<Output, GitError>)`.

---

## 2. Dependencies

| Crate | Purpose | Notes |
|---|---|---|
| `ratatui` | TUI widgets/layout | core |
| `crossterm` | terminal backend, raw mode, mouse, events | core |
| `ropey` | rope text buffer | core |
| `unicode-segmentation` | grapheme clusters | cursor movement |
| `unicode-width` | display width | visual columns, wide CJK/emoji |
| `syntect` | syntax highlighting | **`default-features = false`, features = `["default-fancy"]`** → pure-Rust `fancy-regex` instead of C `onig`; keeps static musl builds trivial |
| `ignore` | .gitignore-aware walking | explorer |
| `serde` + `toml` | config | |
| `directories` | XDG / macOS config+state paths | avoids hand-rolled path logic (SPEC §42) |
| `anyhow` | error context in app paths | |
| `thiserror` | typed errors in `editor/`, `git/` | library-ish modules get real error enums |
| `clap` (derive) | CLI, `--help`, `--version` | small; can be hand-rolled if binary size matters |
| `log` + `simplelog` (or `tracing-subscriber` file layer) | file logging, never stdout | SPEC §46 |

Optional / feature-gated:

| Crate | Purpose | Why gated |
|---|---|---|
| `arboard` | native system clipboard | pulls X11/Wayland libs on Linux, breaks pure-static goal, useless over SSH. Behind `--features native-clipboard`, off by default. |

Dev-dependencies: `insta` (snapshot tests for the git parser and rendered
`TestBackend` buffers), `tempfile` (fs and git integration tests).

Explicitly rejected: `libgit2`/`git2` (SPEC §5), `tree-sitter` (SPEC §21), Tokio (§1.7).

---

## 3. Technical risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | **Unicode width vs terminal reality.** `unicode-width` disagrees with terminals on emoji ZWJ sequences (`👨‍👩‍👧`), regional indicators, and VS16. Cursor drifts from the visible caret. | High — visible corruption | Grapheme-cluster-first movement; width computed per cluster with an explicit override table; fixtures from SPEC §48 as unit tests; accept known-imperfect ZWJ width and log it in PROGRESS as a known issue rather than blocking. |
| R2 | **syntect + onig breaks musl static builds.** Default features link C oniguruma. | High — blocks distribution goal | Use `default-fancy` from day one (Phase 0), not after the fact. Verify a musl build in CI at Phase 0, before syntect is even wired up. |
| R3 | **syntect default syntax set is incomplete.** No TSX/JSX, no Dockerfile, weak TOML. | Medium — SPEC §21 list unmet | Bundle extra `.sublime-syntax` files, compile them into a `SyntaxSet` dump at build time (`build.rs`) and `include_bytes!` it. Keeps startup fast and the binary self-contained. |
| R4 | **Mouse capture steals native terminal selection.** Users can no longer select/copy with the mouse via the terminal itself. | Medium — UX complaint | Provide `View → Toggle Mouse Capture` and document that most terminals bypass capture with Shift+drag. |
| R5 | **Key combos terminals cannot deliver.** `Ctrl+Shift+Tab`, most macOS `Cmd+…`, and `Ctrl+/` need the kitty keyboard protocol. | Medium | Try `PushKeyboardEnhancementFlags` (crossterm), detect support, and always ship a plain-ANSI fallback binding for every command. Cmd is explicitly non-MVP per SPEC §2.2. |
| R6 | **Clipboard over SSH.** No X11/Wayland/pasteboard on the remote side. | Medium | `ClipboardProvider` trait; default chain = OSC52 write → internal register read; `arboard` only behind a feature flag. Never let clipboard failure abort an edit. |
| R7 | **Bracketed-paste flood.** Pasting 10k lines as individual key events is O(n) rope ops and O(n) renders. | Medium — perceived hang | Enable bracketed paste; handle `Event::Paste` as one insert + one undo transaction. Coalesce renders (§1.3). |
| R8 | **Highlight cache invalidation bugs.** Stale checkpoints → wrong colors after edits. | Medium | Watermark invalidation only (never partial patching); property test: highlight-from-scratch == highlight-after-random-edits. |
| R9 | **git subprocess blocking + interactive prompts.** `git push` may block forever on a credential or host-key prompt. | High — frozen worker | Always run with `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, `-c core.pager=cat`, and a job timeout. Surface the failure as a dialog telling the user to configure credentials outside the editor. |
| R10 | **`porcelain=v2` parsing.** Renames use tab-separated paths; paths with spaces/quotes; `-z` changes the framing. | Medium — wrong git panel | Use `git status --porcelain=v2 -z --branch` and split on NUL. Snapshot tests over recorded fixtures (rename, conflict, untracked, submodule). |
| R11 | **Terminal left in raw mode after panic.** | High — user's shell is broken | `TerminalGuard` with `Drop` + `std::panic::set_hook` that restores first, then prints the payload. Test with a deliberate `--panic-test` hidden flag. |
| R12 | **aarch64 musl cross-compilation** in CI. | Low-Medium — release only | Use `cross` (docker) for the two musl targets; native runners for macOS arm64/x86_64. Validate at Phase 0 so it never becomes an end-of-project surprise. |
| R13 | **Very long single lines** (minified JS) break viewport math and syntect. | Low | Horizontal scrolling from Phase 2; hard cap highlighting per line. |

---

## 4. Spec ambiguities to resolve before coding

1. **§10 says `ted` opens the current directory** — leftover from an older name.
   Assumption: the binary is `ferroedit`; bare `ferroedit` opens `.`. No `ted` alias.
2. **Quit shortcut.** §2.2 lists `Ctrl+W` (close tab) but no quit key; Phase 1
   acceptance uses `Ctrl+Q`. Assumption: `Ctrl+Q` = Quit. Needs `IXON` off (raw mode
   already handles it).
3. **`Ctrl+Z` is SIGTSTP** in cooked mode. Raw mode disables `ISIG`, so it arrives as
   a key event — undo works, but suspend-to-shell is then unavailable. Assumption:
   undo wins, no suspend in MVP.
4. **Redo key.** `Ctrl+Y` per §16, with `Ctrl+Shift+Z` as an alias where the terminal
   can deliver it.
5. **Config precedence** between file and CLI flags is unspecified. Assumption:
   CLI > config file > defaults.

---

## 5. Build order

Each phase = one working branch, small commits, ending green on
`cargo fmt --check && cargo clippy -- -D warnings && cargo test`, plus a
`docs/PROGRESS.md` update. Sizes are rough session counts, not calendar estimates.

### Phase 0 — Bootstrap  (~1 session)
Cargo project, module skeleton with empty files, `rustfmt.toml`, `clippy.toml`,
GitHub Actions (fmt + clippy + test on linux/macos), release workflow stub with the
**4 target triples building an empty binary** (validates R2/R12 early).
Docs: README, ARCHITECTURE, ROADMAP, PROGRESS, DECISIONS (ADR-001 git CLI,
ADR-002 no Tokio, ADR-003 syntect-fancy, ADR-004 grapheme cursor).
**Acceptance:** all four targets link; CI green.

### Phase 1 — TUI shell  (~1–2 sessions)
`TerminalGuard` + panic hook, input thread, `AppEvent`, main loop, `ui::layout` with
all five zones drawn from mock data, `Command::Quit`, mouse capture on.
**Acceptance:** launches; `Ctrl+Q` exits; terminal fully restored (incl. after a
forced panic); resize works down to 60×20; mouse events logged.

### Phase 2 — Editor core  (~2–3 sessions)
`coords.rs` **first, with the §48 Unicode fixtures**, then `Document`(Rope), `Cursor`,
viewport with vertical + horizontal scrolling, insert/backspace/delete/newline,
open/save preserving line endings (detect LF/CRLF on load, write back the same).
**Acceptance:** `ferroedit test.txt` edits and saves; Unicode fixture tests pass.

### Phase 3 — Selection + clipboard  (~1–2 sessions)
`Selection`, Shift+navigation, `Ctrl+A`, mouse click/drag/double-click-word,
`ClipboardProvider` (OSC52 + internal), bracketed paste as one operation.
**Acceptance:** cut/copy/paste round-trips; drag-select across wide chars is correct.

### Phase 4 — Undo/redo  (~1 session)
`History`, transactions, coalescing, cursor restore.
**Acceptance:** typing a word then `Ctrl+Z` undoes the word, not one letter;
redo restores it; memory stays flat over 100k keystrokes (bench test).

### Phase 5 — Tabs  (~1 session)
`Vec<Tab>`, switching, dirty marker, close-with-confirm, reuse-existing-tab on open.
**Acceptance:** 10+ files open, switch with mouse and `Ctrl+Tab`, no leaks.

### Phase 6 — Explorer  (~1–2 sessions)
Lazy tree over `ignore`, expand/collapse, keyboard + mouse, active-file highlight,
file ops (new/rename/delete with confirmation) via the dialog system.
**Acceptance:** `ferroedit .` navigates a real project; ignored files hidden.

### Phase 7 — Syntax highlighting  (~2 sessions)
`build.rs` syntax dump, checkpoint cache, viewport-only highlighting, big-file
fallback, theme mapping to `ratatui::Style`.
**Acceptance:** the SPEC §21 language list highlights; typing in a 5 MB file stays
responsive (add a `criterion` bench or a simple frame-time assertion).

### Phase 8 — Search / replace  (~1 session)
`Ctrl+F` / `Ctrl+H` bar, next/prev, case toggle, replace, replace-all as one undo
transaction, match highlighting in the viewport.

### Phase 9 — Menus + command wiring  (~1 session)
Full `Command` enum, keymap table, menu bar driven by the same commands, mouse menu
navigation, `docs/SHORTCUTS.md` generated from the keymap table so it cannot drift.
**Acceptance:** every menu item and every shortcut resolves to a `Command`; no
duplicated logic (enforced by making `execute_command` the only mutator).

### Phase 10 — Git status  (~1–2 sessions)
`GitService`, repo detection, `--porcelain=v2 -z --branch` parser with fixtures,
sidebar rendering, "Not a Git repository" state, status refresh on save + on focus.

### Phase 11 — Git actions  (~1–2 sessions)
Worker thread + `JobId`, stage/unstage/stage-all, commit dialog, pull, push,
in-progress and completion notifications, error dialogs.
**Acceptance:** a push against a real remote never blocks the UI; `GIT_TERMINAL_PROMPT=0`
failures show an actionable message.

### Phase 12 — Branches + merge  (~1 session)
Branch picker dialog, switch, create, merge, conflicted-file display.

### Phase 13 — Diff viewer  (~1 session)
Read-only unified diff pane from `git diff -- path`, with +/− coloring.

### Phase 14 — Polish  (~ongoing)
Help screen, error message pass, responsive layout for small terminals, theme
cleanup, profiling, Unicode bug fixes, README screenshot/mockup, release binaries.

---

## 6. Testing strategy

- **Unit (headless, `editor/`)**: coords conversions, cursor movement, insert/delete,
  selection, undo/redo, search. The §48 fixtures (`Hello`, `Привіт`, `Україна`,
  `日本語`, `🙂`, `👨‍👩‍👧`, `é` precomposed *and* decomposed) live in
  `tests/fixtures/unicode.txt` and are exercised by every movement test.
- **Property tests**: random edit sequences — undo-all returns the original rope;
  incremental highlighting equals from-scratch highlighting.
- **Snapshot (`insta`)**: git porcelain-v2 parser over recorded fixtures; a few
  `ratatui::TestBackend` layout snapshots to catch gross regressions.
- **Integration (`tempfile`)**: real temp git repos for `GitService` (init, commit,
  branch, merge conflict), real temp dirs for the explorer and .gitignore behavior.
- **Manual checklist per phase**, recorded in `docs/PROGRESS.md`: terminal restore
  after quit *and* after panic, mouse in the actual target terminals
  (iTerm2, Ghostty, Terminal.app, tmux, plain ssh).

---

## 7. Per-session workflow (SPEC §52)

1. Re-read `docs/ARCHITECTURE.md` + `docs/PROGRESS.md`.
2. State the phase's file list and plan before editing.
3. Implement in small commits (`feat:` / `fix:` / `test:` / `docs:`).
4. `cargo fmt && cargo clippy -- -D warnings && cargo test` — all green, no warnings.
5. Manual smoke test in a real terminal.
6. Update `PROGRESS.md` (completed / in progress / known issues / next) and add an
   ADR to `DECISIONS.md` for any non-obvious choice.
7. Do not start the next phase until the current one is clean.

---

## 8. First session scope (SPEC §63)

Phase 0 + Phase 1 only:

- Cargo project + module skeleton + CI + four-target release check.
- The five docs, with ADR-001…004 written.
- Terminal guard, panic hook, input thread, event loop, five-zone layout with mock
  data, `Ctrl+Q` quit, mouse capture.
- Green fmt/clippy/test.

Stop there. Review the architecture against Phase 2's needs before continuing.
