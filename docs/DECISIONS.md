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

*Superseded in Phase 14 by ADR-047.* Asking about all the dirty buffers at once is what
made Cancel the only safe default, and it is also what left saving them out of reach:
the walk asks per file, so `QuitDiscarding` is gone and Save is the default again.

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

---

## ADR-037: The diff viewer is a pane over the editor, and it dies with its focus

**Decision.** The unified diff of SPEC §36 is drawn as a bordered pane occupying the
editor's rect, with its own `FocusTarget::Diff` and a pager's keys. It is neither a
dialog nor a split: it covers the editor rather than shrinking it, and it is closed
automatically as soon as focus moves to another pane — one rule, enforced once, at the
end of `execute_command`. Which file it shows is the git panel's selected row while that
panel has focus, and the active tab's file everywhere else; which side it shows is the
worktree's when anything is unstaged and the index's when nothing is, with `s` to swap
and the title always naming the side. The diff itself is read in the foreground and
classified once into line kinds; the viewer re-reads itself on every status refresh.

**Why.** A diff wants width — eighty columns of context and a `+` column that lines up —
and the editor pane is the only place in this layout that has it. A dialog would have
been narrower and modal, and a diff is something a user reads *while* deciding what to
stage. A split would have squeezed the document to half a screen for a pane that is only
open for a few seconds.

Covering the editor is what forces the focus rule. A pane the user cannot see is a pane
the user is typing behind, so the viewer cannot outlive the focus that opened it. Putting
that check at the end of the one mutation entry point (ARCHITECTURE invariant 3) means no
individual command has to remember it: `CycleFocus`, a click on the explorer, opening a
file from the git panel and every future focus-mover are covered by the same three lines.
The menu and dialogs are the exception, because they draw *over* the viewer and hand
focus back when they close.

Foreground, like the status and the branch list, for the reason ADR-030 gave: a `git
diff` of one path is a local read that finishes in milliseconds, and a pane that opened
empty and filled in later would be one that scrolls under the reader. The write
operations are on the worker because they take the index lock; a read of one path does
not.

Classifying lines when they are read rather than when they are drawn is ARCHITECTURE
invariant 4 applied to one more derived view. It also puts the one subtle rule —
`--- a/x` and `+++ b/x` are the file header, not a removal and an addition — in a place
with tests rather than in a renderer.

Choosing the side automatically is what makes one key useful. "What did I change?" is the
question, and its answer is the unstaged diff right up until the moment there is nothing
unstaged left, when it becomes the staged one. Guessing wrong costs one keystroke, and
the title says what was guessed.

**Consequence.** An untracked file has no diff at all: git has nothing to compare it
with, so the viewer says why instead of opening blank — `--no-index` against a null
device would show it, at the price of a platform-specific path and an exit status that
means "they differ". A conflicted file's combined diff is shown as git writes it, but the
colouring reads only the first marker column, so a line removed from one parent reads as
context. The automatic re-read costs one extra `git diff` per save while the viewer is
open, and it is what makes staging the file on screen close the pane it emptied rather
than leave a change that is no longer there. A diff over `MAX_LINES` is cut with `(cut)`
in the title, the ADR-027 rule for one more unbounded thing. And ADR-031's rename has
somewhere to go at last: git's own `rename from` / `rename to` lines are in the header
the viewer draws.

---

## ADR-038: The help screen is the keymap, rendered — and it takes `Unimplemented` with it

**Decision.** Help ▸ Shortcuts and `F1` open a read-only pager over `docs::sections()`,
the same table `docs/SHORTCUTS.md` is generated from (ADR-028). It is drawn over the
whole body — sidebar, tab bar and editor — with `FocusTarget::Help`, the diff viewer's
keys, and the diff viewer's rule: it closes as soon as focus moves to another pane. Its
lines are laid out against the pane's width on demand rather than stored. With it,
`Command::Unimplemented` is deleted: the help screen was the last menu entry that did
not resolve to a real command.

**Why.** A help screen written by hand is a help screen that is wrong within two phases.
Phase 9 already made the keymap the single source for the document; the screen is a
second reader of the same data, so a key on screen, a key in the file and a key that is
actually bound are one row of one table by construction, and the tests that check the
document check the screen too.

Over the body rather than over the editor, unlike the diff viewer, because the two panes
want different things. A diff wants to be read *beside* the file tree while deciding what
to stage; a key table wants width, and at 60 columns the editor pane is 44 of them —
enough for `Ctrl+Shift+Tab / Ctrl+PageUp` and not much else. Covering the sidebar costs
nothing, because nothing under a help screen is being consulted while it is open. The
menu bar stays above it, which is also how the screen is dismissed with the mouse alone.

The same focus rule as ADR-037, and for the same reason: a pane the user cannot see is a
pane the user is typing behind. It is now one check covering both, at the end of the one
mutation entry point.

Laying the lines out on demand is what makes the notes wrap honestly. They are prose, so
they reflow when the window changes width, and a scroll offset counted against a layout
that no longer exists would jump on a resize. It is a few dozen rows of static text once
a frame — cheaper than storing a layout and remembering to invalidate it.

