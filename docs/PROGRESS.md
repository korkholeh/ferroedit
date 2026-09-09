# Current Phase

Phase 14 — Polish (in progress: help screen, status bar, watcher, undo budget,
external reload, job cancellation, the combined-diff parser, the error-message pass,
the per-file quit walk, the Open browser and green CI done)

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

- **Phase 10 — Git status**
  - `git/models.rs`, `git/parser.rs`, `git/service.rs`: the shapes, the parser and the
    subprocess, in that order and each testable without the next. `mock_git` is gone —
    nothing on screen is invented data any more.
  - `GitService::discover` runs `git rev-parse --show-toplevel`, so opening a
    subdirectory of a project still lists the whole repository's paths. Every failure of
    it is reported as "not a repository": that is what it means in all but pathological
    cases, and the real message is logged.
  - Every invocation goes through one `run`, with `GIT_TERMINAL_PROMPT=0` (a credential
    prompt fails loudly instead of hanging on a terminal the TUI owns),
    `GIT_OPTIONAL_LOCKS=0` (reading status does not fight the git in the next window),
    `-c core.pager=cat`, a closed stdin, and a ten-second kill timer. Both pipes are
    drained on threads of their own, so a status too large for the pipe buffer cannot
    deadlock against the wait (ADR-030).
  - Arguments are passed as arguments, never as a shell string (SPEC §29), so a file
    called `; rm -rf ~` is a file name.
  - The parser reads `--porcelain=v2 --branch -z` as **bytes** (ADR-032): the branch
    headers, the four record types (`1` ordinary, `2` rename/copy with its original path
    in a second field, `u` unmerged, `?` untracked), `# branch.ab` for ahead/behind, and
    `(initial)` and `(detached)` as the states they are rather than as names. Ignored
    files and future record types are skipped; a record whose *shape* is broken is an
    error rather than a guess.
  - The panel draws git's own `XY` pair — index column, worktree column — with the file
    count in the title next to the branch and its `↑`/`↓` (ADR-031). Paths are elided
    from the left, so the file name survives a sidebar sixteen cells wide. A directory
    that is no repository says `Not a Git repository`, wrapped rather than clipped, in
    SPEC §28's own words; a clean tree says so; a failed command puts its own first line
    there.
  - The status refreshes itself after a save, a create, a rename, a delete and a Save
    As, and `F5` in the panel (or Git → Refresh) re-runs discovery as well, so a
    `git init` in another window does not need a restart. Outside a repository the
    refresh is not even a subprocess.
  - The git panel keeps a selection and a scroll of its own now that the list can be
    longer than the four to ten rows the layout gives it; `git_rows` is mirrored out of
    the frame next to `explorer_rows` (ADR-010).
  - Phase 10's acceptance as a test: a repository is built with a modified, a deleted, a
    staged, a renamed, an untracked and a conflicted file, and the panel's codes are
    asserted equal to what `git status --short` prints for the same tree. `TestRepo`
    isolates every fixture repository from the machine's global and system config, so a
    developer who signs commits gets what CI gets.
  - 489 tests (was 444), including the pty check below.

- **Phase 11 — Git actions**
  - `git/worker.rs`: one `std::thread`, one `mpsc` queue in, and the main loop's own
    `AppEvent` channel out (ADR-033). `GitJob` is what goes in, `JobOutcome` — a
    `JobId`, the job, and `Result<String, String>` — is what comes back, and the loop
    turns it into `Command::GitJobFinished` so a background result mutates `App` through
    the same door as a key press. The error is flattened to a string at the worker's
    edge because a `Command` has to be `Clone` and `Eq`, and nothing downstream ever
    matched on the variant.
  - `AppEvent` has its first non-terminal variant. Jobs run serially in submission
    order: they take the index lock, and a commit queued behind the staging it depends
    on has to see that staging finish.
  - `GitService` gained `stage`, `unstage`, `stage_all`, `unstage_all`, `commit`, `pull`
    and `push`. `run` now takes `OsString` arguments, so a pathspec goes to git as the
    bytes the status printed rather than as a lossy transcription of them — ADR-032's
    promise, finally exercised end to end.
  - Unstaging is `git reset -q --`, not `git restore --staged`: the latter resolves
    `HEAD` and so fails outright in a repository with no commits yet, which is exactly
    where a file staged by mistake is most likely.
  - `GIT_EDITOR=true` joins the env every invocation gets. A commit that opened
    `core.editor` would be a second full-screen program on the terminal the TUI is
    drawing to. Network commands get a two-minute timer instead of the local ten
    seconds; killing an honest push at ten would be worse than the pause the worker
    exists to prevent.
  - The panel has keys now: `Space` stages the selected row and unstages it again, `a`
    and `u` do it for everything, `c` opens the commit dialog, `Enter` opens the file.
    Pull and Push are deliberately menu-only — a single letter is not the gesture for
    the two operations that change what other people see.
  - The commit dialog is the existing input dialog with a different submission:
    `DialogState::with_input` takes the `Command` the confirm button carries, so
    `SubmitCommit` is paired with the typed message exactly as `SubmitInput` is paired
    with a file name. The prompt says how many files the commit will include, because
    the panel behind it is covered. An empty message and an empty commit are both
    refused before git sees them.
  - `Pushing…` goes in the panel title as well as on the status bar (SPEC §34): a
    notification expires after four seconds and a push does not, so the title is the one
    place that can say "still running" for as long as it is true.
  - A failure reads `Push failed: No configured push destination.` — the job's own noun
    plus git's reason, with git's `git push failed:` prefix dropped so it is not said
    twice and so an unstage does not report itself as `git reset`.
  - Conflicted files are refused rather than staged (ADR-034), and `pull` is
    `--ff-only`: both are Phase 12's, and asserting a resolution on the user's behalf is
    not something an editor with no diff view should do.
  - The Git menu is fully wired — Refresh, Stage, Unstage, Stage All, Unstage All,
    Commit…, Pull, Push — and `Unimplemented` is down to one entry, the Phase 14 help
    screen, with the test that names it updated to match.
  - The acceptance as tests: a push against a real bare remote on disk clears the panel's
    ahead count, a push with no remote comes back as an actionable sentence, and the
    command that starts a job is asserted to return while `git.busy()` is still `Some`.
  - 522 tests (was 489), including the pty checks below.

