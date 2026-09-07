# Current Phase

Phase 10 — Git status (not started)

## Completed

- **Phase 0 — Bootstrap**
  - Cargo project `ferroedit`, edition 2021, MSRV 1.75
  - Full module skeleton per `docs/ARCHITECTURE.md` §2 (every file exists with a
    `//!` header stating its responsibility; no logic yet)
  - `rustfmt.toml`, `clippy.toml`, `.gitignore`
  - Dependency set pinned, including `syntect` with `default-features = false,
    features = ["default-fancy"]` — pure-Rust regex, no C oniguruma (ADR-003)
  - CI (`.github/workflows/ci.yml`): fmt + clippy + test on linux/macos, plus a
    `targets` job that links all four release triples and asserts the Linux binaries
    are statically linked
  - Release workflow stub (`.github/workflows/release.yml`) and `Cross.toml`
  - SPEC §48 Unicode fixtures at `tests/fixtures/unicode.txt` (with precomposed *and*
    decomposed `é`), documented in `tests/fixtures/README.md`
  - Docs: README, ARCHITECTURE, ROADMAP, PROGRESS, DECISIONS (ADR-001…006)

- **Phase 1 — TUI shell**
  - `TerminalGuard` (RAII) + panic hook, both restoring raw mode, the alternate
    screen, mouse capture and bracketed paste (`src/terminal.rs`)
  - Input thread → `mpsc` → `AppEvent`; the main loop blocks for the first event and
    then drains with `try_recv`, so a paste burst costs one frame instead of N
  - Five-zone layout (menu bar, sidebar with explorer + git panels, tab bar, editor,
    status bar) drawn from mock data, with a "terminal too small" fallback below 40x8
  - `Command` enum with `execute_command` as the *only* `App` mutator; the keymap
    table, the menu and mouse hit-testing all feed it
  - Mouse: tab selection, menu open/activate/dismiss, pane focus, wheel scrolling
  - `Theme` struct — no colour is hardcoded in a widget (ADR-007)
  - Menu shortcut labels are read out of the keymap table, so the menu cannot
    advertise an unbound key (ADR-008)
  - CLI: `ferroedit`, `ferroedit .`, `ferroedit file.rs`, `+42` line argument,
    `--help`, `--version`
  - File logging to the platform state dir, filtered to the `ferroedit` crate;
    level from `FERROEDIT_LOG`. Nothing is ever written to stdout (SPEC §46)
  - 49 tests: keymap resolution, mouse hit-testing, layout tiling, command
    execution, CLI parsing, and `TestBackend` render assertions

- **Phase 2 — Editor core**
  - `editor/coords.rs` first, as planned: `ByteIdx` / `CharIdx` / `GraphemeIdx` /
    `VisualCol` as distinct newtypes, cluster iteration, tab-stop-aware widths,
    `visual_col` ↔ `char_at_visual_col`, grapheme and word motion, and `snap`, which
    is what keeps a cursor from ever landing inside a cluster. Every function takes one
    line as `&str`, so the module needs neither a rope nor a terminal to be tested.
  - 18 coords tests over the SPEC §48 fixture, including the round-trip property that
    `char_at_visual_col(visual_col(x)) == x` on every cluster boundary of every line
  - `Document`: `ropey::Rope`, UTF-8-only load with a real error for anything else,
    CRLF normalised on load and written back on save (ADR-009), insert / newline /
    backspace / delete by grapheme cluster, and the twelve motions of SPEC §13 with a
    preferred column that survives short lines
  - `editor/viewport.rs`: per-tab scroll offsets, minimal scroll-to-cursor in both
    axes, and the `gutter_width` that rendering *and* mouse hit-testing share, so a
    click and the character under it cannot disagree
  - `Tab` now holds a `Document` and a `Viewport`; `App::open_path` reuses an existing
    tab for an already-open file (SPEC §11)
  - Keymap: cursor motions, `Ctrl+Left/Right` by word, `Ctrl+Home/End`, Enter, Tab,
    Backspace, Delete, and `Ctrl+S`. Printable characters resolve to
    `Command::InsertChar` in `resolve` rather than in the table — a million bindings
    would not fit in it — so text entry still takes the one mutation path (SPEC §25).
  - The editor pane renders the rope through the viewport with tabs expanded and both
    edges clipped by cluster, and puts the *terminal's* cursor on the document cursor
    while the editor has focus
  - Mouse click in the editor places the cursor; the wheel still scrolls without
    moving it
  - Status bar shows the real `Ln`/`Col` (counted in user-perceived characters) and
    names the line ending only when it is CRLF
  - `NotificationKind::Error` arrived with its first producer: a failed save
  - A path with nothing behind it opens as an empty buffer that creates the file on the
    first save; anything else that fails to read (a directory, a permission error,
    invalid UTF-8) is still an error rather than a blank screen
  - `App::editor_view` mirrors the editor pane's size out of each frame, and the loop
    redraws immediately when it changes — which is what makes `ferroedit +42 file`
    land on line 42 rather than on the first frame's idea of it (ADR-010)
  - 144 tests (was 49)