Deleting `Unimplemented` is the phase's own acceptance made structural. It existed so a
menu entry could be wired before its feature landed and say so rather than doing nothing;
every entry now resolves, so keeping the variant would only preserve the ability to add a
new dead one. A test asserting the list is empty is a weaker statement than a type that
cannot express the case.

**Consequence.** A key label wider than the column — `Ctrl+Shift+Tab / Ctrl+PageUp` is
the only one — sits proud of the other rows rather than being cut, because a truncated
key is a key nobody can press. The screen covers the tab bar, so no tab can be clicked
while it is open; `Ctrl+Tab` still switches, and the switch closes the screen. Typing,
the mouse and the terminal's own limits are prose in `src/docs.rs` and reachable only
through `docs/SHORTCUTS.md`; the screen says so on its last line rather than growing
three more sections of text nobody scrolls to. And a future phase that wants to ship a
menu entry ahead of its feature has to add the placeholder back deliberately.

---

## ADR-039: The status bar's readout has a drop order, not a format string

**Decision.** The right-hand readout — position, selection count, encoding, line ending,
language, branch, focus — is built as a list of pieces, each with a drop order. When the
readout will not fit in the width left after the sentence on the left has had what it
needs (a twenty-column floor, or its own length when that is longer), pieces leave in
that order until it does: encoding first, then the focus label, the language, the branch,
the line ending and the selection count. The cursor position is never dropped.

**Why.** This was a known issue from Phase 3: the readout was one `format!` of fixed
length, so at 60 columns it took its space and the notification beside it was clipped to
whatever was left. That is backwards. The readout is a reference — it says the same thing
on almost every file — and the notification is the one part of the bar that is telling
the user something they do not already know, often the reason a command did nothing.

The order is by how much a piece is worth reading. `UTF-8` is the same on every file the
MVP opens and goes first. The focus label is a debugging aid that a user learns to read
off the borders instead. The language is on screen in the colours; the branch is in the
git panel; `CRLF` is the surprising one and outlives them. A selection count is transient
and is only shown while it is true, so it is nearly last. `Ln 1, Col 1` is what a status
bar is for.

**Consequence.** The readout changes width as the window does *and* as the notification
beside it does — a long sentence sheds pieces on a terminal wide enough to have kept
them. Pieces reappear when the sentence expires or the terminal grows. That movement is the cost of the fix, and it is bounded: the left
edge of the readout is the only thing that moves, and the position stays at the right. At
40 columns — the minimum layout width — the bar is a notification and a position, which
is the honest content of a 40-column status bar.

---

## ADR-040: The filesystem watcher is a filter and a window, not a subscription

**Decision.** `notify` watches the workspace root recursively on a thread of its own.
Every event passes a filter — everything git ignores is dropped, and everything under
`.git` except the handful of paths that decide what the panel shows — and what survives
is held in a coalescing window before one `AppEvent::FilesChanged` is sent into the
channel the input thread and the git worker already write to. The change says whether the
worktree moved or only the repository did, and the refresh it causes is silent. A watcher
that will not start is a warning on the status bar and nothing else.

**Why.** Until this the explorer, the git panel and the diff viewer learned about a
`git checkout` in another terminal on the next save, file operation or `F5`. That is the
gap; the reason it stayed open through four git phases is that the naive version of
closing it is worse than leaving it open. A recursive watch on a repository reports git's
own churn — loose objects by the thousand, `index.lock` held for a millisecond — and a
build reports every artefact it writes. Refreshing on each of those would mean a
subprocess per file written.

So the filter is the feature, not the watch. `.git` is matched on the first path
component, which covers `refs/heads/topic` with one entry, and `index.lock` reports as
`index` because the rename over the real file is the event that matters and reporting
both would double every refresh. Outside `.git` the root's own ignore rules decide, which
is exactly the rule the explorer and `git status` already follow: what git ignores is not
on screen, so a change to it changes nothing.

The window then bounds what is left. A quiet period, so one `git checkout` is one
refresh; and a ceiling on top of it, because a writer that never goes quiet would
otherwise keep resetting the window and the panel would say nothing until it finished.
Only an event that survives the filter extends the quiet period — without that rule a
build in an ignored directory would hold the window open with events nobody wants, and
the refresh would be paced by the ceiling rather than by the change that earned it.

The worktree/repository split exists for one direction only: a worktree change is also a
`git status` change, but staging a file in another terminal is not a reason to rebuild
the explorer's rows.

Silent because a refresh nobody asked for should not talk. The status bar is where the
answers to the user's own commands go, and a notification every time a build touched a
file would take it away from them.

Best effort because it has to be: inotify watches are a per-user resource on Linux and a
large tree can exhaust them. Without a watcher the editor behaves exactly as it did
before this existed, which is a working editor, and `F5` is still there.

