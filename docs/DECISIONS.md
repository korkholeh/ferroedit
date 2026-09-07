# Architecture Decision Records

Short records of non-obvious choices. Add one whenever a decision would otherwise be
re-litigated in a future session.

---

## ADR-001: Git CLI instead of libgit2

**Decision.** All Git functionality goes through the system `git` binary via
`std::process::Command`. FerroEdit ships no Git implementation of its own.

**Why.**

- Credentials keep working — helpers, keychains, tokens, everything the user already has.
- SSH config, `include`s, and host aliases keep working.
- Commit signing keeps working.
- No native C dependency, so static musl builds stay trivial.
- The porcelain formats are stable and documented; the parsing surface is small.

**Cost.** We depend on `git` being installed, we pay process-spawn latency, and we must
parse text. Accepted: SPEC §4 already permits a system `git` dependency, and the
latency is hidden behind the worker thread.

**Safety rails.** Every invocation sets `GIT_TERMINAL_PROMPT=0` (never block on a
credential prompt), `GIT_OPTIONAL_LOCKS=0` (status must not fight a concurrent
`git gc`), and `-c core.pager=cat`, plus a job timeout.

---

## ADR-002: No async runtime

**Decision.** Background work uses `std::thread` + `std::sync::mpsc`. No Tokio, no
async-std.

**Why.** The entire concurrency workload is a handful of subprocess calls and directory
reads — a few long-lived worker threads, not thousands of tasks. A runtime would add a
large dependency tree and colored functions, and would simplify nothing: the main loop
already has to be a blocking select over one channel.

**Consequence.** Long git operations get a `JobId` and reply through the same
`AppEvent` channel as terminal input, which keeps a single mutation point in the loop.

---

## ADR-003: syntect with `fancy-regex`, not oniguruma

**Decision.** syntect is built with `regex-fancy` rather than its default `regex-onig`.

*Amended in Phase 7:* the runtime dependency is now
`features = ["parsing", "regex-fancy", "dump-load"]` and the build-dependency keeps
`default-fancy`. Since ADR-022, the grammars reach the binary as a dump rather than as
syntect's bundled assets, so the YAML and plist loaders that build them are needed at
compile time only. This is a smaller dependency tree, not a change of regex engine.

**Why.** syntect's default feature set links the C oniguruma library, which breaks the
pure-static musl build that SPEC §4 asks for and complicates cross-compilation to
`aarch64-unknown-linux-musl`. `fancy-regex` is pure Rust and supports the backreference
and lookaround constructs that Sublime syntax definitions actually use — which
`regex`-based alternatives do not.

**Cost.** `fancy-regex` is somewhat slower than oniguruma on pathological patterns. The
per-line checkpoint cache and viewport-only highlighting keep this off the hot path;
if it ever shows up in a profile, the fix is a better cache, not a C dependency.

**Verified.** Phase 0, in an `aarch64` Alpine container: the full dependency tree
compiles against musl and links to a ~400 KB static binary
(`ldd` → "Not a valid dynamic program").

---

## ADR-004: Grapheme-cluster cursor movement

**Decision.** The cursor is *stored* as a char index (`(line, char_in_line)`), but it
*moves* by grapheme cluster, and every horizontal position shown to the user is a
visual column computed with `unicode-width`.

**Why.** The three obvious single-type designs all break on real text:

- Byte indices split multi-byte scalars (`Привіт`).
- Char indices split grapheme clusters — one Left press on decomposed `é`
  (`e` + U+0301) would land between the letter and its accent.
- Visual columns are not invertible: many distinct positions share a column.

So each system is used where it is correct: chars for storage and rope operations
(`ropey` is char-indexed, making this O(1)), graphemes for movement and deletion,
visual columns for rendering, mouse hit-testing, and the preferred column that survives
vertical movement.

**Consequence.** `editor/coords.rs` is written and tested *before* anything depends on
it, against `tests/fixtures/unicode.txt` (SPEC §48). Known imperfection: terminals
disagree on the width of ZWJ emoji sequences (`👨‍👩‍👧`); we follow `unicode-width` and
record divergence as a known issue rather than special-casing per terminal.

---

## ADR-005: System clipboard is opt-in, OSC52 is the default

**Decision.** A `ClipboardProvider` trait with two always-available implementations —
an internal register and an OSC52 terminal escape writer. The native `arboard` backend
sits behind the non-default `native-clipboard` feature.

**Why.** FerroEdit is meant to be useful over SSH, where there is no local X11,
Wayland, or macOS pasteboard to talk to; OSC52 is the only mechanism that reaches the
*user's* clipboard from a remote host. `arboard` also pulls X11/Wayland libraries on
Linux, which conflicts with the static-binary goal.

**Consequence.** OSC52 is write-mostly (most terminals refuse paste-back for security),
so reads fall back to the internal register. A clipboard failure must never abort an
edit — copy failing is a notification, not an error dialog.

---

## ADR-006: Release builds keep unwinding

**Decision.** No `panic = "abort"` in the release profile.

**Why.** Terminal restoration has two independent paths: the `Drop` impl on
`TerminalGuard` and a `std::panic::set_hook` that restores before printing the payload.
`panic = "abort"` skips `Drop`, leaving only the hook — and if the hook itself is ever
bypassed or panics, the user's shell is left in raw mode with no echo (SPEC §41, R11).
The binary-size and speed win is not worth that failure mode.

---

## ADR-007: The theme uses 256-colour indexed values, not truecolor