- **Phase 3 — Selection and clipboard**
  - `editor/selection.rs`: `Position` ordered in reading order, `Selection` as an
    anchor/head pair, and `line_span`, which is what rendering asks per visible line.
    A backwards selection is the same object with its ends swapped.
  - `Document` stores the *anchor* only — the head is the cursor, so a highlight can
    never lag the caret by a keystroke (ADR-011). Plain motion clears the anchor,
    Shift motion leaves it, and both take the same twelve motions of SPEC §13.
  - Shift+Arrow/Home/End/PageUp/PageDown, `Ctrl+Shift+Left/Right`,
    `Ctrl+Shift+Home/End`, `Ctrl+A`, and mouse drag and double-click-word (SPEC §14,
    §27). Double-click is paired in `event/mouse.rs` from the last click's cell and a
    400 ms window, because crossterm reports presses, not double clicks.
  - Typing, `Backspace`, `Delete` and paste all replace the selection; deleting one is
    a single rope removal, so it is one undo step when Phase 4 arrives.
  - `editor/clipboard.rs`: `ClipboardProvider`, the internal `Register`, an `Osc52`
    writer over any sink, and a hand-written base64 (fifteen lines against another
    dependency). `Clipboard` writes the register first and the terminal second, so a
    copy the terminal refuses still cuts and pastes inside the editor (ADR-005).
    With `--features native-clipboard`, `arboard` leads and OSC52 stays behind it for
    the SSH case.
  - Bracketed paste inserts in one operation and is remembered as the clipboard's
    contents, so a following `Ctrl+V` repeats it. `\r\n` and lone `\r` in pasted text
    are normalised on the way in, exactly as they are when a file is opened (ADR-009).
  - `Command` gave up `Copy` so `InsertText(String)` can carry a paste through the one
    mutation path rather than around it (ADR-012). The Edit and Selection menus are
    wired to the real commands and now advertise their keys.
  - Selection is painted per *cell*, not per cluster: a selection ending inside a tab
    highlights only the cells it reaches, a wide character is highlighted whole, and a
    line whose newline is selected gets one cell past its text so a multi-line
    selection reads as one block. `theme.editor_selection` sets a background only, so
    Phase 7's syntax colours will survive it.
  - The status bar shows `Sel N` while something is selected, counted without
    materialising the text — `Ctrl+A` on a 5 MB file must not copy it into a counter.
  - 208 tests (was 144).

- **Phase 4 — Undo/redo**
  - `editor/history.rs`: `EditOperation` (`Insert`/`Delete`, each carrying the text
    that inverts it), `Transaction` (the ops of one user action plus the cursor on
    either side of it), and `History` with an undo and a redo stack. No snapshots:
    memory is O(edited bytes), which the module's own test holds to under 64 bytes per
    keystroke over 100 000 of them.
  - Coalescing is the 500 ms window *and* a character-class run — word, whitespace,
    punctuation, newline (ADR-013). `hello world` is three undo steps at any typing
    speed, and each `Enter` is its own. A paste or a selection removal is one step and
    does not absorb the keystroke after it.
  - One user action is one transaction however many rope operations it takes:
    `History::begin_edit`/`end_edit` nest, so typing over a selection — a remove and an
    insert — undoes in a single `Ctrl+Z`. This is what the Phase 3 note about "one rope
    operation per mutation" was for.
  - `Document` records in the two places that touch the rope, so no editing method can
    forget to; `remove` reads the text out before the rope loses it. Undo walks a
    transaction's ops backwards (a later op's offsets are only valid while it is still
    applied), redo walks them forwards, and both put the caret back where the step
    started or ended. The restored cursor is clamped rather than trusted (SPEC §45).
  - A cursor move, a selection change and a save all seal the open step — `step` and
    `set_cursor_at` are the two places every motion passes through, so the rule is two
    lines rather than a dozen.
  - `History` owns the save point (ADR-014): undoing back to what is on disk clears the
    dirty marker, redoing past it sets it again, and a save point on a branch abandoned
    by a new edit is forgotten rather than reused.
  - `Command::Undo`/`Command::Redo` bound to `Ctrl+Z`/`Ctrl+Y` in the editor and wired
    to the Edit menu, which now advertises the keys instead of showing nothing
    (ADR-008). `Ctrl+Shift+Z` is deliberately unbound — a legacy terminal cannot tell it
    from `Ctrl+Z`. An empty stack says "Nothing to undo" rather than appearing to miss.
  - 242 tests (was 208).

- **Phase 5 — Tabs**
  - `app/tabs.rs`: `Tab` moved out of `app/mod.rs` as planned, plus
    `active_after_close`, which is the whole "which tab comes forward" rule in one
    tested function. Closing a tab to the left of the active one shifts the active
    index down, so the file in front of the user does not silently change.
  - Close by `Ctrl+W`, by the `×` on any tab, by middle click, and from File → Close
    Tab — four producers of the same two commands (SPEC §11, §27). A closed tab drops
    its `History` with it; nothing else holds one.
  - The tab bar scrolls. The offset is *derived* on every frame rather than stored
    (ADR-015): the bar starts at the earliest tab that still leaves room for the active
    one, and `‹` / `›` say there are more tabs off each edge. Twelve files open at 100
    columns show six and both work by mouse and keyboard.
  - A tab is ` name ● × `, and the dirty marker keeps its cell whether or not the file
    is modified — typing the first character into a file must not shift every tab after
    it sideways under the pointer. A tab only partly on screen has no `×` to click.
  - The dialog system arrived with its first producer (SPEC §40): `app/dialog.rs` is a
    title, a message and a row of buttons, each button carrying the `Command` that
    choosing it runs. Two constructors so far — the close prompt and the quit prompt —
    rather than two input modes (ADR-016). `FocusTarget::Dialog` finally has a producer
    too.
  - Modality is enforced where input enters: `resolve` consults only the `Dialog`
    bindings while a dialog has focus, so `Ctrl+Q` cannot quit out of the prompt asking
    whether the user meant to, and `hit_test` returns a button index or nothing at all.
  - `Save` on the prompt writes and then closes, and a save that fails leaves the tab
    open with the error on the status bar — the edit the user asked to keep is the one
    they must not lose. `Document::is_dirty` being history-aware (ADR-014) is what keeps
    a file undone back to what is on disk from being asked about at all.
  - `Ctrl+Q` now confirms too when anything is modified, defaulting to Cancel
    (ADR-017). Closing carefully while quitting discarded everything silently was not a
    defensible pair.
  - The CLI takes several files: `ferroedit a.rs b.rs c.rs` (ADR-018). Until the
    explorer lands there is no in-app way to open a second file, so without this the
    phase's own acceptance would have been reachable only from the test suite.
  - 286 tests (was 242).