- **Phase 12 — Branches and merge**
  - `DialogBody::List` (ADR-035) — the variant ADR-019 left open and ADR-029 deferred,
    with the rule they set: a list body earns its place only when no pane behind the
    dialog already lists the same things better. A branch list is the first thing that
    passes, and there is not going to be a branch pane in a sidebar that already holds a
    tree and a status.
  - The picker needs no new input mode. Its confirm button carries `SubmitListChoice`,
    which `activate_dialog_button` replaces with the highlighted row's own command —
    the same substitution that pairs `SubmitInput` with a name and `SubmitCommit` with a
    message. `Up` and `Down` are the list's axis, bound in the dialog table; a body that
    is not a list ignores them.
  - The box's height is computed from the body now (`body_height`) rather than being a
    constant, so a picker is as tall as its list while a message and an input stay at the
    five rows they always were. The button row is placed from the bottom border.
  - The selection starts on the branch `HEAD` is already on, so opening the picker and
    pressing Enter without reading is a no-op and not a checkout. A click selects a row
    without choosing it — the opposite of the explorer's rule (ADR-020), for the reason
    ADR-020 gave: a click that acts is worth it when the action is cheap, and a checkout
    is not.
  - `GitService` gained `branches`, `switch_to`, `create_branch` and `merge`. One
    `for-each-ref` with our own format rather than `git branch -a`, so nothing has to be
    recovered from a display form that varies with the terminal and with `color.branch`.
    `refs/remotes/origin/HEAD` is skipped: it is a symbolic ref to a branch already in
    the list.
  - Remote-tracking branches are offered and switched to by their *short* name:
    `git switch origin/topic` would detach `HEAD`, while `git switch topic` creates the
    local branch that clicking that row means. `switch` and not `checkout`, so a branch
    name that is also a path cannot be read as a request to discard that file's changes.
  - A merge that conflicts comes back as `GitError::Conflicted` and not as a failed
    command (ADR-036): git wrote its complaint to *stdout*, exited non-zero, and left the
    tree in exactly the state the panel exists to show.
  - A stopped merge is read from `MERGE_HEAD` — a `stat` on a path `discover` already
    learned, not another subprocess — and shown as `[merging]` in the panel title. It is
    deliberately not `conflicts() > 0`: once every conflicted file is staged there are no
    conflicts left and the merge is still uncommitted, which is the moment people get
    lost.
  - ADR-034's two refusals are paid off. A conflicted file can be staged, because that is
    how git is told a conflict is resolved and there is now a merge in the editor that
    needs saying so; a file that still holds a `<<<<<<< ` line asks first. `git pull`
    loses `--ff-only` and gains no rebase flag of its own, so `pull.rebase` decides.
  - `first_line` now prefers a line git marked `fatal:` or `error:` over the first one. A
    failed `git pull` writes the fetch it managed, then a dozen hints, and only then says
    what went wrong — the old rule would have shown the user `From /srv/repo`.
  - Committing is refused while anything is conflicted and allowed during a merge even
    with nothing newly staged, because a resolved merge still needs its commit.
  - Panel keys: `b` opens the branch picker, `m` the merge picker. The Git menu gained
    Branch…, New Branch… and Merge…
  - 556 tests (was 522), including the pty checks below.

- **Phase 13 — Diff viewer**
  - `GitService::diff` runs `git diff` (or `git diff --cached`) for one path, with
    `--no-color` and `--no-ext-diff` passed rather than inherited: a user with
    `color.ui = always` would otherwise get escape sequences drawn as text, and
    `diff.external` is somebody else's program writing somebody else's format.
  - `git/diff.rs` classifies each line once, when it is read — header, hunk, addition,
    removal, context, meta — so `ui/diff.rs` reads colours out of the state the way
    every other renderer does (ARCHITECTURE invariant 4). `--- a/x` and `+++ b/x` are
    headers and not a change, which is the one classification order that matters.
  - The viewer is a pane over the editor, not a split and not a dialog: a diff wants
    the width, and the editor behind it is not being typed into while it is up. It
    closes as soon as another pane takes focus, enforced in one place at the end of
    `execute_command` (ADR-037).
  - Which file: the git panel's selected row while the panel has focus, and the file
    being edited anywhere else — the Git menu's Diff, pressed mid-edit, means "this
    one". Which side: what has not been staged when there is any, and what has been
    when there is not, with `s` to see the other one and the title always saying which
    is on screen.
  - An untracked file says why it has no diff instead of opening empty, and a diff
    that has nothing in it never opens a pane at all.
  - The viewer follows the file it is showing: every `git status` refresh re-reads it,
    so staging what is on screen closes it rather than leaving a change that is no
    longer unstaged. `F5` is the manual form.
  - A pager's keys — arrows, `PageUp`/`PageDown`, `Space`, `Home`/`End`, `Left`/`Right`
    by eight columns for lines wider than the pane, `Esc` to close — and the wheel over
    it scrolls it. Nothing types: an unbound letter in a read-only pane does nothing.
  - Long lines scroll sideways rather than wrapping, with tabs expanded and wide
    characters split by an edge drawn as the cells they occupy — the editor's own rule,
    because a diff whose `+` and `−` stopped lining up is a diff nobody can read.
  - `MAX_LINES` caps a diff at 5000 lines with `(cut)` in the title, the ADR-027
    reason: a generated file's rewrite is a diff nobody reads and megabytes nobody
    asked to allocate.
  - 599 tests (was 556), including the pty checks below.