**Consequence.** One dependency and one thread, as the plan said it would cost. On Linux
`notify` is the pure-Rust `inotify` crate, so the static musl goal of ADR-003 is
untouched; the C `fsevent-sys` it links on macOS is not in any release target. Only the
root `.gitignore` and `.git/info/exclude` are consulted, because `ignore` matches a path
against a set of patterns and honouring a `.gitignore` in every subdirectory would mean
walking for them — the cost of missing one is an extra refresh, not a wrong screen. The
thread is detached like the input thread and notices the main loop's exit on the next
event, which on the way out of the process is never. And an open file whose contents
changed underneath is still not reloaded or flagged: the watcher makes that gap
*visible*, since the tree and the panel now update while the buffer does not, and closing
it needs a reload prompt that is its own decision.

---

## ADR-041: The panel names what git is in the middle of, not just a merge

**Decision.** `RepoStatus::merging` becomes `operation: Option<Operation>` over Merge,
Rebase, CherryPick and Revert, read from the four paths git records one at: `MERGE_HEAD`,
the rebase state directory, `CHERRY_PICK_HEAD` and `REVERT_HEAD`. Only a merge is
finished by a commit; for the other three the commit dialog does not open and the status
bar says which `git … --continue` does finish it.

**Why.** Phase 11 read one path because a merge was the only unfinished operation the
editor could start. A rebase or a cherry-pick started in a terminal left the panel listing
conflicted files under a title that said nothing about why they were conflicted — the
files were right and the sentence above them was missing, which is the worst of both.
Four `stat`s on a directory `discover` already found is the whole cost.

A rebase is recognised by its state *directory* and not by `REBASE_HEAD`, which is what
git's own status does: the directory is there for the entire rebase, and `REBASE_HEAD`
appears only once one has stopped. Merge is tested first because it is the one the editor
can finish, and because git will not have two of these in progress at once anyway.

The commit gate follows from that. `git rebase --continue` reuses the message git already
recorded and moves the rebase along; a commit written in the editor's dialog would be
neither of those things. Refusing and naming the command is more useful than a dialog that
produces the wrong commit.

**Consequence.** `cherry-picking` is long enough that `[cherry-picking]` fills a
32-column panel title on its own and pushes the changed-file count off the end. Finishing
a rebase or a cherry-pick still means leaving the editor for a terminal; making them
first-class is a phase of its own and SPEC §35 does not ask for it. And the watcher of
ADR-040 is what makes this worth having at all: the panel now notices the rebase starting.

---

## ADR-042: The undo stack has a byte budget, and the newest step is exempt

**Decision.** The undo stack carries a running weight — each step's text plus its own
place on the stack — and drops its oldest steps once that weight passes sixteen
megabytes. The newest step is never dropped, whatever it weighs. The save point moves
down by what was dropped, and is discarded when it was one of them.

**Why.** SPEC §16 asks for O(edited bytes) and never O(document), and that is what the
history has always delivered. It is still unbounded: a session that edits a hundred
megabytes holds a hundred megabytes, and nothing gives it back. Sixteen megabytes is
sixteen million characters of *edited* text, which no typing session reaches — the budget
is not for the typist, it is for the session that pastes and rewrites for hours.

Counting each step's own header as well as its text is what makes the budget mean
something. A million one-character steps hold almost no text and a great deal of `Vec`,
and a budget that saw only the text would not see them at all.

The newest step is exempt because the alternative is an editor that cannot undo the paste
that just happened. A twenty-megabyte paste against a sixteen-megabyte budget is exactly
the case, and losing it would be a data-loss bug wearing a memory-limit costume.

The save point is a stack *height*, so dropping the bottom of the stack moves every
height down by the same amount. A save point below the new floor names a state that can
no longer be undone back to, so it is discarded rather than left pointing at the wrong
one — a document that differed from disk and still looked clean would be worse than one
that always looks modified.

The weight is kept incrementally rather than recomputed. The budget is checked after
every keystroke, and walking the stack to do it would be O(steps) per character; a test
asserts the running figure against a full recount across typing, coalescing, pastes and
pops, because a number kept by hand is a number that can drift.

**Consequence.** Undo history can now be lost without the user doing anything, and
nothing says so — `Nothing to undo` is the only surface, which is the same sentence an
empty stack has always produced. The budget is a constant, not a setting: the field
behind it exists so the tests can use a small one, and nobody has asked for the option.
The redo stack is fed from the undo stack, so the pair is bounded by twice the budget
rather than by it.

---

## ADR-043: A buffer follows its file when it can, and asks when it cannot

**Decision.** Every open tab remembers what its file was — modification time and length —
the last time the buffer and the disk agreed, which is the moment it was opened, reloaded
or saved. When the watcher reports a change in the worktree, every tab is stat'd against
its stamp. A clean buffer whose file moved is re-read where it stands, silently. A
modified one is marked and left alone, and the question is put to the user: Keep Mine, or
Reload. The reload is a single undo step, so `Ctrl+Z` gives the unsaved work back.
`F5` in the editor is the same reload, asked for deliberately.

**Why.** ADR-040 made the gap visible and did not close it: a `git checkout` in another
terminal updated the explorer and the git panel while the document the user was looking
at stayed as it was, and the first hint of it was a save that quietly undid somebody
else's commit. Half a feature is worse here than none, because the panes that *did*
update are what makes the stale buffer look current.

The split between reloading and asking is the whole design. A clean buffer holds nothing
that is not also on disk, so re-reading it cannot lose anything and a prompt would only
be a keystroke charged for nothing. A modified buffer is the only copy of that work, and
no filesystem event — a build, a formatter, a branch switch — is allowed to spend it.