**Decision.** `ui::theme::Theme` holds `Color::Indexed(..)` values throughout. No
widget names a colour directly; they all read the theme.

**Why.** Terminal.app is a supported target (SPEC §3) and does not do 24-bit colour —
it approximates RGB badly enough to wreck a dark theme's contrast. tmux and plain ssh
need per-setup configuration to pass truecolor through. Indexed colours render
identically everywhere on the target list, and ratatui does no automatic downgrading,
so choosing truecolor would mean choosing it for the terminals that cannot show it.

**Consequence.** The palette is limited to the xterm-256 cube. A truecolor theme stays
possible later — `Theme` is the only place colours live, so it is a data change, not a
sweep through `ui/`. Config-driven themes (Phase 14) will need to decide whether to
detect `COLORTERM` and offer both.

---

## ADR-008: Menu shortcut labels come from the keymap, not from the menu table

**Decision.** `commands::MENUS` carries no shortcut column. `ui::menu` asks
`event::keyboard::shortcut_for(command)` for the label to print.

**Why.** SPEC §25 requires that menu items and shortcuts cannot drift apart. A hardcoded
shortcut string in the menu table is exactly that drift waiting to happen: it will keep
advertising `Ctrl+S` after the binding is renamed, or before it exists at all.

**Consequence.** During the MVP build-out the menu shows a shortcut only for commands
that are genuinely bound — which is honest, and makes an unbound command visible at a
glance. Phase 9 generates `docs/SHORTCUTS.md` from the same table, so the menu, the
keymap and the documentation all have one source.

---

## ADR-009: The rope holds `\n` only; the file's line ending is remembered

**Decision.** `Document` normalises CRLF to LF when a file is loaded, records which
ending the file used, and writes that ending back on save. The buffer never contains a
carriage return.

**Why.** SPEC §17 asks that saving does not spoil line endings, which leaves two
options: keep `\r` in the buffer, or remember it. Keeping it puts a character on every
line that the cursor can land on, `End` stops in front of, and grapheme movement has to
special-case — a `\r` bug in every one of the coordinate functions, forever. Recording
it costs one enum field and one branch in the writer.

**Consequence.** A mixed-endings file is written back with the ending its *first* line
used, so untouched lines change. That is deliberate: normalising per line would mean
either remembering an ending per line or rewriting the file's every terminator, and
neither is worth it for a file that is already inconsistent. The status bar shows the
ending only when it is CRLF, so the case that matters is visible and the common one
does not spend the space.

---

## ADR-010: The editor pane's size is mirrored onto `App`, not passed to commands

**Decision.** `App::editor_view` holds the width and height of the editor pane in
cells. The main loop writes it after each frame; `execute_command` reads it to decide
how far to scroll and how far a `PageDown` goes.

**Why.** Scrolling needs the size of the window it is scrolling, and the layout only
exists while a frame is being drawn. The alternatives were worse: passing geometry
through every `Command` variant that might scroll would put terminal measurements into
the one enum that is supposed to be free of them, and letting `ui/` scroll would break
the read-only rendering invariant outright.

**Consequence.** This is a carve-out from invariant 3 in `docs/ARCHITECTURE.md`, and a
narrow one: the field is frame geometry, exactly like the `LayoutRects` the loop
already keeps for mouse hit-testing, nothing but the loop writes it, and no command
reads it for anything but a scroll distance. When it changes — at startup and on every
resize — the loop scrolls the cursor back into view and redraws immediately, which is
what makes `ferroedit +42 file.rs` land on line 42 rather than on the first frame's
idea of it.

---

## ADR-011: `Document` stores the selection anchor only; the head is the cursor

**Decision.** There is no `selection: Option<Selection>` field. `Document` keeps
`anchor: Option<Position>` and builds a `Selection` from the anchor and the live cursor
on demand. Plain movement clears the anchor; Shift movement leaves it.

**Why.** A stored two-ended selection has to be updated by every motion, every edit and
every mouse gesture, and the bug it invites is the head describing a place the caret has
already left — a highlight that lags the cursor by one keystroke. With the head defined
as the cursor, that state cannot be represented at all: the pair is consistent by
construction, and "extend" versus "move" is one line of difference at the call site
rather than a second position to maintain.

**Consequence.** `Command` carries `MoveCursor` and `ExtendSelection` as separate
variants over the same `Motion`, which is what lets Shift+navigation reuse all twelve
motions of SPEC §13 without a second table. A future multi-cursor editor would need to
revisit this — the anchor would become a per-cursor field rather than a document one —
but multi-cursor is not in the MVP (SPEC §50).

---

## ADR-012: `Command` is `Clone`, not `Copy`, so a paste can travel as one command

**Decision.** `Command::InsertText(String)` carries the pasted text, and the enum gives
up `Copy` to hold it. Bracketed paste and `Ctrl+V` both produce it.

**Why.** SPEC §15 wants a paste to be one operation, not N keystrokes: it is one rope
insert, one status line, and — from Phase 4 — one undo step. The alternatives were to
mutate `App` outside `execute_command` (breaking invariant 3 in
`docs/ARCHITECTURE.md`), or to park the text in a side channel on `App` and refer to it
by a handle, which is the same mutation with an extra indirection.

**Consequence.** The static keymap and menu tables hand out `command.clone()` instead of
a copy; for every variant but `InsertText` that clone is a memcpy of a few words, and a
paste is moved rather than copied. Nothing else changes: the tables are still `const`,
and `PartialEq` still drives `shortcut_for`, which now takes `&Command`.

---