- **Phase 14 — Polish** (in progress)
  - **The help screen** (SPEC §6). `F1` and Help ▸ Shortcuts open a read-only pager over
    `docs::sections()` — the same table `docs/SHORTCUTS.md` is generated from, so a key
    on screen, a key in the file and a key that is bound are one row of one keymap
    (ADR-038). It answers the last menu entry that did nothing.
  - It covers the whole body, sidebar and tab bar included, rather than only the editor
    the diff viewer covers: a key table wants width, and at 60 columns the editor pane is
    44 of them. The menu bar stays above it, which is how it is dismissed with the mouse
    alone. Its keys are the diff viewer's — arrows, `PageUp`/`PageDown`, `Space`,
    `Home`/`End`, `Esc`, and `q` because nothing here types — and the wheel over it
    scrolls it.
  - It dies with its focus, by the same one check at the end of `execute_command` that
    ADR-037 wrote for the viewer, now covering both. A menu or a dialog drawn over it
    leaves it open and hands focus back.
  - The lines are laid out against the pane's width on demand rather than stored, so the
    notes re-wrap on a resize instead of stranding the scroll against a layout that no
    longer exists. A key label wider than the column keeps its own gap rather than being
    cut — a truncated key is a key nobody can press.
  - **`Command::Unimplemented` is deleted.** Every menu entry now resolves to a real
    command, so the placeholder would only preserve the ability to add a new dead one; a
    type that cannot express the case is a stronger statement than the test that used to
    name the exceptions.
  - **The status bar gives way on a narrow terminal** (ADR-039), closing the Phase 3
    known issue. The right-hand readout is a list of pieces with a drop order rather than
    one `format!`: past a floor of eighteen columns for the left half, the encoding goes
    first, then the focus label, the language, the branch, the line ending and the
    selection count. `Ln 1, Col 1` is never dropped. At 60 columns a notification is no
    longer clipped to make room for text that is the same on every file.
  - **A filesystem watcher** (ADR-040), the largest thing the phase owed. `notify`
    watches the workspace root on a thread of its own and reports into the channel the
    input thread and the git worker already write to, so a `git checkout` in another
    terminal reaches `App` through the same door as a key press.
  - The filter is the feature, not the watch. Everything git ignores is dropped, and
    everything under `.git` except the paths that decide what the panel shows — matched
    on the first component, so `refs/heads/topic` is one entry, and `index.lock` reports
    as `index` because the rename over the real file is the event that matters. A
    coalescing window then turns what is left into one refresh, with a ceiling so a
    writer that never goes quiet still gets one. Only an event that survives the filter
    extends the window.
  - The change says whether the worktree moved or only the repository did: staging in
    another terminal re-reads the status without rebuilding the explorer's rows. The
    refresh is silent, because the status bar is where the answers to the user's own
    commands go.
  - Best effort by construction: inotify watches are a per-user resource, so a watcher
    that will not start is a warning and an editor that behaves exactly as it did before.
  - **Every unfinished git operation is named** (ADR-041), not only a merge.
    `RepoStatus::merging` becomes `operation: Option<Operation>` over Merge, Rebase,
    CherryPick and Revert, read from the four paths git records one at. A rebase is
    recognised by its state *directory* rather than by `REBASE_HEAD`, which is what git's
    own status does. Only a merge is finished by a commit; for the other three the commit
    dialog does not open and the status bar names the `git … --continue` that does.
  - **A byte budget for the undo stack** (ADR-042). O(edited bytes) was still unbounded
    over a long session, so the stack carries a running weight — text plus each step's own
    place on it — and drops its oldest steps past sixteen megabytes. The newest step is
    exempt however large it is, the save point moves down by what went and is discarded
    when it was one of them, and the running figure is asserted against a full recount.
  - A long notification now takes the width it needs from the readout rather than only
    what a constant floor left it: `Nothing staged — finish the rebase with …` lost three
    characters at 110 columns before this.
  - **A buffer follows its file when it can, and asks when it cannot** (ADR-043) — the
    gap ADR-040 made visible and did not close. Every tab remembers what its file was —
    mtime and length — the last time the two agreed, and a worktree change stats them all
    against it.
  - A clean buffer is re-read where it stands and says nothing: everything in it is also
    on disk, so a prompt would be a keystroke charged for nothing. A modified one is
    marked and left alone, because it is the only copy of that work.
  - The question — Keep Mine, or Reload — is asked once per change and only for the tab
    on screen; a background tab keeps its mark in the tab bar and is asked when it comes
    forward. Keep Mine is the default: this dialog is opened by a filesystem event rather
    than by the user, so it can arrive between two keystrokes.
  - The reload is one undo step, so `Ctrl+Z` gives the unsaved work back — the byte
    budget of ADR-042 is what keeps holding both versions bounded. A save answers the
    question by overwriting whatever the file became, and settles the tab the same way.
  - A file deleted under a clean buffer is not closed and not emptied: the buffer is the
    last copy, so it is said once and marked, and saving it puts the file back.
  - `F5` in the editor is the same reload asked for deliberately, which is what "show me
    what is really there" already means in the explorer, the git panel and the diff
    viewer. It is also the whole feature's manual door on a machine where the watcher
    would not start.
  - **`ferroedit a.txt` opened a workspace called `/`.** `Path::parent` of a bare file
    name is the *empty* path rather than `None`, so the fallback next to it never fired
    and the root canonicalised to nothing: no tree, no repository, and nothing for the
    watcher to watch. Found by the first smoke test of the reload, which is exactly the
    case a pty harness reaches and a unit test with `tempdir` paths never does.
  - **A running git job can be stopped** (ADR-044), closing the Phase 11 known issue.
    `Esc` in the git panel, and Git ▸ Cancel, set a watermark on the job ids: everything
    outstanding at that moment is cancelled, and anything submitted after it is not.
  - The job in git's hands has its subprocess killed on the same 2 ms poll that already
    watches the timeout, so a `git push` blocked on a socket stops in milliseconds
    instead of at the two-minute timer. The jobs queued behind it are answered without
    being run, which is the other half of the value: stopping a stuck push is worth
    little if the three things behind it happen anyway, minutes later.
  - Every cancelled job still comes back through the same door as a finished one, so the
    panel's queue is only ever shortened by an answer. `JobOutcome` now carries a typed
    `JobFailure` rather than a string, because `Push failed: cancelled` would be the
    editor blaming git for doing as it was told; it reads `Push cancelled`, in the
    information colour.
  - **A conflicted file's diff is read with both of its marker columns** (ADR-045),
    closing the known issue ADR-036 left behind. `git diff` of an unmerged path is a
    *combined* diff — one column per parent of the stopped merge — so the two sides of
    the conflict are printed ` +ours` and `+ theirs`, and a one-column reader called the
    first of those context. The line a user opens the viewer to look at was the one line
    it declined to colour.
  - The classifier is now a small state machine per file: the column count comes out of
    the hunk header (`@@@ … @@@` is two, and an octopus adds a `@` per parent), and a
    body line is an addition if *any* of its columns holds a `+`. It also keeps whether a
    hunk has begun, which is the only thing that separates the file header `--- a/file`
    from a line removed from both parents whose own text starts `- ` — the same bytes, in
    different places.
  - The title says `[worktree, merge]` when what is on screen has more than one parent,
    because the columns do not announce themselves. `git diff --cached` of a conflicted
    file answers `* Unmerged path f.txt`, which is now drawn dim as git talking about the
    file rather than as a line of it.
  - 681 tests (was 674), including the pty check below.
  - **The error-message pass** (ADR-046), read as a pass rather than one feature at a
    time. Every message the editor can put on the status bar was listed side by side,
    which is the only way the split showed: `Nothing to undo` was `Info` and `No file to
    save` was `Warning`; `Nothing selected in the Git panel` was `Info` and `Select
    something in the explorer first` was `Warning` — the same situation in the two
    panels, in two colours and two voices.
  - The kinds now answer one mechanical question. `Info` — it was done. `Warning` —
    nothing was done, and the reason is the state the editor is in. `Error` — it was
    attempted and something outside the editor refused. Seventeen sites moved; the rule
    is the one the existing `Warning` sites already followed, so nothing that read
    correctly changed colour.
  - **`Diff failed: git diff failed: fatal: …`** said it twice and named a subprocess the
    user never typed. `worker.rs` had solved this for the background path and written
    down why; `GitError::reason` moves that knowledge onto the error so both paths share
    it. The three "could not open" sites and the delete site got the verb `Failed to
    save:` and `Failed to reload:` already had.
  - `GitState::start` returns a `NotStarted` with two variants instead of one string, so
    "there is no repository" can be a warning and "the worker is gone" an error without
    the caller matching on a message (SPEC §45).
  - 685 tests (was 681). Ten declined commands are asserted to be warnings in one test,
    so the rule is checkable rather than a paragraph — which is what the fifteen-way
    split came from not having.
  - **Quitting asks about each unsaved file in turn** (ADR-047), closing the Phase 5
    known issue. The old prompt could express two answers — lose everything, or nothing —
    and the one users want most was not among them: saving four modified files on the way
    out meant cancelling the quit, pressing `Ctrl+S` in each tab, and quitting again.
  - It is not a new dialog. A quit *is* closing every dirty tab and then exiting, so the
    question is the one `Ctrl+W` already asks — `a.txt has unsaved changes.` over
    `[ Save ] [ Don't Save ] [ Cancel ]` — once per tab, in tab order, with the count in
    the title: `Unsaved changes (3 left)`.
  - Save is the default again, which ADR-017 could not afford: its Enter discarded every
    dirty buffer. Here Enter saves this file and asks about the next, so holding it down
    saves everything and quits.
  - The count is in the title and not the message because `alpha.txt has unsaved changes
    (2 left).` is what pushes the box past a 40-column terminal for an ordinary file
    name. A fourth button — `Discard All` — is fifty columns of button row and clips on
    the same forty, so discarding four files costs four answers where `Quit Anyway` cost
    one. That is the trade, and a safe Enter is worth more than a short one.
  - A failed save stops the walk where it is: the file is still only in the buffer, and
    quitting past `Permission denied` is the worst thing the editor could do.
    `Command::QuitDiscarding` is deleted — nothing produced it any more.
  - 689 tests (was 685).
  - **CI had never been green, and both reasons were ours** (ADR-048). The first push
    that reached the `targets` job showed them.
  - **Eleven Linux tests failed on `empty ident name (for <runner@…>)`.** `TestRepo`
    passed an identity as environment variables on the commands *it* ran; `GitService` is
    the other thing running git in those tests and it passes the editor's own environment
    and deliberately no identity, because SPEC §32 says the user's configuration is the
    one that applies. So it resolved an identity from the machine — a full name from the
    password file on any developer's account, and nothing at all on a runner. The
    identity now goes into each fixture repository's `.git/config`, which is what both
    callers read and what a real repository already has.
  - A test asserts the exact name a `GitService` commit lands with rather than that it
    has one: the fixture quietly falling back to the developer's own name is what made
    this invisible until CI.
  - **x86_64 musl links, and always did.** The `Assert the Linux binary is static` step
    matched `statically linked` and current Rust with musl produces `static-pie linked` —
    both static. The check accepts both now and looks for an ELF interpreter separately,
    so a wording change alone cannot pass a dynamic binary. The known issue "x86_64 musl
    not yet built anywhere" is answered: `file` on the runner's artefact reads
    `ELF 64-bit LSB pie executable, x86-64, static-pie linked, stripped`.
  - **CI is green, for the first time in the project's history.** Run 34125005795 on
    `fe6af84`: `fmt + clippy + test` on ubuntu and macos, and all four `link` jobs —
    `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`,
    `aarch64-apple-darwin`. Every target the release owes now links on a machine that is
    not this one, and both Linux binaries are asserted static there.
  - 690 tests (was 689).
  - **`v0.1.0` is published**, with `.tar.gz` and `.sha256` for all four targets, on the
    first run `release.yml` ever had. Both Linux binaries were asserted static twice —
    once in CI on the commit, once here on the file that actually ships. No workflow
    artifact was produced by any of it, which is the point of ADR-049.
  - **Open is a file browser** (ADR-051), replacing the text field ADR-029 chose. The
    dialog opens at the workspace root and shows a `Filter:` field and the directory's
    rows — `..`, then directories, then files — in a framed pane with a scrollbar. Open
    walks into a directory or opens a file; Open Folder makes a directory the workspace,
    which is the first time the root has been able to move at all. A click on the row
    already selected opens it, and the wheel moves through them, so browsing costs one
    click a step.
  - The rows are a pane and not three lines of a prompt: twenty of them where the branch
    picker gets ten, clamped to the terminal, with the scrollbar drawn only when there is
    something to scroll. The window they scroll within comes out of the last drawn frame
    (`App::dialog_rows`) rather than from a constant, the way `explorer_rows` already
    did — a constant scrolls against a window a short terminal does not have.
  - The filter is the old path field, not a second one: text that names a real path wins
    over the selection, and text that matches nothing is read as the path of a file that
    does not exist yet, so `Ctrl+O` + a pasted path still works exactly as before.
    Typing aims the selection at the first row that is not `..`, without which Enter
    after typing a name would have walked *up* a directory.
  - Opening a file leaves the sidebar on the folder it is in: revealed in the tree when
    it is already inside the workspace, and the workspace moves when it is not — the same
    rule `ferroedit path/to/file` has followed since Phase 1.
  - A moving root meant the watcher had to be stoppable. `watcher::spawn` now returns a
    `Watch` that owns the `notify` watcher; dropping it closes the channel the thread is
    blocked on. `App` holds that handle and a clone of the loop's sender, so switching
    folders ends one watch and starts one — not two threads reporting on two trees.
  - 712 tests (was 690).
  - **Every scrolling pane has a scrollbar** (ADR-052): the explorer, the git panel and
    the editor, drawn by one shared `ui::scrollbar::render` and only when the pane has
    more rows than it can show. The two sidebar panels put theirs on the border they
    already draw, so a long tree costs no width; the editor has no border to borrow, so
    the layout reserves it a column of its own (`LayoutRects::editor_scrollbar`) and
    keeps it reserved whether or not a bar is in it — a column that appears the moment a
    file outgrows the window would reflow every line on screen while the user is typing.
    `MIN_EDITOR_WIDTH` now means twenty columns of text with the bar on top. The diff
    viewer still gets the whole pane, and for the mouse the column belongs to the editor:
    the wheel over it scrolls the document, a click focuses the pane.
  - 721 tests (was 712).