- **Phase 6 — Explorer**
  - `filesystem/tree.rs`: a real `FileTree` over `ignore`. A directory is read when it
    is first expanded and not before — `children: Option<Vec<Node>>` *is* the laziness —
    so opening a repository costs one `read_dir` of the root. The walk is built per
    directory with `max_depth(1)`, and `require_git(false)` makes a `.gitignore` count
    outside a repository too.
  - What is visible is a flattened `Vec<TreeRow>` rebuilt on every structural change.
    Rendering, the keyboard selection and mouse hit-testing all index into that one
    vector, so the row on screen and the row a command acts on cannot be different
    things. Directories sort first, then case-insensitively by name.
  - `refresh` re-reads every open directory rather than patching the tree at the point
    of a change, and re-expands by path; `reveal` opens whatever it takes to put a path
    on screen and is what selects a file the moment it is created. Both are how a file
    operation and `F5` end in the same state.
  - Hidden and git-ignored files are one switch, off by default, and `.git` is filtered
    by name so it is never a row either way (ADR-021). Without the switch the editor
    could not open its own `.gitignore`.
  - Keyboard (SPEC §18): `Enter` opens a file and folds a directory, `Right` expands and
    then steps *into*, `Left` collapses and then steps *out* to the parent, `F2` renames,
    `Delete` deletes, `F5` refreshes. The selection scrolls its own panel:
    `SidebarState::follow_selection` is the one rule, fed by `App::explorer_rows`, which
    the main loop mirrors out of each frame exactly as it does `editor_view` (ADR-010).
  - Mouse: one click on a row opens the file or folds the directory, focused or not
    (ADR-020). The git panel keeps the older focus-then-select behaviour, because it has
    no row action to reach yet.
  - `app/dialog.rs` grew the second kind of body: `DialogBody::Message` and
    `DialogBody::Input`, the enum of SPEC §40 one level below `DialogState` (ADR-019).
    The field's caret is a `CharIdx` moving by grapheme cluster, so `Backspace` over `é`
    written as `e` + U+0301 removes the whole cluster, and the caret drawn in the box is
    the terminal's own cursor.
  - A dialog that types needs its own keymap — `Left` is a caret there and a button
    selection in a confirmation — so `resolve` takes a `text_input` flag and consults
    `INPUT_BINDINGS`. It is still modal: `Ctrl+Q`, `Ctrl+S` and `Ctrl+W` resolve to
    nothing behind an open prompt.
  - New File, New Folder, Rename and Delete (SPEC §20), as four constructors and one
    `FileOp`. The confirm button carries `SubmitInput(op)` because the name does not
    exist when the dialog is built; activating it is what pairs the operation with the
    text, one line in `activate_dialog_button`. `Ctrl+N` is bound globally, the rest are
    on the File menu and on the explorer's own keys.
  - `filesystem/mod.rs` takes a *name* and joins it itself, so nothing typed into a
    dialog can leave the directory it was typed into: a separator, `.` and `..` are
    rejected before any syscall. Creating uses `create_new`, so "does it exist" is part
    of the same call rather than a race; renaming checks first, because `fs::rename`
    would silently overwrite. Deleting a directory takes its contents, which is what the
    confirmation says out loud.
  - A rename follows the open tabs — the file itself, and every file open from inside a
    renamed directory (`Document::set_path`). Without it, saving after a rename would
    write the old name back and leave two copies on disk.
  - The Phase 1 explorer mock is gone. `mock_git` is the last invented data in the
    sidebar, and Phase 10 is what removes it.
  - 340 tests (was 286).

- **Phase 7 — Syntax highlighting**
  - `build.rs` links the syntax set at compile time and dumps it into `OUT_DIR`; the
    binary loads it back with `from_binary`. Measured: ~105 ms to link ~80 grammars
    against ~1.5 ms to load the dump, which is the whole reason the build script exists
    (ADR-022).
  - `assets/syntaxes/` holds two hand-written grammars — TOML and Dockerfile — because
    syntect's defaults are the Sublime default packages and have neither, and TOML is
    the language FerroEdit's own config is written in. TypeScript, TSX and JSX alias
    onto the JavaScript grammar: a superset colours everything the two share and leaves
    type annotations plain, which is worse than a real grammar and much better than
    nothing.
  - Detection is by file *name* first and extension second, with the first line as the
    last resort. `Cargo.lock` is TOML because its name says so and its extension says
    nothing; a `deploy` with `#!/usr/bin/env python3` is Python. The status bar now
    names the grammar the highlighter actually chose — `Document::language`, the Phase 2
    guess from the extension, is gone, so there is one answer instead of two that could
    disagree.
  - `syntax/highlighter.rs` never touches `syntect::highlighting`: it keeps the
    `ScopeStack` itself and folds it into a 14-value `StyleKind` that `Theme` colours,
    which is what keeps the palette 256-colour indexed and visible in one file
    (ADR-023). The scope table is read in *its* order rather than the stack's, so `//`
    and the comment it opens are one colour and not two.
  - `syntax/cache.rs` is the per-tab cache: the parser's state before every 64th line,
    plus the styled runs of the lines on screen and nothing else. An edit on line N
    drops the checkpoints below N and keeps the ones above it, because nothing that
    happens later can change what the parser had already seen.
  - The cache learns about edits from a watermark on `Document`: every rope mutation —
    insert, remove, undo and redo alike — records the lowest line it touched, and
    `take_dirty_from` is a take rather than a read because the cache is its only
    consumer.
  - A jump into a part of a huge file that has never been drawn restarts from a clean
    state 256 lines above the viewport instead of running 100 000 lines down from line 0
    (ADR-024). Bounded work, at the cost of a construct longer than the look-back.
  - Fallback: a file over 5 MB, or one with a line over 4 KB, is drawn as plain text,
    says so once as a notification, and keeps saying so as `(plain)` on the status bar.
  - `App::sync_highlight` runs in the loop *before* the draw and not inside it, so `ui/`
    stays read-only over `&App` (ARCHITECTURE §1, invariant 4). Only the active tab is
    synced: a background tab is not on screen, and an edit it did not receive cannot
    have invalidated it.
  - The selection sets a background and no foreground, which Phase 3 chose on purpose:
    selected code keeps its own colours instead of flattening into one block.
  - The responsiveness acceptance is asserted as *work* rather than as wall-clock — a
    keystroke never reparses more than the viewport plus one checkpoint interval — which
    is the same claim without the flakiness of timing a test runner under load.
  - 370 tests (was 340).