Asking is not free either, which is why the question is asked *once* per change and only
for the tab on screen. The watcher reports every burst in the workspace, so a mark that
did not remember having been raised would re-open the same dialog on every `cargo build`.
A background tab keeps its mark in the tab bar and is asked about when it comes forward:
a modal question about a document the user cannot see is a question about nothing.

The reload being undoable is what makes Reload a safe button rather than a one-way door.
It costs one step holding both versions of the file, which is what ADR-042's budget is
for — and the alternative, a reload that clears the history, makes the one destructive
action in the editor also the only one that cannot be taken back.

Default is Keep Mine. Every other confirmation in the editor is opened by the user, and
this one is opened by a filesystem event: it can arrive between two keystrokes, so the
reflex `Enter` must be the answer that does nothing.

The stamp is mtime *and* length because neither is enough alone — a coarse-grained
filesystem hides a rewrite inside one second, and a one-character change by another
editor keeps the length. Together they miss only a same-second edit that also preserves
the length. The alternative, reading every open file back on every event to compare it,
would make a build in the next terminal cost the size of the working set; a hash would
cost the same read. The stamp is taken *before* the read, not after, so a writer that
finishes between the two is reported rather than recorded as the state the buffer holds.

**Consequence.** A file deleted under a clean buffer is not closed and not emptied: the
buffer is the last copy, so it is marked, said once, and saving it puts the file back.
The same under a modified buffer is a question with one answer, because there is nothing
to reload from. A save answers the question by overwriting whatever the file became —
the editor does not offer a merge, and SPEC has never asked for one. The marker shares
the dirty marker's cell in the tab bar rather than taking one of its own; a stale tab is
nearly always a modified one, and the tab bar is already short of cells. Without a
watcher — the best-effort case ADR-040 leaves open — none of this runs on its own, and
`F5` in the editor is the manual door to all of it.

---

## ADR-044: A cancel is a watermark on the job ids, and it kills the child

**Decision.** `GitWorker` holds an `AtomicU64` saying "every job below this id is
cancelled", shared with its thread. `Esc` in the git panel — and Git ▸ Cancel — sets it to
the next id, which is exactly the set of jobs outstanding at that moment. The thread
checks it before starting each job, so a queued one is answered without being run, and
again on the same 2 ms poll that watches the timeout, so a running one has its subprocess
killed. Every cancelled job still comes back as an outcome, reported as `Push cancelled`
rather than `Push failed:`.

**Why.** The worker runs one job at a time and in order (ADR-033), which is right for the
index lock and wrong for a `git push` against an unreachable host: the two-minute network
timer is the only thing that ends it, and everything the user does in those two minutes
queues behind it. Half the value of cancelling is stopping the push; the other half is
that the three things behind it do not then happen anyway, minutes later, against a
repository the user has moved on from.

A watermark rather than a flag per job because "cancel" has exactly one meaning here —
stop what is outstanding — and outstanding is precisely "submitted before now". One
number answers for the job in git's hands and for the queue behind it, and a job
submitted *after* the cancel has a higher id and is untouched, so pressing Esc and
pushing again does the obvious thing instead of racing a flag that has to be reset.

It is an atomic rather than a message because a cancel sent down the job channel would
queue behind the job it is meant to stop, which is the one place it can never arrive.

Killing is the only way to stop the case that matters. A `git push` blocked on a socket
is not going to notice a polite request, and the kill path already exists for the
timeout — cancellation reuses it and differs only in which error it produces. Checking it
on the timeout's own poll means a cancel costs at most one `POLL` and no new thread.

The cancel token travels on the `GitService` copy the job already carries, so a status
read on the UI thread simply has `None` and nothing to check. That is what turned the
free `run`/`run_with`/`run_os` helpers into methods: the token has to reach the wait, and
threading it through as a parameter at twelve call sites would have been twelve chances
to forget it.

Cancellation is not a failure. `JobOutcome` carries a `JobFailure` rather than a `String`
so the difference is in the type: `Push failed: cancelled` would be the editor blaming
git for doing as it was told, and a job that stopped without a word would be a keystroke
the user has to guess about.

**Consequence.** A killed git leaves the repository in whatever state it had reached —
a partial fetch, a half-written index lock — and nothing here cleans that up; the status
re-read after every job is what reports it, and it is the same state a `Ctrl+C` in a
shell would leave. `GitState::cancel` returns how many jobs it asked about and does *not*
shorten the queue: the panel's idea of what is outstanding is only ever changed by an
answer. Cancel is bound only in the git panel, because `Esc` in the editor closes the
find bar; the menu entry is the door from everywhere else, and it advertises the key
because the menu reads its labels out of the bindings (ADR-008).

---

## ADR-045: The diff classifier is a state machine, because the format is one

