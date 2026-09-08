# FerroEdit — Architecture

The authoritative *what* is `SPEC.md`; the phase-by-phase *when* is `docs/PLAN.md`.
This document is the *how*: layering, data flow, and the invariants that must survive
every future change.

---

## 1. Layering

```
main.rs        → CLI parse, terminal setup/teardown, panic hook, run loop
app/           → App state + command dispatch (no rendering, no crossterm calls)
event/         → terminal events + worker messages → AppEvent → Command
ui/            → pure render functions: fn(&App, &mut Frame) — never mutates App
editor/        → headless core: Rope, Cursor, Selection, History, Search. No TUI types.
filesystem/    → lazy tree, file IO
git/           → GitService (subprocess), porcelain=v2 parser, worker thread
syntax/        → syntect wrapper + per-line highlight cache
config/        → TOML settings, theme
commands/      → Command enum + execute()
```

### Invariants

1. **`editor/` has no TUI dependency.** It compiles and is fully testable without
   `ratatui` or `crossterm`. Everything about text — coordinates, cursor, selection,
   undo, search — is exercised headlessly.
2. **`ui/` is read-only over `&App`.** Rendering never mutates state. Anything a
   render function "wants to change" is a bug or a missing `Command`.
3. **One mutation entry point.** Keyboard, menu, mouse and dialogs all produce the
   same `Command` enum, and `execute_command(&mut App, Command)` is the only function
   that mutates `App`. This is what keeps menu items and shortcuts from drifting apart,
   and it is what makes `docs/SHORTCUTS.md` generated from the keymap table — which it
   is, from Phase 9 on (`src/docs.rs`, ADR-028).
   The single carve-out is `App::editor_view`, the size of the editor pane in the last
   drawn frame: it is written by the main loop and read by scrolling (ADR-010).
4. **Derived views are prepared, never computed while rendering.** The run loop calls
   `App::sync_highlight` and `App::sync_search` before each draw, so `ui/` reads colours
   and match positions the same way it reads text — out of `&App`. A renderer that
   parsed or searched as it drew would either break invariant 2 or throw the cache away
   every frame.

## 2. Module tree

```
build.rs                  # links the syntax set once, at compile time (ADR-022)
assets/syntaxes/          # the grammars syntect's defaults are missing
src/
  main.rs
  cli.rs                  # arg parsing, `+42 file.rs`
  terminal.rs             # RAII TerminalGuard + panic hook
  docs.rs                 # renders docs/SHORTCUTS.md from the tables (ADR-028)
  app/       mod.rs focus.rs tabs.rs dialog.rs diff.rs input_field.rs
             browser.rs notifications.rs search.rs workspace.rs
  event/     mod.rs keyboard.rs mouse.rs
  commands/  mod.rs execute.rs
  ui/        mod.rs layout.rs menu.rs tabs.rs editor.rs explorer.rs field.rs
             git.rs search.rs statusbar.rs dialog.rs diff.rs theme.rs
  editor/    mod.rs document.rs cursor.rs selection.rs history.rs search.rs
             coords.rs viewport.rs clipboard.rs
  filesystem/ mod.rs tree.rs      # file operations; lazy, ignore-aware tree
  git/       mod.rs service.rs parser.rs models.rs diff.rs worker.rs
  syntax/    mod.rs highlighter.rs cache.rs
  config/    mod.rs settings.rs
```

## 3. Event loop

A single-consumer `mpsc` channel. Input is read on its own thread so background
workers can push results without polling gymnastics.

```
[input thread]  crossterm::event::read()  ─┐
[git worker]    command output            ─┼──► mpsc::Receiver<AppEvent> ──► handle_event
[fs scanner]    directory listings        ─┘                                     │
                                                                                 ▼
                                                                          execute_command
                                                                                 │
                                                                                 ▼
                                                              terminal.draw(|f| ui::render(f, &app))
```

The loop drains the channel with `try_recv` before drawing, so a paste burst or a fast
scroll produces one frame instead of N.