- **Phase 8 — Search / replace**
  - `editor/search.rs` works one line of `&str` at a time. A query typed into a one-line
    field cannot contain a newline, so no match can straddle a line break — and a hit
    reported as `(line, start..end)` is already in the coordinates the renderer and the
    cursor speak, so nothing has to convert.
  - Two search paths: case-sensitive goes through `str::find`, which skips ahead;
    case-insensitive compares character by character through `char::to_lowercase`, so
    `ПРИВІТ` matches `привіт` without lowercasing the line into a second buffer whose
    indices would no longer line up with the first.
  - The current hit *is* the document's selection (ADR-025), which is what makes
    `Ctrl+C` copy the match, gives Replace something to act on, and makes "scroll the
    match into view" the `follow_cursor` rule Phase 2 already wrote.
  - `Document::replace_matches` applies the hits last to first, so the offsets of the
    ones still ahead of the edit are the ones they were found at. It is one
    `begin_edit`/`end_edit` with a `seal` on either side, which is what makes Replace
    All one undo step and keeps it from merging into the typing before it (SPEC §23).
  - `app/search.rs` holds the bar: two fields, the case flag, the hits and which one is
    current. The hits are a cache refreshed by `App::sync_search` once a frame when the
    query, the option, the tab or the document's `revision` has changed (ADR-026) — the
    same shape as the highlight cache, and next to it in the run loop.
  - `Document` gained a `revision` counter alongside the highlight cache's watermark.
    The watermark is a *take*, so two consumers would starve each other; a counter can
    be compared by any number of readers without consuming anything.
  - `MAX_MATCHES` caps the list at ten thousand and the bar says `1/10000+` when it bit
    (ADR-027). `e` in a five-megabyte file is half a million hits, and Replace All then
    reports what it did and asks to be run again rather than half working in silence.
  - The bar takes its rows out of the editor's, so opening it scrolls the cursor exactly
    as a resize does. Its pieces — both fields, `[Aa]`, `[Replace]`, `[All]` — are rects
    computed in `ui/layout.rs` and drawn from the same numbers, so a click cannot land
    somewhere other than what it looked like it hit. Furniture that would squeeze the
    field below eight columns is dropped instead, which is what keeps the bar usable at
    the 40-column minimum.
  - `InputField` moved out of `app/dialog.rs` into `app/input_field.rs` and its renderer
    into `ui/field.rs`: the bar's two rows want the same cluster-wise caret and the same
    scroll-under-the-caret behaviour the Phase 6 name prompt has.
  - `FocusTarget::Search` is not in the cycle and is not modal: `Ctrl+S` still saves
    while the caret is in the bar, unlike behind a dialog. `Esc` closes the bar from the
    editor as well as from the bar.
  - Alt+C / Alt+R / Alt+A are the bar's own shortcuts, and every one of them is also a
    Search menu item and a clickable label — macOS Terminal.app does not send Alt unless
    the user has turned on "Use Option as Meta", and a feature reachable only by a key
    that terminal eats is not reachable.
  - 426 tests (was 370).

- **Phase 9 — Menus + command wiring**
  - `docs/SHORTCUTS.md` is now *rendered* from `BINDINGS`, `INPUT_BINDINGS` and `MENUS`
    by `src/docs.rs` (ADR-028). `ferroedit --dump-shortcuts` writes it to stdout, and
    `cargo test` fails when the checked-in copy is stale — `FERROEDIT_UPDATE_DOCS=1`
    rewrites it instead of failing. It is the whole file, not a fragment: the prose the
    tables cannot know (typing, the mouse, terminal limits) lives in the generator.
  - `Command::description` is an exhaustive match, so a command added without a line of
    English for it does not compile. That is what makes the document impossible to
    forget rather than merely easy to regenerate.
  - Each table says which focus it is for, which is the scoping the hand-written file
    could not express. The menu table gained a *Where* column, and it immediately paid
    for itself: the `Ctrl`+BackTab row was labelled `Ctrl+Tab` when BackTab *is* the
    shifted Tab (fixed), and the Search menu really does advertise three `Alt` keys that
    only resolve while the find bar has the caret (true, and now written down).
  - **Open…** (`Ctrl+O`) and **Save As…** are wired: both are the input dialog Phase 6
    already had, asking for a path rather than a name (ADR-029). The explorer is the
    file browser; a field is how a path outside the workspace is reached. Open on a path
    that is not there yet starts a buffer for it, exactly as the command line does.
  - Save As moves only the path (`Document::set_path`): the buffer, the history and the
    save point come with it, so an undo afterwards still walks back through edits made
    under the old name. Writing over a path another tab holds is refused rather than
    silently making two histories over one file.
  - `FileOp` therefore carries two operations that are not filesystem changes;
    `apply_file_op` routes them before its create/rename tail, and the match stays
    exhaustive so a third cannot be added without an answer.
  - Help → About is a one-button message dialog. `Unimplemented` is down to five
    entries: the four Git ones (Phase 11) and the help screen (Phase 14), and a test
    names exactly those five, so a sixth cannot appear quietly.
  - A test walks every menu item from every menu and activates it, which is Phase 9's
    acceptance as a test rather than as a paragraph.
  - 444 tests (was 426).