**Decision.** `Diff::parse` carries two pieces of state per file — how many marker
columns its hunks have, and whether a hunk has begun — instead of classifying each line
by its first byte. The column count is read out of the hunk header (`@@@ … @@@` is two),
a body line is an addition if *any* of its columns holds a `+` and a removal if any holds
a `-`, and a `diff --git` / `diff --cc` / `diff --combined` line resets both. The diff
records the parent count it saw; the viewer's title says `[worktree, merge]` when there is
more than one. `* Unmerged path <file>` — what `git diff --cached` prints for a conflicted
file — is `Meta`.

**Why.** ADR-036 made a conflicted file's diff a view rather than a question, and the
view was wrong. `git diff` of an unmerged path is a **combined** diff, with one marker
column per parent of the stopped merge, so the two sides of the conflict are printed
` +ours` and `+ theirs`. A one-column reader called the first of those context: the line
a user opened the viewer to look at was the one line it declined to colour, and the `+5`
in the title was a `+4`.

Two columns also make the ambiguity that the old ordering papered over real rather than
theoretical. `--- a/file` is a file header; `--- x` is a line removed from both parents
whose own text begins `- `. They are the same bytes. What separates them is position —
a file header comes before its file's first hunk and contents come after — which is
exactly the state a one-line classifier does not keep. Reading the column count out of
the hunk header rather than out of `diff --cc` is the same argument: the header is where
git spells the number out, and it spells out three for an octopus without needing a
second rule.

`+` is answered before `-` only to have an answer at all. A combined row cannot hold
both: a `-` marks a line missing from the result and a `+` marks one that is in it.

The parent count reaches the title because the columns do not announce themselves. A
reader who has not been told that `+ theirs` is "added against the first parent" will
read it as an ordinary addition of a line beginning with a space, and a viewer that
knows the difference and keeps it is the one thing worse than not knowing.

**Consequence.** The classifier is per-`parse` state and not a free function, so nothing
else can classify one line in isolation and get it right — which is the point, and is why
`classify` is gone. A combined diff's `+`/`−` summary counts result lines rather than
changed ones, so a conflict of one line reads `+5 −0`: that is what the output holds, and
inventing a smaller number would be summarising a diff we do not otherwise interpret. The
staged side of a conflicted file is still one dim line saying there is nothing to diff —
git's own answer, and a truer one than an empty pane.

---

## ADR-046: A notification's kind says what happened, not how bad it sounds

**Decision.** The three `NotificationKind`s are decided by one mechanical question — did
anything change?

- `Info` — it was done. `Saved main.rs`, `Copied 5 characters`, `Cancelling…`.
- `Warning` — nothing was done, and the reason is the state the editor is in.
  `No file to save`, `Nothing to undo`, `Nothing selected in the Git panel`.
- `Error` — it was attempted and something outside the editor refused: the filesystem,
  git, the terminal. `Failed to save: Permission denied`.

Seventeen call sites moved to match. A failure message names the thing the *user* asked
for and quotes the reason after a colon — `Failed to open:`, `Failed to delete:`,
`Diff failed:` — and `GitError::reason` is what goes after that colon. `GitState::start`
returns a `NotStarted` with two variants rather than one string, so its caller can tell
the two apart without reading the message.

**Why.** The messages were written one feature at a time and each read well beside the
feature it belonged to. Read side by side they did not: `Nothing to undo` was `Info` and
`No file to save` was `Warning`; `Nothing selected in the Git panel` was `Info` and
`Select something in the explorer first` was `Warning` — the same situation in the two
panels, in two colours and two voices. Fifteen sites had made this decision on their own
and split roughly down the middle.

"Nothing changed" is the whole of the middle case, and it is the rule that makes the
existing `Warning` sites right without exception rather than a new convention imposed on
them. It is also the one worth a colour: yellow means the keystroke landed and produced
nothing, which is exactly the moment a silent no-op leaves a user pressing the key again.
`Error` is reserved for something outside the editor saying no, so it keeps meaning
"a thing went wrong" rather than "you asked for something unavailable".

The doubled prefix was the other thing only a side-by-side reading finds. `Diff failed:
{err}` printed `Diff failed: git diff failed: fatal: …` — the sentence twice, naming a
subprocess the user never typed. `worker.rs` had already solved this for the background
path and documented why; the fix is to move that knowledge onto `GitError` so both paths
share it, and `git reset failed` never reaches a user who asked to unstage.

`FsError` keeps no verb prefix. Three of its four variants complain about a typed *name*
and already stand alone — `Could not create foo.txt: foo.txt already exists` would say
the name twice to add a verb nobody needed. `DocumentError` is the opposite: it is
`{path}: {source}` and the source is the OS's fragment, so every one of its sites
supplies the verb.

**Consequence.** A test walks ten declined commands and asserts every one is a `Warning`;
the rule is checkable rather than a paragraph, which is what the fifteen-way split came
from not having. The rule does not decide *wording* — `No file to save` and `Nothing to
undo` are both correct under it — and the pass left the two families that already read
consistently alone. `Merge failed: conflicts — resolve them in the panel, then commit`
stays an `Error` even though ADR-036 calls a stopped merge not-a-failure: git itself
calls it one, the message is accurate and actionable, and the alternative was a third
`JobFailure` variant for a case the user is not confused about.

---

## ADR-047: Quitting walks the unsaved tabs, one question each