## In progress

- Phase 14. The help screen, the responsive status bar, the watcher, the undo budget,
  reloading a buffer whose file changed, cancelling a running job, the combined-diff
  parser, the error-message pass, the per-file quit walk and the Open browser have
  landed; the README screenshot has not.
- **The CSV table view** (SPEC §65, ADR-062). A `.csv` or `.tsv` tab opens as a grid —
  the first row as the header, one row per record — with the delimiter and the quote
  character as clickable readouts on the status bar beside the encoding. `F4` swaps the
  pane between the table and the text, and the grid is reparsed from the buffer whenever
  the buffer or the dialect changes.
- **Editing in the table** (SPEC §65, ADR-063). Cells are typed into in place: `F2` or
  `Enter` opens one, a printable character replaces it, `Enter` and `Tab` save and move on,
  `Esc` gives up. `Insert` adds a record and `Ctrl+D` removes one; the header is a row the
  selection reaches, so a column is renamed in the grid. A write replaces only the field's
  own characters, quoting it where the dialect needs it, and is an ordinary document edit —
  one undo step, saved by `Ctrl+S`.
- **Selecting cells in the table** (SPEC §65, ADR-064). `Shift` with a motion key, a drag,
  a click on a record's number, a click on a column's name, `Ctrl+A` and the Selection menu's
  Select Row / Select Column all make a rectangle of cells; the status bar counts it as
  `Sel 3×2`. Copy writes the block
  in the file's own dialect, cut empties it as one undo step, `Delete` empties it in place.