## In progress

- Nothing. Phase 9 is closed.

## Known issues

- **`cross` is unusable on this machine.** Both musl targets fail before the build
  starts: `couldn't install toolchain stable-x86_64-unknown-linux-gnu` (cross 0.2.5 on
  an Apple Silicon host). Worked around locally with a plain `rust:alpine` container;
  CI still uses `cross`, so the `targets` job is the thing to watch on the first push.
- **x86_64 musl not yet built anywhere.** Only `aarch64` musl was verified locally.
  R12 (aarch64 musl cross-compilation) is confirmed; the x86_64 musl link is still
  CI-only.
- **The status bar crowds itself at 60 columns.** The notification is clipped to make
  room for the fixed `Ln/Col … focus` readout. Correct, but a responsive readout
  (dropping the encoding and focus label when narrow) belongs in Phase 14.
- **Mouse capture takes over terminal text selection.** Shift-drag is the escape hatch
  in most terminals. Expected, and unchanged from SPEC §27.
- **OSC52 is write-only, and silently so.** The terminal never says whether it took
  the copy: a terminal with OSC52 disabled (the default in some builds of Terminal.app
  and in tmux without `set -g set-clipboard on`) reports success and puts nothing on
  the clipboard. Pasting inside FerroEdit still works — that is the internal register —
  but there is no way to detect the failure and say so.
- **A copy over 64 KB does not reach the system clipboard.** The OSC52 payload is
  skipped past that size and the status bar says so; the text is still in the internal
  register, so cut and paste inside the editor are unaffected. The limit exists because
  tmux truncates around 74 KB and some terminals stall on a much larger sequence.
- **Double-click uses a 400 ms window over the same cell.** Two deliberate clicks on
  one character inside that window select the word, which is the desired behaviour and
  also the only thing a terminal makes possible — crossterm reports presses, not double
  clicks. A very slow link can stretch a real double-click past the window.
- **A drag that leaves the editor pane does not auto-scroll.** The selection follows to
  the pane's edge and stops there; scrolling the view while dragging needs a timer in
  the event loop, which is Phase 14 polish rather than Phase 3 correctness.
- **Double-clicking CJK selects one ideograph.** UAX #29 puts a word boundary between
  them because Japanese is written without spaces. That is the standard segmentation;
  dictionary-based word breaking is not something to invent here.
- **There is still no Open… and no Save As.** New File, New Folder, Rename and Delete
  arrived in Phase 6, but a file *outside* the workspace can only be reached from the
  command line (`ferroedit ../other/file.rs`). Both need a dialog that browses rather
  than one that takes a name, which is `DialogBody::List` and not yet written; `Ctrl+O`
  stays unbound, so the menu correctly shows no shortcut next to it (ADR-008).
- **Rename pre-fills the name and there is no way to select it.** The field has a caret
  but no selection, so replacing `main.rs` wholesale is seven `Backspace`s. Editing an
  extension or a suffix — the common case — is what the pre-filled name is good at.
- **The tree does not watch the filesystem.** A file created, deleted or renamed by
  something else appears on `F5` (or after any file operation, which refreshes anyway).
  A watcher is a dependency and a thread, and neither belongs in this phase.
- **Deleting a file that is open leaves the tab open.** The buffer keeps its text, the
  tab keeps its name, and `Ctrl+S` writes the file back into existence. That is
  recoverable rather than surprising, but the tab does not say that what it is showing
  is no longer on disk.
- **Expanding a very large directory blocks the frame.** The read and the sort happen on
  the UI thread, so a directory with tens of thousands of entries is a visible pause.
  Everything above it stays lazy, so this is one directory's worth of work rather than
  the project's — moving it to a worker is Phase 14.
- **A symlink to a directory is listed as a file.** The walker reports the link's own
  type and following links is off, so it cannot be expanded. Correct for loop safety,
  wrong for the user who symlinked a directory in on purpose.
- **There is no context menu, no multi-select, and no copy or move.** The file
  operations are the four of SPEC §20, reached from the keyboard and the File menu; a
  right-click menu is not wired to anything.
- **The tab bar cannot be scrolled without switching tabs.** The offset is derived from
  which tab is active (ADR-015), so the `‹` and `›` markers are indicators and not
  buttons. Browsing the tab list without changing the file in front of you is not
  possible; it needs either a stored offset or a tab picker, and neither is Phase 5.
- **Closing tabs one at a time is the only way to close several.** There is no Close
  All, Close Others, or a Save All on the quit prompt — quitting with four dirty files
  is one prompt with one Quit Anyway, so the choice is all or nothing. Per-file answers
  belong with the other multi-file work in Phase 14.
- **A tab's `×` costs two cells on every tab, whether or not it is wanted.** At 60
  columns that is roughly one tab's worth of the bar. The alternative — showing the
  close button only on the active tab, or on hover — makes tabs change width as the
  selection moves, which is worse: the target under the pointer would move as it is
  being aimed at.
- **The tab bar has no middle-click fallback the terminal cannot deliver.** Terminals
  that do not report the middle button simply do not close tabs that way; `Ctrl+W` and
  the `×` are always there, which is why middle click is the third route and not the
  only one (SPEC §27).