**Decision.** `Ctrl+Q` with dirty buffers asks the close-tab question — `a.txt has
unsaved changes.` over `[ Save ] [ Don't Save ] [ Cancel ]` — once per dirty tab, in tab
order, counting down in the *title*: `Unsaved changes (3 left)`. Each answer closes that
tab and asks again; when none are left, the editor exits. Save is the default. A save
that fails stops the walk where it is. `Command::QuitDiscarding` is deleted and replaced
by `SaveAndQuit(index)` and `DiscardAndQuit(index)`.

**Why.** The all-at-once prompt could express two answers — lose everything, or nothing —
and the one users want most was not among them. Saving four modified files on the way out
meant cancelling the quit, pressing `Ctrl+S` in each tab, and quitting again. ADR-017
chose Cancel as that dialog's default for exactly this reason: its Enter discarded every
dirty buffer, so the reflex answer had to be the one that did nothing.

Asking per file makes Enter safe, and a safe Enter is worth more than a short one: hold
it down and every file is saved and the editor exits. It is also not a new dialog. A quit
*is* closing every dirty tab and then exiting, so the question is the one `Ctrl+W`
already asks, with the same three answers in the same order.

The count is in the title because the message is a sentence about one file and a counter
bolted onto it — `alpha.txt has unsaved changes (2 left).` — is what pushes the box past
a 40-column terminal for any ordinary file name. A title is where a progress indicator
belongs, and it leaves every question of the walk asking in the close-tab dialog's own
words.

A failed save is the one answer that must not go on. The file is still only in the
buffer, and an editor that shrugged and quit past `Permission denied` would be the single
worst thing it could do; the walk stops, the error is on the status bar, and the tab is
still open.

**Consequence.** Discarding four files now costs four `Right, Enter` pairs where
`Quit Anyway` cost one — the price of not having a fourth button, which at
`[ Save ] [ Don't Save ] [ Discard All ] [ Cancel ]` is fifty columns and clips on the
forty the layout supports. Cancel ends the walk without undoing the answers already
given: each was final when it was made, and a save is not something to take back. That
leaves a cancelled quit with the answered tabs closed and the rest untouched, which is
what every editor that asks this question does.
---

## ADR-048: A fixture repository carries its own identity, because the code under test does not

**Decision.** `TestRepo` writes `user.name`, `user.email` and `commit.gpgsign = false`
into each fixture repository's own `.git/config`, rather than passing them only as
environment variables on the commands the helper itself runs. `TestRepo::clone_from`
exists so the one repository made by cloning gets the same treatment. `commit` drops its
`-c commit.gpgsign=false`, which the repository config now covers.

The CI `targets` job also stops failing a binary it should pass: `file` reports
`static-pie linked` for what a current Rust and musl toolchain produces, and the step
matched only the older spelling `statically linked`. Both are static, and an ELF
interpreter is checked for separately as the half that does not depend on wording.

**Why.** The environment `try_run` passes covers the commands *these helpers* run.
`GitService` is the other thing running git in these tests, and it is the code under
test: it passes the editor's own environment — `GIT_TERMINAL_PROMPT`, `GIT_EDITOR`,
`GIT_OPTIONAL_LOCKS` — and deliberately no identity, because SPEC §32's rule is that the
user's configuration is the one that applies. So every merge, pull and commit
`GitService` made was resolving an identity from the machine, and the module's own
promise — "a developer with `commit.gpgsign = true` gets the same results as CI" — was
only half kept.

It held on any developer's machine and failed on a runner, which is the worst shape for
this: eleven tests went red on `empty ident name (for <runner@…>)` the first time CI
reached them, and none of them were about identity. git derives a name from the
password-file entry when nothing else supplies one; a personal account has a full name
there and the `runner` user does not.

Repository config is the fix rather than more environment variables because it is what
both callers read without being told, and because it is what a repository a user actually
works in already has. Environment would also have been *wrong* in the other direction:
`GIT_AUTHOR_NAME` overrides config, so setting it in the test process would have masked
the very resolution the tests exercise.

**Consequence.** A test asserts the exact identity a `GitService` commit lands with, not
merely that it has one: the failure being guarded against is the fixture silently
falling back to the developer's own name, which is what makes this class of bug invisible
until CI. The static-link assertion now accepts two spellings, so a future `file` that
invents a third will need it added — the interpreter check is there so that a wording
change alone cannot turn a dynamic binary green.

---

## ADR-049: A release is a GitHub Release, drafted first and published last

**Decision.** `release.yml` uploads no workflow artifacts. A tag push (or a manual run
naming an existing tag) drafts a GitHub Release, four matrix jobs build their target and
attach a `.tar.gz` and a `.sha256` to that draft with `gh release upload --clobber`, and
a final job publishes it. The static-link assertion `ci.yml` makes is repeated here, on
the binaries that actually ship.

**Why.** The Phase 0 stub packaged the four binaries and handed them to
`actions/upload-artifact`, which is the wrong end of the pipe. An artifact expires after
ninety days, needs a GitHub login to download, and is reachable only by finding the run
that produced it — none of which describes what someone wants when they are looking for a
build. A release asset is a stable public URL on the repository's front page.