## ADR-013: Undo steps are split by character-class runs, not by a timer alone

**Decision.** Two consecutive edits join one undo step when they are the same kind,
adjacent in the buffer, inside the 500 ms window *and* on the same side of a
character-class boundary: word characters, whitespace, punctuation, and a newline that
is always its own class. Anything longer than one grapheme cluster — a paste, a
selection removal — is a step of its own and does not absorb the keystroke after it.

**Why.** The acceptance for Phase 4 is that a typed word undoes as a word. A timer
alone cannot do that: typing fast enough turns a sentence into one step, and typing
slowly turns a word into five. The class rule is what makes `hello world` three steps
(`hello`, the space, `world`) regardless of speed, and it is the same segmentation a
user already sees under `Ctrl+Left`/`Ctrl+Right`. The window stays as the second half
of the rule so that coming back to the keyboard after a pause starts a new step.

**Consequence.** A run of emoji coalesces as punctuation rather than as words, because
`char::is_alphanumeric` is false for them — one backspace over `b👨‍👩‍👧` is two undo
steps, not one. That is a defensible split and it costs nothing but an extra `Ctrl+Z`.
The rule lives in one function (`history::mergeable`), so a different segmentation is a
local change rather than a redesign.

---

## ADR-014: The history owns the save point, so undo can clear the dirty marker

**Decision.** `History` remembers the undo depth at the last save. `Document::is_dirty`
is still a flag set by every mutation, but undo and redo recompute it from that mark, so
undoing back to what is on disk leaves the buffer clean. A save point on a branch
abandoned by a new edit is forgotten rather than reused.

**Why.** A document edited and then undone differs from its file by nothing; leaving it
flagged as modified would make the Phase 5 close-with-confirm dialog ask about a file
that needs no answer. Comparing buffer to disk on every keystroke is the alternative,
and it is O(document).

**Consequence.** One `Option<usize>` on `History` and two lines in undo/redo. The
compare is by *position in the history*, not by content: two different edits that happen
to produce identical text are still both dirty, which errs towards asking.


---

## ADR-015: The tab bar's scroll offset is derived, not stored

**Decision.** `ui::layout` computes which tab the bar starts at on every frame: the
earliest tab that still leaves room for the active one. No scroll offset is kept on
`App`, and nothing writes one.

**Why.** A stored offset is a second copy of the truth, and every operation that
changes the tab list — open, close, and the reordering that a future drag would bring —
has to remember to correct it. The bugs that follow are the classic ones: a bar scrolled
past the end after the last tabs are closed, or an active tab hidden because the offset
was not updated when it changed. Deriving it makes those states unrepresentable, and
the tab bar is one row of a few dozen cells, so recomputing it per frame costs nothing
worth measuring.

**Consequence.** The bar cannot be scrolled independently of the selection: there is no
"look at the other tabs without switching to one". That is the trade — and with the
active tab always on screen, the case for browsing the bar without switching is weak.
The rule "prefer showing the tabs *before* the active one" is what makes stepping right
scroll one tab at a time instead of jumping the active tab to the left edge.

---

## ADR-016: One dialog shape, and modality enforced at the input boundaries

**Decision.** A dialog is a title, a message and a row of buttons, where each button
carries the `Command` that choosing it runs (`app/dialog.rs`). It is a struct rather
than the `enum DialogState` of SPEC §40, because confirmation is the only kind that
exists yet. Modality lives in `event::keyboard::resolve` — which consults only the
`Dialog` bindings while a dialog has focus — and in `event::mouse::hit_test`, which
returns a button index or nothing. `execute_command` has no modal guard.

**Why.** SPEC §40 asks for one abstraction rather than an event architecture per popup,
and "a button is a label plus a `Command`" is the smallest thing that delivers it: the
close prompt and the quit prompt are two constructors, not two input modes. Enforcing
modality at the boundaries rather than in `execute_command` keeps the dispatcher a
plain match: the commands a dialog's own buttons produce have to run *through* it while
the dialog is closing, and a guard there would have to know which commands are exempt.

**Consequence.** Anything that reaches `execute_command` without passing the keyboard
or the mouse — a future worker thread's completion, say — is not blocked by an open
dialog. That is correct for a git job finishing, and it is a thing to remember when the
next producer of commands is added. The enum arrives in Phase 6, when input and
file-open dialogs give it a second and third variant.

---

## ADR-017: Quitting with unsaved changes asks first

**Decision.** `Ctrl+Q` and File → Quit open the same confirm dialog as closing a
modified tab when any tab is dirty, defaulting to Cancel. `Command::QuitDiscarding` is
what actually ends the run.

**Why.** Phase 5's scope is close-with-confirm, and stopping there would have left the
more common exit — quitting — discarding every unsaved buffer without a word. A prompt
on `Ctrl+W` next to a silent `Ctrl+Q` is not a defensible pair.

**Consequence.** `Ctrl+Q` is no longer a single keystroke to exit when something is
modified, which changes the Phase 1 acceptance ("Ctrl+Q завершує") into "Ctrl+Q exits,
after one Enter when there is unsaved work". The quit prompt defaults to Cancel rather
than to Save, because quitting is about *all* the dirty buffers at once and the answer
that does nothing is the safe one; the per-tab prompt defaults to Save for the opposite
reason.

---

## ADR-018: The CLI takes several files

**Decision.** `ferroedit a.rs b.rs c.rs` opens one tab per file, with the first active
and a `+42` applying to it. The workspace still comes from the first argument, and a
directory argument still opens no tab.