## 4. Coordinate systems

The main correctness risk in the whole project (SPEC §13). Four distinct types —
never bare `usize` aliases:

| Type | Meaning | Used by |
|---|---|---|
| `ByteIdx` | rope byte offset | file IO, syntect |
| `CharIdx` | rope char offset | **canonical cursor storage** |
| `GraphemeIdx` | user-perceived character | movement steps |
| `VisualCol` | display columns | rendering, mouse hit-test, preferred column |

Rules:

- A cursor stores `(line, char_in_line)` plus a `preferred_visual_col` that survives
  vertical movement across short lines. Horizontal movement rewrites it; vertical
  movement only reads it.
- Left/Right move by **grapheme cluster**, not char — `é` as `e` + U+0301 is one step.
- Visual column = sum of `unicode_width` per cluster, tabs expanded to the next
  `tab_width` stop; zero-width joiners and combining marks contribute 0.
- A mouse click maps `visual_col` → the grapheme cluster whose cell range contains it.

All conversions live in `editor/coords.rs` and are unit-tested against the SPEC §48
fixtures (`tests/fixtures/unicode.txt`) before any UI depends on them. Every function
there takes one line as `&str`, so the module is testable without a rope; `Document` is
what knows about lines, and `editor/viewport.rs` is what knows how many of them fit.

The caret the user sees is the terminal's own cursor, positioned by the editor widget
while the editor has focus. It therefore blinks the way the user's terminal is
configured to, and a screen reader follows it.

## 5. Undo model

```rust
enum EditOperation {
    Insert { at: CharIdx, text: String },
    Delete { at: CharIdx, text: String },   // text kept for the inverse
}
struct Transaction { ops: Vec<EditOperation>, cursor_before: Cursor, cursor_after: Cursor, at: Instant }
```

`History { undo: Vec<Transaction>, redo: Vec<Transaction> }`. Memory is O(edited
bytes), never O(document) — no document snapshots. Consecutive edits coalesce into the
open transaction when they are adjacent, the same kind, within ~500 ms, and do not
cross a word or newline boundary. A cursor jump, a save, or a selection change seals it.

## 6. Dialogs

One shape for every modal window (SPEC §40): a title, a body and a row of buttons,
where each button carries the `Command` that choosing it runs. A dialog therefore has
no event handling of its own — it feeds the same `execute_command` path as the
keyboard, the menu and the mouse, and a new dialog is a constructor rather than a new
input mode.

The body is the enum: `Message` for a confirmation, `Input` for a name (ADR-019), `List`
for a choice — the branch picker, and the one case where no pane behind the dialog
already lists the same things (ADR-035) — and `Browser` for Open, a directory listing
with a filter over it (ADR-051). A dialog with a field has its own keymap table, because
`Left` is a caret there and a button selection everywhere else, and `resolve` picks the
table from the open dialog. `Up` and `Down` are the list's axis the way `Left` and
`Right` are the button row's, and they are in *both* tables: the browser is a field and a
list at once, while a body that is not a list ignores them.

A button whose command needs something that does not exist yet carries a marker instead,
and `activate_dialog_button` substitutes on the way out:

| Button carries | Becomes | Because |
|---|---|---|
| `SubmitInput(op)` | `ApplyFileOp(op, text)` | the field stops existing when the dialog closes |
| `SubmitCommit` | `GitCommit(text)` | the same, for a message |
| `SubmitBranch` | `GitCreateBranch(text)` | the same, for a name |
| `SubmitListChoice` | the highlighted row's own command | the choice is made after the button is built |

The browser's `BrowserOpen` and `BrowserOpenFolder` are the exception to the rule below
that a button runs with its dialog already closed: one of them walks into a directory,
which is the dialog *staying* open, and both read a listing that closing would drop.

The box's height comes from the body — `DialogState::body_height` — so a picker is as
tall as its list while a message and an input stay at the five rows they always were. The
browser asks for the most (twenty rows, a frame and a scrollbar), and the terminal clamps
it, which is why the window it *scrolls* within is read back out of the drawn frame as
`App::dialog_rows` rather than being a constant — the same arrangement `explorer_rows`
has.