Drafting first is what makes a four-job matrix safe to publish from. The alternative —
each job creating-or-reusing the release as it finishes — races on the first two to
arrive, and worse, makes the release visible the moment the fastest target lands, so a
Linux build that failed leaves a published release that quietly has no Linux binary in
it. Here `publish` needs every build, so a half-finished run leaves a draft only the
maintainer can see.

`--clobber` because re-running one failed target has to be a normal thing to do; without
it the upload fails on the asset a previous attempt already put there.

The static assertion is duplicated rather than trusted from CI because CI checks the
commit and this checks the artefact. They are the same code today and the failure they
guard against — shipping a musl binary that turns out to need a dynamic loader — is worth
catching on the file that is actually going out.

**Consequence.** The workflow needs `permissions: contents: write`, which the default
read-only token does not have. `workflow_dispatch` now takes a tag rather than defaulting
to a branch: a release has to name a version, and a manual run of it must be the same
thing as the tag push, not a second kind of release. Nothing publishes automatically
without a `v*` tag, so the release binaries the phase owes are one `git tag` away rather
than one workflow away — which is the right place for that decision to sit.

---

## ADR-050: The macOS binaries are signed and notarised, and cannot be stapled

**Decision.** `release.yml` signs each macOS build with a Developer ID Application
certificate under the hardened runtime and submits it to Apple's notary service before it
is packaged. The step is conditional on `MACOS_CERT_P12` being configured, but a
*half*-configured signing setup fails the job rather than falling through: with the
certificate present, every other secret must be too. A macOS build going out unsigned
says so as a warning in the run.

**Why.** The first release proved the thing everyone assumes is a Finder problem is not.
A quarantined binary with no Developer ID is killed on `execve` — from a terminal, with
no dialog in the shell at all, just `exit 137`:

```
$ xattr -w com.apple.quarantine "0081;…;Safari;…" ferroedit
$ ./ferroedit --version
exit=137          # 128 + 9, SIGKILL
```

Rust's linker ad-hoc signs on arm64, which is what lets the binary run at all on Apple
silicon, but `TeamIdentifier=not set` and `spctl` answers `rejected`. Quarantine is
applied by browsers; a `curl | tar` install never carries it, which is why the same file
runs perfectly one way and dies the other.

**A bare Mach-O cannot be stapled.** `xcrun stapler` writes its ticket into an `.app`, a
`.dmg` or a `.pkg`, and an executable has nowhere to put one. Notarisation still works —
Gatekeeper looks the ticket up online — so the cost is that a first run of a
browser-downloaded binary needs the network. Shipping a `.pkg` would remove even that,
at the price of turning "unpack and run" into an installer; for a terminal editor mostly
fetched with `curl`, that trade is not worth making yet.

The keychain is created per run, unlocked with a throwaway password, and deleted by a
trap on exit, so the private key does not outlive the step even when signing fails.
`set-key-partition-list` is not optional: without it `codesign` blocks on a UI prompt no
runner can answer, which presents as a hung job rather than an error.

**Consequence.** Releases now depend on six secrets and on Apple's notary service being
up; `--wait --timeout 30m` bounds the second. A notarisation that is rejected prints the
submission log before failing, because the status alone is one opaque word. The
`spctl` check at the end is informative only — an unstapled binary is assessed online,
and a release must not fail on Apple's CDN being briefly slow.

---

## ADR-051: Open is a browser, and opening a folder moves the workspace

**Decision.** `Ctrl+O` opens a directory listing instead of a bare text field. The box
shows where it is, a `Filter:` field, and the rows of that directory in a framed pane with
a scrollbar — `..` first, then directories, then files, each sorted case-insensitively.
`Open` walks into the selected directory or opens the selected file; `Open Folder` makes
the selected directory the workspace, falling back to the directory being listed. The
field is still the path field it used to be: an answer that resolves to a real path wins
over the selection, and one that matches nothing in the listing is taken as the path of a
file that does not exist yet.

It opens at the **workspace root**, not at the explorer's selection. The sidebar's
position is a place in a tree the dialog is not showing, so starting there opens the box
somewhere the user did not choose and cannot see the way out of; the root is the one
directory they picked on purpose.

`..` is the way out of a listing, not a thing in it, so `Open Folder` does not count it.
Walking into a folder leaves the selection on `..` — which is precisely when Open Folder
is likely to be pressed — and counting it opened the *parent* of the folder just chosen.

Opening a *file* leaves the sidebar showing the folder it is in. Inside the current
workspace that only means revealing it in the tree; outside it, the workspace moves to
the file's directory — the same rule `ferroedit path/to/file` has followed since Phase 1
(SPEC §10).

**Why.** ADR-029 chose a text field on the grounds that the explorer was how a project
was browsed and the prompt only had to reach what the explorer could not. That was true
of paths *inside* the workspace and false of everything else: the field could only be
used by someone who already knew the answer, and there was no way at all to open a
different project without restarting the editor. A file manager is the one dialog where
the list is not a worse version of a pane behind it — there is no pane behind it.