**Why.** Phase 5's acceptance is ten files open and switched between. Until the
explorer lands in Phase 6 there is no in-app way to open a second file at all — no
`Ctrl+O`, no clickable tree — so tabs would have been a feature reachable only from the
test suite. SPEC §49 lists the invocations that must work rather than the ones that must
not, and one-file-per-argument is what every editor already does.

**Consequence.** `Cli::path` became `Cli::paths`, and `open_cli_files` reports "Opened 4
files" rather than naming each one. A file among several that fails to open is an error
on the status bar and the rest still open, which is the behaviour that matters when a
shell glob picks up something unreadable.

---

## ADR-019: The dialog body is the enum, not the dialog

**Decision.** `DialogState` stays one struct — title, body, buttons, selected button,
return focus — and the SPEC §40 enum lands one level down as `DialogBody`, with
`Message` and `Input` variants. An input dialog is therefore still a constructor, and
still a row of buttons carrying `Command`s.

**Why.** ADR-016 promised the enum "in Phase 6, when input and file-open dialogs give it
a second and third variant". Writing it as `enum DialogState { Confirm(..), Input(..) }`
turned out to duplicate everything the two kinds share — every variant would carry the
same buttons, the same selected index and the same return focus, and every reader would
match twice to reach them. What actually differs between a confirmation and a name
prompt is the two rows above the buttons. Putting the enum there keeps `ui::dialog`,
`ui::layout` and `event::mouse` unchanged: they read `prompt()` and `buttons`, and only
the renderer asks whether there is a field.

**Consequence.** SPEC §40's `FileOpen`, `Commit` and `BranchPicker` become bodies rather
than dialogs, which is the right shape for the first two (a prompt plus a field, a
message plus a field) and will be the test for the third — a branch picker is a *list*,
and a `DialogBody::List` is where this decision earns or loses. The keyboard has to know
which body is open, because `Left` moves a caret in one and a button selection in the
other: `resolve` takes a `text_input` flag and consults a second binding table, which is
also the only place in the keymap where one focus has two tables.

---

## ADR-020: A click in the explorer acts on the row, without spending itself on focus

**Decision.** Clicking a row in the file tree selects it *and* opens the file or folds
the directory, whether or not the explorer already had focus. The git panel keeps the
older behaviour: the first click focuses the panel, the second selects a row.

**Why.** SPEC §27 asks the explorer's mouse to select a file, open a file, and expand and
collapse a directory. Clicking a file in a tree is how a file is opened in every editor
with a tree, and a rule where the same click opens a file only when the sidebar is
already focused is one the user has to keep in their head — and cannot see. The git
panel has no such action yet (staging arrives in Phase 11), so there is nothing there
that a focusing click would be in the way of.

**Consequence.** The two sidebar panels answer a click differently, which is a wart to
revisit in Phase 11 when the git panel gains its own row actions. `Command::SelectSidebarRow`
survives for the git panel and for tests; the explorer's click produces
`ExplorerActivateRow`, which focuses, selects and activates in one command so that the
three cannot happen out of order.

---

## ADR-021: Hidden and git-ignored files are one switch, and `.git` is never on it

**Decision.** The explorer hides both dotfiles and git-ignored files by default, and
View → Show Hidden Files toggles both together. `.git` is filtered by name and is not
shown either way. The walk uses `ignore` with `require_git(false)`, so a `.gitignore`
counts outside a repository too.

**Why.** SPEC §19 asks for git-ignored files to be hidden and lists "show ignored files"
and "show hidden files" as later, optional additions. Two switches for what the user
experiences as one question — "why can I not see my file?" — is worse than one, and
without any switch at all the editor cannot open its own `.gitignore`, which is a hole
big enough to notice on the first day. `.git` stays out because its contents are not
files anyone edits by hand, and a tree that offers to open `.git/objects` is offering a
mistake.

**Consequence.** A user who wants dotfiles but not build output cannot have exactly that;
the switch is coarse. `require_git(false)` also means a stray `.gitignore` in a
directory that is not a repository silently hides files there — correct, and worth
remembering when a file "is not in the tree" and nothing else explains it.

---

## ADR-022: The syntax set is linked at build time, and two grammars are our own

**Decision.** `build.rs` loads syntect's default grammars, adds every
`.sublime-syntax` in `assets/syntaxes/`, links the set, and dumps it into `OUT_DIR`.
The binary loads that dump with `from_binary`. `assets/syntaxes/` currently holds
hand-written TOML and Dockerfile grammars, and `highlighter::ALIASES` maps TypeScript
and JSX extensions onto the JavaScript grammar.

**Why.**

- SPEC §21 lists fifteen languages. syntect's defaults are the Sublime default
  packages, which have no TOML, no Dockerfile, and no TypeScript — three of the
  fifteen, and TOML is the language the editor's own config is written in.
- Adding a grammar means rebuilding the set, and *linking* it is the expensive part:
  measured at ~105 ms for ~80 grammars, against ~1.5 ms to load a pre-linked dump.
  105 ms before the first frame is a visible pause on every single run.
- Doing it in `build.rs` also means a broken grammar fails the build rather than the
  editor.

**Cost.** syntect is compiled twice, once for the host and once for the target, which
shows up in a cold `cargo build` and in CI. The dump is ~2 MB of the binary. Both were
already true of the defaults; the build script only adds the host compile.