## Known issues

- **`cross` is unusable on this machine.** Both musl targets fail before the build
  starts: `couldn't install toolchain stable-x86_64-unknown-linux-gnu` (cross 0.2.5 on
  an Apple Silicon host). Worked around locally with a plain `rust:alpine` container; CI
  uses `cross` and builds both targets there, so this is a local inconvenience rather
  than a gap in coverage.
- **The status bar's readout changes width as the window does.** Pieces leave as the
  terminal narrows and come back as it grows (ADR-039), so the left edge of the readout
  moves. That movement is the cost of not clipping the notification; the position stays
  at the right edge, which is where the eye returns to.
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
- **Deleting a file that is open leaves the tab open.** The buffer keeps its text and
  `Ctrl+S` writes the file back into existence, which is the point: the buffer is the
  last copy. Since ADR-043 the tab says so rather than looking unchanged; what it still
  does not do is offer to close itself.
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
- **There is still no Close All and no Close Others.** Quitting now asks about each
  unsaved file in turn (ADR-047), so the answers are per file, but closing several tabs
  that are *not* on the way out is still one `Ctrl+W` each. The cost of the walk is at
  the other end: discarding four files is four answers, where the old one-shot
  `Quit Anyway` was one.
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
- **Undo history can be dropped without the user being told.** Past sixteen megabytes the
  oldest steps go (ADR-042), and `Nothing to undo` is the only surface — the same sentence
  an empty stack has always produced. The newest step is never dropped, so the thing that
  just happened is always undoable.
- **Mixed line endings are normalised to the file's first one on save** — deliberate,
  and recorded as the cost in ADR-009.
- **ZWJ emoji width follows `unicode-width`.** `👨‍👩‍👧` is reported as six cells; some
  terminals draw it as two. Cursor movement is right either way (it is one stop), but
  the column arithmetic after it can be off on those terminals. ADR-004 chose this over
  per-terminal special-casing.

- **The status is read in the foreground.** It is a local read and takes milliseconds,
  but it happens on the UI thread, so the frame that triggers it waits for it — bounded
  by the ten-second kill timer, which is the pathological case (a held index lock) and
  not the normal one. Everything that *writes* runs on the worker (ADR-033); the read
  did not move, and a `git status` behind a held lock is still a pause.
- **The status is re-read after every job, in the foreground.** Staging three files in
  three keystrokes is three `git status` runs. They are milliseconds each and they are
  correct; they are not batched.
- **A branch cannot be deleted from the editor.** SPEC §33 says it can wait, and it is
  the one branch operation that loses work.
- **`cherry-picking` fills a narrow panel title.** All four unfinished operations are
  named now (ADR-041), and `[cherry-picking]` is long enough to push the changed-file
  count off a 32-column title. Finishing a rebase or a cherry-pick still means leaving the
  editor for a terminal; SPEC §35 does not ask for more.