- **Undo does not restore the selection, only the caret.** A transaction remembers
  `cursor_before` and `cursor_after`, per ARCHITECTURE §5, and nothing about the anchor.
  Undoing a "type over a selection" step therefore puts the text back and leaves the
  caret where the typing started, without re-selecting what was replaced.
- **A run of emoji coalesces as punctuation.** `char::is_alphanumeric` is false for
  them, so `b👨‍👩‍👧` backspaces in two undo steps rather than one (ADR-013).
- **The undo stack is unbounded.** It is O(edited bytes) and never O(document), which is
  what SPEC §16 asks for, but a session that edits a hundred megabytes holds a hundred
  megabytes. A cap belongs with the other memory work in Phase 14, and dropping the
  oldest steps is a behaviour change worth deciding deliberately.
- **Mixed line endings are normalised to the file's first one on save** — deliberate,
  and recorded as the cost in ADR-009.
- **ZWJ emoji width follows `unicode-width`.** `👨‍👩‍👧` is reported as six cells; some
  terminals draw it as two. Cursor movement is right either way (it is one stop), but
  the column arithmetic after it can be off on those terminals. ADR-004 chose this over
  per-terminal special-casing.

## Manual checks performed

Phase 2 through Phase 8 acceptance were verified by driving the real binary in a pty
(the Phase 1 harness: fork a pty, set `TIOCSWINSZ`, write key and mouse bytes, replay
the output through a minimal terminal emulator).

### Phase 8

- **Find.** `Ctrl+F` in a three-line file, `foo` typed: the bar reads `Find: foo` with
  `1/3 [Aa]`, and the SGR shows the first hit on background 24 (the selection) and the
  other two on 58 (the match background) — the current hit is visibly not the same as
  the rest. `Enter` walks 1/3 → 2/3 → 3/3 → 1/3, and the status bar follows with
  `Ln 3, Col 8   Sel 3`.
- **Match case.** `Alt+C` over `foo` / `FOO` / `foo` drops the count from `1/3` to
  `2/2` and says `Match case: on`. Clicking `[Aa]` with the mouse does the same thing.
- **Unicode.** `привіт` typed against a file of `Привіт` / `Привіт` / `ПРИВІТ` finds
  three hits insensitively and none sensitively — the folding is not ASCII-only.
- **Replace.** `Ctrl+H`, `foo`, `Tab`, `QUUX`, `Alt+A`: all three lines are rewritten,
  the status bar says `Replaced 3 matches`, `Ctrl+Z` puts all three back in **one** step
  and `Ctrl+Y` redoes them. `Ctrl+S` writes `QUUX bar\nbaz QUUX\nqux QUUX\n` to disk.
- **Replace one.** `Alt+R` twice rewrites the first hit and then the second, leaving the
  caret after each — `Ln 1, Col 2` then `Ln 2, Col 6`.
- **The menu drives all of it.** `F10`, `Right`×3 opens Search and shows Find… `Ctrl+F`,
  Replace… `Ctrl+H`, Find Next `F3`, Find Previous `Shift+F3`, Match Case `Alt+C`,
  Replace Match `Alt+R`, Replace All `Alt+A`. Activating Find… opens the bar.
- **`Esc` from the editor closes the bar.** Clicking into the document moves focus to
  `Editor` with the bar still open and the hits still highlighted; `Esc` then dismisses
  it.
- **A 3.3 MB, 120 000-line file.** Typing a query costs a median 38.9 ms a keystroke
  against a 33.8 ms baseline with no bar open, so the search itself is about 5 ms —
  ~20 ms of both numbers is the harness's own floor. `Replace All` over ten thousand
  matches took 560 ms and undid in 730 ms, as one step, with
  `Replaced 10000 matches — more than 10000 were found, run it again` on the status bar.
- **Narrow terminals.** At 100 and 60 columns the bar shows the count and both buttons;
  at the 40-column minimum the count and `[Replace]` drop out so the field keeps eight
  columns, and `Find:`, `[Aa]` and `[All]` remain.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D
  warnings` and `cargo test --all-features` — all clean, 426 tests, zero warnings.

### Phase 7

- **The SPEC §21 languages highlight.** Driven in the pty at 120×40 against a fixture
  of each: `demo.rs`, `demo.py`, `demo.toml`, `demo.md` and a `Dockerfile`. The emitted
  SGR shows the expected indexed colours — 176 keyword, 111 function, 150 string, 173
  number, 243 comment, 180 type, 110 tag — and the status bar names `Rust`, `Python`,
  `TOML`, `Markdown`, `Dockerfile`. The hand-written TOML grammar colours `[package]`
  as a table header and `name = "demo"` as key/operator/string; the Dockerfile grammar
  colours `FROM … AS build` and the trailing `# comment`.
- **A 3.3 MB, 120 000-line Rust file stays responsive.** `Ctrl+End` to the last line,
  then 30 keystrokes: median 30.5 ms, p90 33.9 ms, max 34.6 ms from writing the byte to
  the screen settling — against 22.7 ms for the same measurement on a file with
  highlighting off, and ~20 ms of that is the harness's own floor. Highlighting costs
  roughly 7 ms a keystroke at the end of a file that size.
- **The big-file fallback fires.** A 6.6 MB file opens with `(plain)` on the status bar
  and no colour anywhere; startup is 0.18 s and `Ctrl+End` 0.18 s, the same as the
  highlighted file.
- **Startup is not slower.** 0.17 s to first frame for a small file, unchanged from
  Phase 6 — the dump is loaded once and lazily.