**On TypeScript.** Writing a TypeScript grammar by hand is a large job done badly, and
vendoring a third party's is a licence and maintenance question that the MVP does not
need to answer. TypeScript is a superset of JavaScript, so the JavaScript grammar
colours everything the two share — keywords, strings, comments, numbers, functions —
and leaves type annotations and JSX tags as plain text. That is visibly worse than a
real grammar and visibly better than nothing. The alias table is one line per
extension when a proper grammar arrives.

**Consequence.** The hand-written grammars are ours to maintain, and they are
deliberately small: enough to colour a `Cargo.toml` or a Dockerfile correctly, not
complete implementations. Their tests are in `syntax/highlighter.rs`, so a change to
either one that breaks the common cases fails `cargo test`.

---

## ADR-023: Scopes are folded into a small `StyleKind`, not into syntect's themes

**Decision.** The highlighter never uses `syntect::highlighting`. It keeps a
`ScopeStack` itself and maps it to one of fourteen `StyleKind` values; `Theme` decides
what each one looks like.

**Why.** syntect's themes are truecolor, and ADR-007 says the palette is 256-colour
indexed because Terminal.app approximates RGB badly. Loading a `.tmTheme` only to
quantise its colours back down would mean two palettes to keep in agreement, one of
them invisible in `ui/theme.rs`. It also drops the theme dump from the binary and the
`HighlightState` from every checkpoint.

**How a kind is chosen.** By the *table's* order, not by the stack's depth: `//`
carries both `comment.line` and `punctuation.definition.comment`, and reading the
stack from the top down would paint the delimiter and the body of one comment in two
colours. `SCOPE_KINDS` is therefore ordered specific-to-general, `comment` and `string`
first, `punctuation` near the end.

**Cost.** A grammar that distinguishes, say, a control keyword from a modifier cannot
say so on screen: both are `Keyword`. That is a limit of 256 colours and a 14-pixel
font, not of the parser.

---

## ADR-024: A jump into an unparsed part of a huge file restarts near the viewport

**Decision.** The cache keeps the parser's state before every 64th line. To draw a
viewport it resumes from the nearest checkpoint at or above it — but only when that
checkpoint is within 2 048 lines. Further than that, it starts from a clean parser
state 256 lines above the viewport, and stores the checkpoints it passes on the way.

**Why.** Checkpoints only exist for lines that have been drawn. `Ctrl+End` in a
120 000-line file that has only ever shown its first screen has exactly one usable
checkpoint — line 0 — and running down from it costs ~6 s. The three options were: a
frozen editor, an uncoloured screen, or a colouring that is right for everything except
a construct longer than the look-back. The third is the only one a user would not file
a bug about.

**Cost.** The guess is wrong when a single construct spans more than 256 lines — a
licence header written as one block comment, a very long heredoc — and the lines below
it are then coloured as if that construct had ended. Scrolling up into the region and
back down does not fix it, because the guessed state is stored as if it were real; the
cure is an edit above the viewport, which invalidates the chain. Both constants are one
line each in `syntax/cache.rs` if a real file ever shows the seam.

**Measured.** In a release build, ~50 µs a line: 64 lines of catch-up is ~3 ms, the
2 048-line ceiling is ~100 ms, and the 256-line look-back is ~13 ms. Typing at the end
of a 3.3 MB Rust file costs ~7 ms a keystroke more than the same file with
highlighting off.

---

## ADR-025: The current match *is* the selection

**Decision.** Stepping to a hit selects it in the document: anchor at its start, caret
at its end. There is no separate "where the search is" cursor, and the current hit is
drawn with the selection's background while the other hits get their own.

**Why.** Every operation the user reaches for next already works on a selection —
`Ctrl+C` copies the match, typing replaces it, Replace has something to act on — and a
second position would need every one of them taught about it. It also means "scroll the
match into view" is `follow_cursor`, the rule Phase 2 already wrote.

**Consequence.** The status bar shows `Sel 3` while a match is current, which is
truthful and mildly noisy. And a search that finds nothing leaves the previous selection
alone rather than clearing it — correct, but worth knowing when reading the code that
does not clear it.

---

## ADR-026: Search results are a cache, refreshed once a frame

**Decision.** `App::sync_search` re-finds the hits before each draw, when any of the
query, the case option, the active tab or the document's `revision` has changed.
Commands that need a fresh list — next, previous, replace — call it themselves rather
than assuming the frame has happened.

**Why.** A dozen commands can invalidate the list, and a design where each of them
remembers to refresh is a design where one of them eventually does not. This is the same
shape as the highlight cache from Phase 7 and sits next to it in the run loop.

**Why a `revision` counter and not the highlight cache's watermark.** The watermark is a
*take*: reading it clears it, because the cache that reads it is re-parsing from that
line. Two consumers of one take would each starve the other. A monotonic counter can be
compared by any number of readers without consuming anything, so `Document` carries
both, incremented in the same place.

**Cost.** Typing a query into a 3.3 MB file costs a full scan per keystroke — measured
at ~5 ms on top of the no-search baseline, because the case-sensitive path is
`str::find` and the case-insensitive one bails on the first character that differs. A
file large enough for that to hurt is a file the highlighter has already given up on.

---

## ADR-027: Ten thousand matches is the limit, and the bar says so

**Decision.** `Document::find_all` stops at `MAX_MATCHES` and reports that it did. The
count reads `1/10000+`, and Replace All rewrites what it found and says "run it again".

**Why.** `e` in a five-megabyte file is around half a million hits: a vector nobody
reads, a memory spike on a keystroke, and a replace-all transaction that has to hold
every one of them. Truncating keeps search usable on exactly the files where an
unbounded list would not be, and saying so out loud is the difference between "run it
again" and "it silently half worked".