- **The commit dialog does not prefill a merge message.** git writes one into `MERGE_MSG`
  and the dialog ignores it, so finishing a merge means typing the subject again.
- **A pull with divergent branches and no `pull.rebase` fails** with git's own advice
  about configuring one (ADR-036). That is the honest answer and it is still one more
  step than a user expected.
- **`git switch` needs git 2.23.** Older git has `checkout` and not `switch`, and the
  editor does not fall back to it.
- **The marker check reads the file on the UI thread**, capped at a megabyte. A
  conflicted file whose first marker is past that cap is staged without a question.
- **`Enter` on a deleted row opens an empty buffer.** The path is not on disk, so it
  opens the way any missing path does — and `Ctrl+S` writes the file back into
  existence, which is recoverable but not obviously what the row was offering.
- **A commit is not amendable, and there is no Undo for a git action.** Stage, unstage
  and commit are one-way from inside the editor; `git reset` in a terminal is the way
  back from a commit.
- **The commit dialog is one line.** `InputField` is a single-line field, so a commit
  body — the blank line and the paragraphs after the subject — cannot be typed. A
  multi-line body needs a text area, which is a Phase 14 shape and not a Phase 11 one.
- **A rename shows only its new path** in the panel, as ADR-031 said it would; the
  diff viewer's header is where `old -> new` is now readable, because git writes it
  there itself.
- **An untracked file has no diff.** git has nothing to compare it with until it is
  staged, and `git diff --no-index /dev/null <path>` — which would show it as one big
  addition — exits non-zero by design and needs a platform-specific null path. The
  viewer says why instead of opening empty.
- **The viewer is one file at a time.** There is no whole-repository diff and no
  hunk-level staging: SPEC §36 asks for a read-only unified diff and that is what this
  is.
- **The diff is re-read on every status refresh while it is open**, which is one extra
  subprocess per save. It is a local read of one path, and the alternative is a pane
  that says something that stopped being true.
- **Five thousand lines is the cap**, and a diff cut there says `(cut)` in its title
  with no way to see the rest from inside the editor.
- **Untracked directories collapse to one row.** `--untracked-files=normal` reports
  `dir/` rather than every file under it, which is what `git status` shows and what
  keeps a fresh `target/` from being ten thousand rows — but the count in the title is
  then a count of rows, not of files.
- **Ignored files are never listed**, and submodule state (the `sub` field of a v2
  record) is parsed past rather than shown.
- **Five thousand changed files is the cap.** Past it the title says `(5000+)` and the
  list stops; a status that large is a mass rewrite, and the panel stopped being
  browsable thousands of rows earlier (ADR-032).
- **The selection is an index, not a path.** A refresh that removes rows above the
  selected one moves the selection to a different file rather than following the file it
  was on.
- **A path that is not UTF-8 is exact on unix and lossy anywhere else.** The bytes are
  the name on the platforms the MVP targets; the fallback exists so the module compiles
  elsewhere, not because it is right there.
- **Discovery happens before the first frame.** Startup therefore waits for one
  `rev-parse` and one `status` — milliseconds in practice, and the same ten-second worst
  case as any other invocation.

## Manual checks performed

Phase 2 through Phase 8 acceptance were verified by driving the real binary in a pty
(the Phase 1 harness: fork a pty, set `TIOCSWINSZ`, write key and mouse bytes, replay
the output through a minimal terminal emulator).

### Phase 14 — the per-file quit walk

Same harness, at 60x20 and at 40x12 (the minimum layout width), over three files opened
from the command line and modified in turn.

- **Three Enters save three files and quit.** `Ctrl+Q` at 60x20 shows
  `┌ Unsaved changes (3 left) ┐` over `alpha.txt has unsaved changes.` and
  `[ Save ] [ Don't Save ] [ Cancel ]` with Save selected. Enter saves `alpha.txt`,
  closes its tab and asks about `beta.txt` with `(2 left)`; the third question drops the
  counter, and the Enter after it exits 0. All three files on disk carry the edit.
- **Two `Right, Enter` pairs discard two files and quit.** At 40x12, exit 0 and both
  files byte-for-byte as they were opened.
- **The box fits the minimum width.** At 40 columns the title, the sentence and all three
  buttons are drawn whole — the counter in the title is what buys that: with it in the
  message, `alpha.txt has unsaved changes (2 left).` needs 42 columns and lost its last
  character.

### Phase 14 — the error-message pass

Same harness, at 100x30, checking the colour the status bar actually emits rather than
the kind the code names.

- **The rule holds where it is easiest to get wrong.** `Ctrl+Z` on a fresh file, `Ctrl+V`
  with an empty clipboard, `Ctrl+C` with no selection, `Ctrl+W` with no tab left and
  `Esc` in the git panel with nothing running all emit `38;5;215` — the warning colour —
  for `Nothing to undo`, `The clipboard is empty`, `Nothing selected`, `No tab to close`
  and `Nothing to cancel`. Every one of those was `38;5;75` before the pass, the same
  blue as `Saved f.txt`.
- **And `Saved f.txt` is still that blue.** Typing a character and pressing `Ctrl+S`
  emits `38;5;75`: the pass moved the messages that report nothing happening, and left
  the ones that report something alone.

### Phase 14 — the diff of a conflicted file

Same harness, at 100x30, over a scratch repository whose `main` and `other` change the
same line of a three-line file, merged so that it stops.

- **`F6` `F6` `d` on `UU f.txt`.** The title reads `Diff — f.txt [worktree, merge] +5 −0`
  and the pane holds the combined diff git printed: `diff --cc f.txt`, an `index` with
  two blobs in it, `@@@ -1,3 -1,3 +1,7 @@@`, and the seven body lines.
- **All five conflict lines are green.** The emitted SGR is `38;5;114` — `git_added` —
  for `++<<<<<<< HEAD`, ` +OURS`, `++=======`, `+ THEIRS` and `++>>>>>>> other`, and
  `38;5;252` — the ordinary foreground — for `  one` and `  three`. Before this, ` +OURS`
  was drawn as context: 252, the same as the lines nobody was looking at.