**The filter is the field, not a second one.** A browser needs both a listing and
somewhere to type, and giving it two places to type would need a rule for which one has
the caret. Instead the one field narrows the rows, and the same text is read as a path
when it names something real or matches nothing. That keeps `dialog_wants_text` — the
predicate that picks the keyboard table — a straight question about whether the body has
a field, and it means a pasted path still works exactly as it did.

Typing moves the selection to the first row that is not `..`. `..` survives every filter,
so a selection resting on it would never move, and `Enter` after typing a name would walk
*up* a directory instead of opening what was typed.

**The rows are a pane, not three lines of a prompt.** The browser gets its own border,
its own scrollbar and twice the picker's row budget (`MAX_BROWSER_ROWS`, twenty), clamped
to the terminal. A branch list is read once and answered; a directory is *scanned*, and
ten unframed rows with no scrollbar say neither "this scrolls" nor "this is the list".
The scrollbar is drawn only when there is something to scroll, because a full-height thumb
is a control that lies about having something to do, and it replaces the right border
between the corners rather than the whole edge — a frame missing its corners reads as
broken.

Because the box is clamped to the terminal, the window it scrolls within is a property of
the last drawn frame and not a constant: `App::dialog_rows` carries it out of the layout
the way `explorer_rows` already does, and `step_list` takes it. A constant here would
scroll against a window that does not exist on a short terminal.

The wheel over the list moves the *selection*, which is what scrolls it. That is what the
keyboard already does, and a picker whose selection can scroll out of sight would need a
second rule for what Enter then means.

**A second click opens.** ADR-020's rule for dialogs is that a click on a list row selects
without choosing, because the confirm button is right there and choosing a branch by
accident is a checkout. Browsing inverts the arithmetic: a directory is several steps
deep, and a click-then-button for each of them is not a convenience. So a click on the row
that is *already* selected opens it. It is a double-click without the timing — the first
click still only selects, so nothing opens by surprise, and it works on terminals whose
click reporting is too coarse for a real double-click.

**Moving the workspace means moving the watcher.** The root was fixed for the life of the
process, so `watcher::spawn` could move its `notify` watcher into the thread that reads
it. A root that changes needs the watch to *end*, so the watcher now stays on the caller's
side in a `Watch` handle: dropping it closes the channel the thread is blocked on, and the
thread returns. `App` holds the handle and a clone of the run loop's sender, which is the
one place state and a thread are coupled — the alternative was for `execute_command` to
return work for `main` to do, and one field is cheaper than a second mutation path.

Open tabs are deliberately untouched when the workspace moves. A tab is a buffer and a
path, not a member of a directory, and closing files because the sidebar moved would be
the editor throwing away work nobody asked it to.

**Consequence.** `FileOp::Open` is gone: the operation it named is no longer answered with
a typed name. The `Up` and `Down` keys are now in the input-dialog table as well as the
plain one, since the browser is a field *and* a list; a field with no list under it
ignores them, which is what those keys did there before. `Browser` reads a directory
eagerly on every step, unlike `FileTree`'s lazy expansion — one `read_dir` per keystroke
is not a cost worth a cache, and a listing that is a syscall old is a listing that is
right.

## ADR-052: Every scrolling pane draws a scrollbar, and only when it scrolls

**Decision.** The explorer, the git panel and the editor each draw a vertical scrollbar,
rendered by one shared `ui::scrollbar::render` rather than three copies of the same
`ScrollbarState` arithmetic. The bar appears only when the pane has more rows than it can
show: a full-height thumb is a control that lies about having something to do, and it is
the same rule ADR-051 already applies to the file browser.

**Where the column comes from.** The two sidebar panels put the bar on the border they
already draw down their right edge, so a long tree costs no width — a sidebar is sixteen
cells at its narrowest, and a file name is what it is there to show. The track is the
border's own `│`, so a pane that fits keeps an unbroken edge instead of growing a second
vertical line.

The editor has no border to borrow, so the layout reserves it one: `LayoutRects` gains an
`editor_scrollbar` column beside `editor`, and `MIN_EDITOR_WIDTH` now means twenty columns
of *text* with the bar on top of it. The column is reserved whether or not a bar is drawn
in it. The alternative — taking the column only when the document outgrows the pane —
reflows every line on screen at the moment the user is typing past the bottom of the
window, which is the least welcome moment there is. It is taken from the text rather than
from the gutter because the gutter's width is what `gutter_width` tells the mouse, and a
click landing a column off the character under it is the bug the shared function exists to
prevent.

**Consequence.** The diff viewer keeps the whole pane — `rects.diff` is the pane and not
the narrowed `editor` — because it covers the editor rather than sharing it, and a viewer
one column short of its own pane reads as a drawing bug. For the mouse the reserved column
belongs to the editor: a wheel over it scrolls the document, and a click focuses the pane
rather than falling through. The thumb is not draggable, which is what the browser's bar
already does — there is no cell of it that means a document position, and the wheel and
the keyboard both already scroll.

The editor's thumb is positioned from `viewport.top_line` against the document's line
count, which is exact because the editor does not wrap: one line is one row. An editor
scrolled past its last page — it may scroll until only the last line is left — pins the
thumb at the bottom rather than running off it, since "you are at the end" is what that
state means.