**Cost.** A genuine replace-all over more than ten thousand matches takes more than one
pass. Rare, and the alternative is a pass that might not finish.

---

## ADR-028: `docs/SHORTCUTS.md` is rendered from the tables, and a test enforces it

**Decision.** `src/docs.rs` renders the whole of `docs/SHORTCUTS.md` from
`event::keyboard::BINDINGS`, `INPUT_BINDINGS` and `commands::MENUS`. `Command::description`
is an exhaustive match, so a command cannot be added without a line of English for it.
Two ways to regenerate: `ferroedit --dump-shortcuts`, and `FERROEDIT_UPDATE_DOCS=1 cargo
test`; a plain `cargo test` fails when the checked-in file differs. The prose the tables
cannot know — typing, the mouse, terminal limits — lives in the generator, so the output
is a whole document rather than a fragment somebody has to splice into one.

**Why.** The file had been updated by hand five times in eight phases and was wrong by
the end of most of them: it described three keymap tables, listed `Ctrl+O` as intended
when it was unbound, and could not say that `Alt+C` only works while the find bar has
the caret. A binding carries a `label` and a `focus` already — everything the document
needs was in the table, and the only thing keeping the two in step was somebody
remembering.

**Why a test and not `build.rs`.** A build script that writes into the source tree makes
every build dirty and fights the CI cache. A test states the same invariant, fails with
the diff in the assertion message, and the environment variable turns the fix into one
command.

**Consequence.** A scope column appears in the menu table, which is what made two
inaccuracies visible on the first run: `Ctrl+Tab` was the label on the `Ctrl`+BackTab
row (it is `Ctrl+Shift+Tab` — BackTab *is* the shifted Tab), and the Search menu
advertises three `Alt` keys that only resolve in the find bar. The first was fixed; the
second is real and now documented rather than hidden.

**Cost.** The document reads more like a reference and less like an essay, and prose
about a binding has to be written in Rust rather than in Markdown. Save As is the phase's
one deliberate hole in the other direction: it is a menu entry with no key, because a
terminal without the kitty protocol reports `Ctrl+Shift+S` as `Ctrl+S` and Save must not
become ambiguous (the `Ctrl+Shift+Z` argument from ADR-008).

---

## ADR-029: Open… and Save As… ask for a path, and the list body waits for Phase 12

**Decision.** Both are `DialogBody::Input`: a prompt naming a directory, a field, and a
button carrying `FileOp::Open` or `FileOp::SaveAs`. A relative answer resolves against
that directory — the explorer's, for Open; the file's own, for Save As, pre-filled with
its current name. An absolute answer is taken as typed. Neither opens a file browser.

**Why.** The editor already has a file browser: the explorer is a pane, it is on screen,
and it opens a file in one click (ADR-020). A modal list inside a dialog would be a
second, worse tree — no expand/collapse, no ignore rules, no rename — built to answer a
question the pane behind it already answers. What the explorer cannot do is reach a path
outside the workspace, or one the user can type faster than they can click to, and a
field does exactly that.

**Consequence.** `FileOp` gained two variants that are not filesystem *changes*, which is
why its doc comment now says "needs a name or a path" rather than "a filesystem change";
`apply_file_op` routes both before its create/rename tail. Save As keeps the buffer, the
history and the save point and moves only the path (`Document::set_path`), so an undo
after it still walks back through edits made under the old name. Writing over a path
another tab holds is refused rather than silently creating two histories over one file.

**Consequence for ADR-019.** The `DialogBody::List` question is therefore still open, and
Phase 12's branch picker is what will settle it — a branch list has no pane behind it to
delegate to, so it is the case where a list body earns its place.

---

## ADR-030: The status is read on demand, in the foreground, with a kill timer

**Decision.** `git status --porcelain=v2 --branch -z` runs synchronously on the UI
thread: once at startup, then after every save, every file operation and every explicit
refresh (`F5` in the panel, Git → Refresh). There is no worker thread and no filesystem
watcher in this phase. Every git invocation gets `GIT_TERMINAL_PROMPT=0`,
`GIT_OPTIONAL_LOCKS=0`, `-c core.pager=cat`, a closed stdin, and a ten-second timer that
kills the child.

**Why.** A status is a local read that finishes in single-digit milliseconds on the
repositories a terminal editor is opened in; a `JobId`, a channel variant and an
in-flight state would be three moving parts spent hiding a pause nobody can see. The
work that genuinely cannot block a frame is `pull`, `push` and `fetch` (SPEC §37), and
those are Phase 11 — which is where the worker earns its keep and where this decision
gets revisited rather than reversed.

What is *not* optional is the timer. The subprocess that never returns is not the slow
one, it is the one waiting on a lock another git is holding, or on a credential helper
that wants a terminal a TUI is not going to give it. Ten seconds is far past any honest
status and far short of an editor that has stopped drawing.

**Consequence.** A change made by another program — a `git checkout` in the next
terminal — is not noticed until something refreshes. `F5` in the panel is the answer,
and a watcher is Phase 14's, along with the thread and the dependency it costs. The
startup discovery also means the very first frame waits for one `rev-parse` and one
`status`.

---

## ADR-031: The panel shows git's own `XY` pair, and the count goes in the title

**Decision.** Each row is two columns — the index side then the worktree side, exactly
as `git status --short` writes them — a space, and the path. There is no "Changes"
header row; the number of changed files is in the panel's title, next to the branch and
its ahead/behind counts. A path too long for the sidebar is elided from the *left*.