- **The staged side says why it is empty.** `s` toggles to `Diff — f.txt [staged] +0 −0`
  with one line, `* Unmerged path f.txt`, in `38;5;245` — dim, because it is git talking
  about the file rather than a line of it.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
  (with and without `native-clipboard`) — all clean, 681 tests, zero warnings.

### Phase 14 — watcher, git operations, undo budget

Same harness. A scratch repository whose `main` and `other` change the same line, so
replaying either onto the other stops.

- **A change made outside the editor, at 74x18.** With the editor sitting idle, writing
  `a.txt` and creating `c.txt` from a shell put ` M a.txt` and ` ? c.txt` in the panel and
  `c.txt` in the tree, with no keystroke. `git add a.txt` turned ` M ` into `M  `.
  `git checkout -b topic` moved the panel title to `Git — topic` and the status bar's
  branch with it.
- **Three thousand files into an ignored `target/`.** The screen did not change, and the
  debug log recorded three `filesystem changed` lines for the whole session — one per real
  change and none for the burst. That is the filter and the window doing exactly what they
  are for.
- **The worktree/repository split.** The two changes that were `git add` and
  `git checkout -b` logged `FsChange { worktree: false, repository: true }`: the status was
  re-read and the tree was not.
- **A stopped rebase, at 110x16.** `git rebase other` from a shell, and the panel became
  `Git — detached [rebasing]` with ` UU c.txt` under it — again with no keystroke, and
  again through the watcher. `git rebase --abort` put it back to `Git — main` and a clean
  tree; `git cherry-pick other` made it `Git — main [cherry-picking]`.
- **The commit gate mid-rebase.** With the resolution staged, `c` opens the commit dialog
  — git allows a commit during a stopped rebase and the user staged something. With
  nothing staged it does not, and says
  `Nothing staged — finish the rebase with \`git rebase --continue\`` instead.
- **That sentence is what found the status bar bug.** It was clipped by three characters
  at 110 columns while the readout sat comfortably beside it; the floor is now the
  sentence's own width when that is longer than the constant.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
  (with and without `native-clipboard`) — all clean, 641 tests, zero warnings.

### Phase 14 — help screen and status bar

Same harness, at 100x30, 60x20 and 40x12 (the minimum layout width). The emulator now
replays the whole output stream for each snapshot rather than feeding it in chunks — a
read can split an escape sequence, and the garbled cells that produced were the harness's
and not the editor's.

- **`F1` at 100x30.** `┌ Keyboard shortcuts ───┐` over the whole body, the sidebar
  included, with the menu bar still on the row above it and `1/172` in the bottom right.
  `Anywhere` as the first heading, its note wrapped to two lines, then `Ctrl+Q  Quit,
  asking first when a tab has unsaved changes` and the rest of the global table. The
  focus readout said `Help`.
- **The over-wide label.** `Ctrl+Shift+Tab / Ctrl+PageUp` sits proud of the column and
  keeps two spaces before `Previous tab`, exactly as intended: the row is wider than the
  others rather than cut.
- **Paging.** `End` went to `147/172` — the input dialog's table and the closing sentence
  about `docs/SHORTCUTS.md` — and `Home` came back to `1/172`. `q` closed it and the
  document reappeared underneath with focus back on the editor.
- **`F1` at 40x12.** The notes re-wrapped to five lines, the key column narrowed, and the
  readout said `1/201` — 201 lines against 172 at 100 columns, which is the wrapping
  being counted honestly rather than a stored layout being reused.
- **The status bar at 60x20.** `Opened src/main.rs      Ln 1, Col 1   Rust   main
  Editor`: `UTF-8` dropped and the notification whole, where before it was clipped.
- **The status bar at 40x12.** `Opened src/main.rs  Ln 1, Col 1   main` — position and
  branch, and the sentence still intact.
- **Quit.** `Ctrl+Q` from inside the help screen still exits 0 with the terminal
  restored: the global table is underneath the screen's own.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`
  (with and without `native-clipboard`) — all clean, 622 tests, zero warnings.

### Phase 13

Same harness, at 100x30. A scratch repository with `a.txt` committed and then changed,
and an untracked `new.txt`.

- **The pane.** `Ctrl+B` into the git panel, `d`:
  `┌ Diff — a.txt [worktree] +2 −1 ────┐` over the editor, with `diff --git a/a.txt
  b/a.txt` and `index 4cb29ea..6addb9b 100644` dim, `@@ -1,3 +1,4 @@` in cyan, `-two`
  red, `+TWO` and `+four` green, ` one` and ` three` plain — and `1/10` in the bottom
  right. The focus readout said `Diff`; the document behind it was gone.
- **Nothing to scroll.** A ten-line diff in a twenty-six-row pane does not move under
  `Down`, which is the clamp doing its job.
- **A diff worth scrolling.** Sixty changed lines, each ninety characters wide:
  `PageDown` moved the readout to `26/125`, `End` to the last window (`+line 35
  changed …` at the top), `Home` back to `1/125`, and `Right` twice slid the text
  sixteen cells left (`xt b/a.txt`, `762eb2 100644`) with the border staying put.
- **The other side.** `s` with nothing staged: the title stayed `[worktree]` and the
  status bar said `Nothing staged in a.txt`. After `Space` staged the file, `d` opened
  `[staged]` and `s` there said `No unstaged changes in a.txt` — each side reports its
  own emptiness rather than blanking the pane.
- **An untracked file.** `d` on ` ? new.txt`: no pane, and
  `new.txt is untracked — stage it to see a diff` on the status bar.
- **Closing.** `Esc` put focus back on the git panel and the document reappeared
  underneath.

### Phase 12

Same harness. A scratch repository on `main` with a `topic` branch and a
`feature/editor` whose change to `c.txt` conflicts with `main`'s.

- **The picker.** `Ctrl+B`, `b`: `┌ Switch Branch ┐` over `3 branches`, the rows
  `  feature/editor`, `* main`, `  topic` — git's own marker on the branch `HEAD` is on
  — and `[ Switch ] [ New… ] [ Cancel ]`. `Down` then `Enter` put `Switched topic` on the
  status bar, `Git — topic` in the panel title and `topic` in the right-hand readout.