Modality is enforced at the two input boundaries, not inside `execute_command`:
`event::keyboard::resolve` consults only the `Dialog` bindings while a dialog has
focus (so no global shortcut can act behind an open prompt), and
`event::mouse::hit_test` returns a button index or nothing at all. Everything a button
runs happens with the dialog already closed, which is what keeps the recursion at one
level — a button that opens a dialog of its own, like the picker's New…, replaces this
one rather than nesting inside it.

## 7. Syntax highlighting

syntect with `ParseState`/`HighlightState` checkpointed per line. The cache is a
`Vec<Option<StateCheckpoint>>` parallel to the lines plus a `dirty_from` watermark: an
edit on line N invalidates `N..`, and re-parsing resumes from the last valid checkpoint
at or before the viewport top rather than from line 0. Only viewport lines are
highlighted per frame. Files past a threshold (~5 MB, or a single line over 4 KB) fall
back to plain text with a status-bar note.

Invalidation is watermark-only — never partial patching. A property test asserts that
highlight-from-scratch equals highlight-after-random-edits.

## 8. Concurrency

`std::thread` + `mpsc`, no async runtime (ADR-002). Reading `git status` is not one of
the long operations: it is a local read taken on the UI thread, on demand, after
anything that changes the tree (ADR-030). Everything that *writes* to the repository —
stage, unstage, commit, pull, push — runs on the git worker instead (ADR-033): one
thread, one `mpsc` in, and the loop's own `AppEvent` channel out.

```
GitState::start(GitJob)  ──►  [git worker]  ──►  AppEvent::GitJob(JobOutcome)
                                                        │
                                                        ▼
                                          Command::GitJobFinished(outcome)
```

Jobs are named by `JobId` and run serially, in submission order, so a commit queued
behind the staging it depends on sees that staging finish. The panel title shows the
oldest running job's `Pushing…` for as long as it runs, which a four-second notification
cannot. The outcome comes back as a `Command`, so the worker mutates `App` through the
same single door as the keyboard — which is why `JobOutcome` carries an error *string*
and not a `GitError`: a `Command` has to be `Clone` and `Eq`.

The filesystem watcher is the third thread (ADR-040), and the only one whose lifetime is
not the process's: `watcher::spawn` returns a `Watch` that owns the `notify` watcher, and
dropping it closes the channel the thread is blocked on. `App` holds that handle and a
clone of the loop's sender, so a workspace root that moves — Open Folder (ADR-051) — ends
the old watch and starts a new one without `execute_command` having to hand work back to
`main`.

Every git subprocess runs with `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`,
`GIT_EDITOR=true` and `-c core.pager=cat` plus a timeout — ten seconds for a local
command, two minutes for one that reaches a remote — so a credential prompt surfaces as
an actionable error instead of a frozen worker.

## 9. Terminal lifecycle

`TerminalGuard` restores the terminal in `Drop`, and `std::panic::set_hook` restores it
*before* printing the payload. Both paths exist because they cover different failures:
the guard handles normal returns and unwinding, the hook handles the case where the
payload would otherwise be printed into a raw-mode screen. Release builds keep
unwinding enabled for exactly this reason (ADR-006).

## 10. Testing

- **Unit, headless** — `editor/`: coords, movement, editing, selection, undo, search.
- **Property** — random edit sequences: undo-all returns the original rope;
  incremental highlighting equals from-scratch.
- **Snapshot (`insta`)** — the porcelain-v2 parser over recorded fixtures, plus a few
  `ratatui::TestBackend` layout snapshots.
- **Integration (`tempfile`)** — real temp git repos and real temp directories.
- **Manual per phase** — recorded in `docs/PROGRESS.md`: terminal restore after quit
  *and* after panic; mouse in iTerm2, Ghostty, Terminal.app, tmux and plain ssh.