**Why.** The layout gives the panel four to ten rows and sixteen to thirty-two columns.
A header would spend one of those rows on a word that the title already implies, and on
a short terminal that is a quarter of the list. The two-column form is what SPEC §30
sketches (` M src/main.rs`) and, more usefully, what every user has already read a
thousand times in a terminal — the alternative, one collapsed "status" letter, throws
away the staged/unstaged split that Phase 11 needs on screen anyway. Eliding from the
left keeps the file name, which is what identifies a row; the directories in front of it
are what a reader can infer.

**Consequence.** An untracked file shows as ` ?` rather than `??`: porcelain v2 prints
one `?` for it, because there is nothing in the index to describe. A rename shows its
new path only — the original is parsed and kept, and the diff viewer of Phase 13 is
where it has somewhere to go.

---

## ADR-032: The parser reads bytes, and the paths stay bytes

**Decision.** `git status` is asked for `-z` output and parsed as `&[u8]`. Only the fixed
prefix of a record — the codes, the modes, the object names — is required to be ASCII;
the path is whatever bytes follow the last field separator, and it becomes a `PathBuf`
through `OsStr::from_bytes` on unix.

**Why.** Without `-z`, git quotes any path that is not plain ASCII, and unquoting it
correctly means reimplementing C string escapes for a format that exists only to be
unquoted. With `-z` there is no quoting at all: the path is raw and the record separator
is a byte that cannot occur in one. Reading it as a `String` would then be the second
mistake, because a unix path is bytes and not necessarily UTF-8 — a file called
`caf\xe9.txt` is a file, and staging it in Phase 11 has to pass git the name it gave us
rather than a lossy transcription of it.

**Consequence.** A rename is the one record that reads two fields, and the parser holds
its iterator across them for exactly that reason. `MAX_ENTRIES` caps the list at 5000
paths, with a `truncated` flag the title shows as `(5000+)` — the ADR-027 argument, for
the same reason: a mass rewrite must not turn one status into a hundred megabytes of
allocations.

---

## ADR-033: Everything that writes to the repository goes through one worker thread

**Decision.** Stage, unstage, stage-all, unstage-all, commit, pull and push run on a
single `std::thread` fed by an `mpsc` queue. Each submission gets a `JobId`; the thread
runs them serially in submission order and replies on the main loop's own `AppEvent`
channel, which the loop turns into `Command::GitJobFinished` so the answer mutates `App`
through the same door as a key press. Reading the status stays on the UI thread, exactly
as ADR-030 left it. `JobOutcome` carries `Result<String, String>` — git's own sentence,
or the reason it failed.

**Why.** This is the phase ADR-030 deferred to. `pull` and `push` are network-bound and
`commit` runs the user's hooks, so none of them has an upper bound a frame can wait for
(SPEC §34, §37). One thread and one channel is what SPEC §37 asks for, and a handful of
subprocesses is not a reason to take on an async runtime.

Staging is on the worker too, though it is fast, because it takes the index lock — and a
lock another git is holding is exactly the case that turns a millisecond into ten
seconds. Serial rather than parallel for the same reason: two of these racing for the
index would produce a failure the user did not cause, and a commit queued behind the
staging it depends on has to see that staging finish.

The error is flattened to a string at the worker's edge because a `Command` has to be
`Clone` and `PartialEq` and `GitError` is neither. Nothing downstream matches on the
variant — `App` only ever shows the message — so nothing is lost, and the invariant that
`execute_command` is the sole mutator is kept rather than special-cased.

`Pushing…` goes in the panel *title*, not only on the status bar: a notification expires
after four seconds (SPEC §39) and a push over a slow link does not, so the title is the
only place that can say "still running" for as long as it is true.

**Consequence.** The `GitService` travels with each job rather than living on the thread,
so a repository rediscovered under the editor cannot be answered about by a worker
holding the old root. `GitState` without a worker refuses jobs with a message instead of
spawning one, which is the state every headless test is in. A network command gets a
two-minute timer rather than the local ten seconds: killing an honest push after ten
seconds would be worse than the pause the worker exists to prevent. And a job's failure
is reported as `Push failed: <reason>` — git's own `git push failed:` prefix is dropped,
because it would say it twice and because "git reset failed" is not what a user who
pressed Space to unstage a file asked for.

---

## ADR-034: Pull is fast-forward only, and a conflicted file is not staged

**Decision.** `git pull --ff-only`. Staging or unstaging a file whose `XY` pair contains
`U` is refused with a message naming the file. `GIT_EDITOR=true` is set on every
invocation. No credential handling of any kind is bundled.

**Why.** Both halves are the same argument: Phase 11 owns the operations, and Phase 12
owns merging. A pull that merges can conflict, and a conflict needs a resolution UI that
does not exist yet — refusing with git's own "Not possible to fast-forward, aborting" is
a state the user can act on; a surprise merge commit made by an editor is not. `git add`
on a conflicted file is git's way of asserting "I resolved this", and the editor has no
diff and no conflict view to justify that assertion on the user's behalf.

`GIT_EDITOR=true` is not optional once commit is on the menu. A command that opens
`core.editor` would put a second full-screen program on the terminal the TUI is drawing
to. `true` exits 0 with an empty file, and every command here already supplies its own
message.

Credentials are SPEC §32's explicit instruction and worth restating: the system binary
is used precisely so the user's helper, keys and signing configuration are the ones that
apply (ADR-001). With `GIT_TERMINAL_PROMPT=0`, a helper that wants a terminal fails and
says so, which is an error a person can act on from a shell.