- **The merge picker leaves out the current branch.** `m` on `main` drew `2 branches`
  with `feature/editor` and `topic` and no `main`.
- **A merge that conflicts.** `Enter` on `feature/editor`: the title became
  `Git — main [merging] (1)`, the row `UU c.txt`, and the status bar
  `Merge failed: conflicts — resolve them in the panel, then commit` in the error colour.
- **Staging a file with markers asks.** `Space` on `UU c.txt` drew
  `┌ Conflict markers ┐` / `c.txt still has conflict markers.` with `[ Cancel ]`
  selected and `[ Stage Anyway ]` beside it.
- **The whole merge, finished inside the editor.** `Enter` on the conflicted row opened
  `c.txt` with git's `<<<<<<< HEAD` / `=======` / `>>>>>>> feature/editor` in the buffer;
  `Ctrl+A`, `resolved`, `Ctrl+S`; `Ctrl+B` twice back to the panel and `Space` — no
  question this time, and the row became `M  c.txt` with `[merging]` still in the title;
  `c`, `merge feature/editor`, `Enter` → `[main 31e2382] merge feature/editor`, the title
  back to `Git — main` and the tree clean. `git log` in the shell beside it showed the
  merge commit with both parents.

### Phase 11

Same harness. A scratch repository with one commit, a modified file, an untracked file
and a bare repository on disk as its `origin`.

- **Staging from the panel.** `Ctrl+B` into the git panel, then `Space`: ` M a.txt`
  became `M  a.txt` and the status bar said `Staged`. `Down`, `Space`: ` ? c.txt` became
  `A  c.txt`. `a` staged both at once and said `Staged every change`.
- **Commit.** `c` drew `┌ Commit ┐` with `Message for 2 staged files`, `[ Commit ]`
  selected and the focus readout on `Dialog`. Typing `phase eleven` and pressing `Enter`
  left the panel reading `Git — main ↑1` with `working tree clean`, and
  `git log -1 --pretty=%s` in the shell beside it printed `phase eleven`.
- **The Git menu.** `F10`, `Right`×5 opened it with all eight entries and their keys:
  Refresh `F5`, Stage, Unstage, Stage All `a`, Unstage All `u`, Commit… `c`, Pull, Push.
  Activating Push cleared the `↑1` from the title and put `Pushed` on the status bar;
  `git status -sb` agreed (`## main...origin/main`).
- **A push that does not return.** A repository whose `origin` is `https://192.0.2.1/`
  (reserved, unroutable): the panel title read `Git — Pushing…` and the status bar
  `Pushing…`, and the editor kept drawing and responding — `Ctrl+B` twice moved focus
  from `Editor` to `Explorer` with the push still hanging. This is the Phase 11
  acceptance: the network never reaches the event loop.
- **A push that fails.** A repository with no remote: `Push failed: No configured push
  destination.` on the status bar in the error colour, once, with no doubled prefix.

### Phase 10

- **The panel against this repository.** `ferroedit .` in a 110×24 pty, with the working
  tree mid-phase: the sidebar drew `Git — main (17)`, then ` M docs/ROADMAP.md`,
  ` M docs/SHORTCUTS.md`, ` M src/app/mod.rs`, ` M …c/commands/execute.rs` — the elision
  keeping the file name — and the status bar's right-hand readout ended in `main`. The
  codes matched what `git status --short` printed in the shell beside it.

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

### The status bar answers back (ADR-058)

The four readouts that describe the file are controls now: the cursor position opens Go to
Line, `UTF-8` the encoding picker, `LF`/`CRLF` the line-ending picker, and the grammar's
name the syntax picker. Each is a View menu entry as well, so none of them needs a mouse.
The line ending is shown on every file rather than only on CRLF ones.

- Converting the line endings asks first and then writes the file, because that is what a
  conversion is; the confirmation says the save takes the rest of the buffer with it, and
  Cancel is its default.
- The chosen grammar outranks detection until the tab's path changes, so a `.txt` file full
  of shell script can be coloured as one.
- The syntax picker has a filter over its 77 rows — `DialogBody::List` became `Picker`,
  which is the browser's `items`/`visible` split and its filter field (ADR-051).
- The rects the mouse hit-tests come from `ui::statusbar` itself, so a piece the bar dropped
  to fit a narrow terminal (ADR-039) has no rect and cannot be clicked.

### Legacy encodings (ADR-059)

A document carries a `Charset` — an `encoding_rs` decoder and whether the file has a byte
order mark — instead of being UTF-8 or nothing. Twenty-five of them in the picker, from
UTF-16 to KOI8-U to Shift_JIS.

- A file is read as its own mark says, else UTF-8 when the bytes are valid UTF-8, else
  Windows-1252 with the status bar saying so. Every byte of that fallback round-trips, so a
  wrong guess is one the user can look at and correct rather than one that costs them the
  file.
- Choosing an encoding asks what it means: **Reopen** re-reads the bytes (undoable, like
  every reload), **Convert and Save** keeps the text and writes it out.
- A save that cannot hold what is in the buffer is refused, naming the character, with the
  file on disk untouched — the Encoding Standard would have written `&#1071;` there.
- Binary files are still refused: a NUL in the first 8 KB of the decoded head.
- The syntax and encoding pickers draw their rows in a framed, scrollbarred box, the way the
  file browser has since ADR-051.
- The branch on the status bar opens the branch picker.

## Next

Phase 14 continues. What is left of it, roughly in order of how much it is worth:

- What is still unchecked by hand, as after every phase: the real target terminals —
  iTerm2, Ghostty, Terminal.app, tmux, plain ssh.
- Nothing about the release pipeline. `v0.1.0` is published with all four binaries and
  their checksums attached, from the first run `release.yml` ever had (ADR-049), and
  `scripts/release.sh` is what cuts the next one.
- A worker for the explorer's directory reads is the last of the known issues above that
  Phase 14 named as its own and has not answered.
- Reloading has no merge and does not offer one: Reload takes the file, Keep Mine keeps
  the buffer, and a save overwrites. Nothing in SPEC asks for a third answer, and the
  reload being undoable is what makes the pair enough.
- No README screenshot yet; the block at the top of it is still a hand-drawn mockup.