- **The first frame is drawn plain and corrected immediately.** The editor pane's size
  is not known until a frame has been laid out, so the very first sync runs with a
  zero-height viewport; `sync_editor_view` then forces the redraw that colours it. Two
  frames, microseconds apart — the same mechanism that scrolls `+42` into view.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D
  warnings` and `cargo test --all-features` — all clean, 370 tests, zero warnings.

### Phase 6

- **`ferroedit .` on a real project.** The tree shows `src`, `Cargo.toml` and
  `README.md`; `target/` (git-ignored), `.gitignore` and `.git/` are not rows at all.
- **The tree opens lazily and by hand.** `F6`, `Right` expands `src` in place; `Down`,
  `Right` expands `src/ui`; `Down`, `Enter` opens `theme.rs` as a tab, with the status
  bar reading `Opened theme.rs` and the language column `Rust`.
- **One click opens a file.** An SGR click on a row in the sidebar opened `README.md`
  as a second tab from an unfocused explorer — one click, not two.
- **`Ctrl+N` creates, opens and selects.** With `src` selected the prompt reads
  `Create in src`; typing `notes.md` and pressing Enter puts an empty `src/notes.md` on
  disk, opens it as a tab, selects it in the tree and says `Created notes.md`.
- **A name already taken is refused.** The second `notes.md` leaves the first one
  untouched and puts `notes.md already exists` on the status bar in the error colour.
- **`F2` renames.** The field arrives holding `notes.md`; eight `Backspace`s and
  `renamed.md` leave exactly that on disk, with the old name gone, the tab retitled and
  `Renamed to renamed.md` on the status bar.
- **`Delete` asks first.** `Delete renamed.md?` with `[ Cancel ] [ Delete ]` and Cancel
  selected; Enter changes nothing, `Right` then Enter removes the file and the row.
- **`F5` picks up an outside change.** A file written into the workspace behind the
  editor's back appears after `F5`, in sort order, with the tree still expanded.
- **View → Show Hidden Files** brings back `target/` and `.gitignore` and says so;
  toggling again hides them. `.git` is a row in neither state.
- **An empty workspace says `(empty or ignored)`** rather than drawing a blank panel,
  and `Ctrl+Q` still exits 0 from it.
- **Unicode names and scrolling.** Forty files named `файл-NN-日本語.txt` lay out at
  their display width; twenty-five `Down`s scroll the panel with the selection, and a
  click then lands on the row that is drawn there.
- **FerroEdit's own repository** opens in 0.6 s with `target/` never walked.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
  (with and without `native-clipboard`) — all clean, 340 tests, zero warnings.

### Phase 5

- **Twelve files open and the bar scrolls to the active one.** `ferroedit f0.txt …
  f11.txt` at 100 columns shows `f0 … f5` and a `›`; eleven `Ctrl+PageDown` later the
  bar reads `‹ f7 f8 f9 f10 f11` with no `›`. The active tab is on screen at every
  step.
- **`Ctrl+W` on a clean tab closes it** — the bar goes from four tabs to three and the
  status bar says `Closed f0.txt`.
- **`Ctrl+W` on a modified tab asks.** Typing `X` paints the tab's `●`; `Ctrl+W` puts
  `f0.txt has unsaved changes.` and `[ Save ] [ Don't Save ] [ Cancel ]` on screen, the
  focus readout reads `Dialog`, and all four tabs are still open.
- **Each answer does what it says.** Enter (Save) leaves `Xfile 0 line one` on disk and
  closes the tab; `Right` then Enter (Don't Save) closes it with the file untouched;
  `Esc` leaves the tab open and dirty and puts focus back on the editor.
- **The prompt is modal.** `Ctrl+Q` with the prompt open changes nothing — the box is
  still there and the process is still running.
- **The mouse closes tabs two ways.** An SGR click on the second tab's `×` closes
  `f1.txt`; a middle click on `f2.txt` closes it. Both report `Closed …`.
- **`Ctrl+Q` confirms when something is modified.** With `X` typed it shows `1 file has
  unsaved changes.` / `[ Cancel ] [ Quit Anyway ]` and keeps running; Enter (Cancel)
  keeps running; `Right` then Enter exits 0. With nothing modified it exits 0 straight
  away.
- **The File menu advertises the key it now has**: `Close Tab  Ctrl+W`, read out of the
  keymap table (ADR-008).
- **The prompt fits the acceptance size.** At 60x20 the box is 38 columns wide, centred,
  and inside the frame; the render sweep covers 40x8 through 200x60 with the widest
  message open.
- **Unicode tab names.** A tab named `файл-日本語.txt` is laid out at its display width,
  not its character count.
- **Terminal restore after panic** re-checked: `--panic-test` still exits 101 with the
  alternate screen left first.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
  (with and without `native-clipboard`) — all clean, 286 tests, zero warnings.

### Phase 4

- **A typed word undoes as a word.** ` hello world` typed at the end of `start`, then
  `Ctrl+Z` twice and `Ctrl+S`, leaves `start hello` on disk — the second `Ctrl+Z` took
  the space, not a letter. `Ctrl+Y` twice after that restores `start hello world`.
- **The caret comes back with the text.** `Down`, `Down`, `End`, `!!!`, `Ctrl+Z` on a
  three-line file goes from `Col 9` back to `Col 6`, which is where the typing started.
- **Replacing a selection undoes in one step.** `Ctrl+A`, `X`, `Ctrl+Z` over
  `one two three` restores the whole line.
- **A bracketed paste undoes in one step.** `ESC[200~a\r\nb\r\nc ESC[201~` then
  `Ctrl+Z` and `Ctrl+S` leaves the file exactly as it was opened.
- **The dirty marker follows the history.** Typing `!` paints the tab's `●`; `Ctrl+Z`
  repaints the cell without it; `Ctrl+Y` brings it back; `Ctrl+S` clears it.
- **An empty history says so.** `Ctrl+Z` in a freshly opened file puts `Nothing to undo`
  on the status bar rather than doing nothing.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
  (with and without `native-clipboard`) — all clean, 242 tests, zero warnings.

### Phase 3

- **Shift+Right five times then `Ctrl+C`** on `fn main() {` reads `Sel 5` on the status
  bar, says `Copied 5 characters`, and puts `\x1b]52;c;Zm4gbWE=\x07` on the terminal —
  base64 of `fn ma`, which is exactly what was highlighted.
- **`Ctrl+A`, one keystroke, `Ctrl+S`** over a three-line file leaves `X` on disk: the
  selection was replaced, not pushed aside.
- **Cut and paste across lines.** `Shift+Down` twice, `Ctrl+X`, `PageDown`, `End`,
  `Ctrl+V`, `Ctrl+S` turns `one\ntwo\nthree\n` into `three\none\ntwo\n`.
- **Bracketed paste.** `ESC[200~alpha\r\nbeta\r\nESC[201~` at the end of `start`
  writes `startalpha\nbeta\n` — one insert, and no carriage returns in the buffer.
  A following `Ctrl+V` repeats the same text, so the terminal's paste became the
  clipboard's contents.
- **Mouse drag** from the `m` of `main` down to the second line copies
  `main() {\n    prin` and reads `Sel 17`; a drag that runs off the pane keeps
  selecting to its edge.
- **Double-click** on `main` (both presses in one write, so they land inside the 400 ms
  window) copies exactly `main`.
- **Unicode.** `Right`, `Shift+Right`, `Ctrl+C`, `Ctrl+X` over `a👨‍👩‍👧b` copies the whole
  ZWJ sequence and leaves `ab` on disk — one cluster, one selection step.
- **A 2 000-line file.** `Ctrl+A`, `Ctrl+C` reports `Sel 140890` and
  `Copied to the internal clipboard only: 140890 bytes is too much for the terminal
  clipboard`; no OSC52 sequence is emitted, and pasting the whole thing back at the end
  of the file still works.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test` (both with
  and without `native-clipboard`) — all clean, 208 tests, zero warnings.

### Phase 2

- **`ferroedit test.txt` edits and saves.** Typed at the start of line 1, moved to the
  end of line 2 with `Down`/`End`, typed again, `Ctrl+S`, `Ctrl+Q`: exit 0 and the file
  on disk is `!Hello\nПривіт!\n日本語\n`. The status bar read `Saved test.txt` and
  `Ln 2, Col 8` — column counted in graphemes, so `Привіт!` is seven of them.
- **CRLF round-trip.** A `\r\n` file edited and saved comes back as
  `one!\r\ntwo\r\nthree\r\n`, and the status bar showed `CRLF` while it was open.
- **A file with no trailing newline** keeps not having one after a save.
- **Creating a file.** `ferroedit brand-new.txt` starts with `New file … — Ctrl+S to
  create it` and creates nothing on disk; typing `Привіт⏎світ` and pressing `Ctrl+S`
  writes exactly that, and the next run of the same command says `Opened …`.
- **`ferroedit +42 lines.txt`** opens with line 42 on screen (viewport top at line 40)
  and `Ln 42, Col 1`. This is the case the first-frame scroll sync exists for.
- **Unicode fixture.** One `Backspace` at the end of the `👨‍👩‍👧` line removes the whole
  ZWJ sequence; the file after saving differs from the original by exactly that line.
- **Mouse.** An SGR click in the editor moved the cursor to the line and column under
  the pointer (`Ln 3, Col 6`); the wheel still scrolls without moving the cursor.
- **Horizontal scrolling.** A 200-column line with the cursor at `End` shows the tail
  of the line and `Ln 1, Col 201`.
- **A read-only file** reports `Failed to save: …: Permission denied` on the status bar
  in the error colour, and the editor keeps running.
- **5.2 MB file** (52 000 lines): open, `Ctrl+End`, type, save and quit took ~1 s in a
  debug build, most of it the harness's own settle delays. No `to_string()` of the
  buffer is on the save or render path (SPEC §44).
- **Terminal restore after quit and after panic** — unchanged from Phase 1 and
  re-checked: the teardown sequence is emitted in reverse order of setup, and
  `--panic-test` still exits 101 with the restore ahead of the payload.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D
  warnings`, `cargo test --all-features` — all clean, zero warnings.

Still not checked by hand in the real target terminals (iTerm2, Ghostty, Terminal.app,
tmux, plain ssh). The pty harness is a stand-in, not a substitute — particularly for
the terminal cursor's appearance, for wide-character and ZWJ rendering, and for mouse
reporting.

## Next

- Phase 10: git status. `GitService`, repo detection, the `--porcelain=v2 -z --branch`
  parser with fixtures, and the sidebar drawn from it. `mock_git` in `app/mod.rs` is the
  last invented data on screen and is what this phase removes.
- The four Git menu entries are still `Unimplemented`; they are Phase 11's actions, not
  Phase 10's status, so the list stays at five until then.
- The help screen behind Help → Shortcuts is Phase 14. It has an obvious source now —
  `docs::shortcuts_markdown()` is the same text, and a scrollable pane over it would be
  the fifth entry closed.
- The dialog's third body is still a *list*, and Phase 12's branch picker is what will
  settle ADR-019. Open… did not need one (ADR-029): it has a file tree behind it, and a
  branch picker will not.
- Phase 9 was verified by the test suite and by `--dump-shortcuts`, not in the pty
  harness: nothing it added draws a new kind of frame — Open, Save As and About are the
  Phase 6 dialog with different text in it. The dialogs' *behaviour* under a real
  terminal is therefore still only as checked as Phase 6 left it.
- Still not checked by hand in the real target terminals (iTerm2, Ghostty, Terminal.app,
  tmux, plain ssh). Unchanged from every phase so far.
- The CI `targets` job has still not been seen green — x86_64 musl has not been linked
  anywhere.