**Consequence.** A user whose workflow is "pull, then merge" gets a refusal instead, and
has to run the merge in a terminal until Phase 12. A conflict has to be resolved outside
the editor. A signing key that wants a passphrase from a terminal turns a commit into an
error message rather than a hang.

---

## ADR-035: The dialog body gains a list, and a picker is chosen with the confirm button

**Decision.** `DialogBody` gains a third variant, `List { prompt, items, selected, scroll }`,
where each `ListItem` carries the `Command` choosing it runs. The confirm button carries
`SubmitListChoice`, which `activate_dialog_button` replaces with the highlighted row's own
command — exactly as it replaces `SubmitInput` with a name and `SubmitCommit` with a
message. `Up` and `Down` are bound in the dialog table to `DialogListMove`; a dialog
with no list ignores them. The box's height is now computed from the body rather than
being a constant.

**Why.** ADR-019 left the list body open and ADR-029 deferred it with a rule: a list
earns its place only when there is no pane behind the dialog that already lists the same
things better. Open… and Save As… failed that test, because the explorer *is* the file
list. A branch list passes it — there is no branch pane, and there is not going to be one
in a sidebar that already holds a tree and a status.

Reusing the submit-and-substitute pattern is what keeps the dialog free of event handling
of its own (SPEC §40). `Enter` is `DialogActivate` on the default button, and the default
button is Switch; nothing about a list needs a second activation path, and the mouse gets
one behaviour rather than two.

The selection starts on the branch `HEAD` is already on, so opening the picker and
pressing `Enter` without reading checks out the branch you are on — a no-op, not a
checkout. Clicking a row selects without choosing, which is the opposite of ADR-020's
rule for the explorer, and for the reason ADR-020 gave: a click that acts saves a step
when the action is cheap, and a checkout is not.

**Consequence.** `DIALOG_HEIGHT` is gone and `body_height()` replaces it; a message and an
input body both report two rows, so every box that existed before is unchanged at five.
The button row is now placed from the bottom border rather than at a fixed offset. A list
is capped at ten visible rows and scrolls past that, because a picker taller than a short
terminal is a dialog that cannot be closed. `LayoutRects` gains `dialog_list` so the mouse
hit-tests the rows the renderer drew, and the row a click reports is a *screen* row —
the scroll is added in `select_visible_row`, which is the only place that knows it.

---

## ADR-036: Merge is a first-class outcome, and that is what unblocks ADR-034

**Decision.** `git merge --no-edit <branch>` runs on the worker. A merge that stops on
conflicts is reported as `GitError::Conflicted` rather than as a failed command. A stopped
merge is detected by `MERGE_HEAD` in the repository directory and shown as `[merging]` in
the panel title, independently of whether any conflict is left. Staging a conflicted file
is now allowed — it is how git is told a conflict is resolved — and asks first when the
file still contains a `<<<<<<< ` line. `git pull` loses its `--ff-only` and gains no
rebase flag of its own.

**Why.** ADR-034 refused both a merging pull and the staging of a conflicted file, and
gave the same reason for each: there was nowhere in the editor for a conflict to be shown
or finished. There is now. A user who starts a merge from the Git menu has to be able to
complete it from the panel, and `git add` is the only gesture git offers for "I resolved
this".

`Conflicted` is a variant and not a message, because a conflicted merge is not a command
that went wrong: git wrote its complaint to *stdout*, exited non-zero, and left the tree
in precisely the state the panel exists to show. The status is what distinguishes the two,
which costs nothing — the panel asks the same question a moment later anyway.

`MERGE_HEAD` rather than `conflicts() > 0` because they are different states and the
difference is where users get lost. Once every conflicted file has been staged there are
no conflicts left and the merge is still uncommitted; a panel that stopped saying so at
that moment would be silent exactly when the remaining step is least obvious. It is a
`stat` on a path already known, not another subprocess.

The marker check is the one thing worth keeping from ADR-034's caution. Only the
`<<<<<<< ` opener is looked for, at the start of a line: it is the marker that cannot
plausibly be a file's own content, and demanding all three would miss a half-finished
resolution. The read is capped at a megabyte because it happens on the UI thread.

Dropping `--ff-only` without adding `--no-rebase` is deliberate. Forcing a merge would
override a user's `pull.rebase` as surely as `--ff-only` overrode it, and SPEC §32's rule
is that the user's configuration is the one that applies. A divergence with nothing
configured is git's own well-known complaint about exactly that, which is better advice
than anything this editor could substitute for it — provided it reaches the screen, which
is why `first_line` now prefers a line git marked `fatal:` or `error:` over the first one.
A failed `git pull` writes the fetch it managed, then a dozen hints, and only then says
what went wrong.

**Consequence.** `GitService` holds the repository's git directory as well as its root, so
`discover` reads both from one `rev-parse`. A commit is refused while anything is still
conflicted, and allowed during a merge even with nothing newly staged — a resolved merge
still needs its commit. `git switch` is used rather than `git checkout`, so a branch name
that is also a path cannot be read as a request to discard that file's changes; it needs
git 2.23. Remote-tracking branches are offered in the switch picker and switched to by
their short name, because `git switch origin/topic` would detach `HEAD` while
`git switch topic` creates the local branch that clicking that row means. Deleting a
branch is still not possible from the editor — SPEC §33 says it can wait, and it is the
one branch operation that loses work.
