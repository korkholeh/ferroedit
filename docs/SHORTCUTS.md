# Keyboard shortcuts

<!-- GENERATED FILE. Do not edit by hand.
     Rendered from `src/event/keyboard.rs` and `src/commands/mod.rs` by
     `src/docs.rs`; run `ferroedit --dump-shortcuts > docs/SHORTCUTS.md`, or
     `FERROEDIT_UPDATE_DOCS=1 cargo test`, after changing a binding (ADR-028). -->

A binding is scoped to the pane that has focus, so every table below says where its
keys work. A key with no entry for the focused pane falls through to the *Anywhere*
table; a key with no entry there does nothing.

## Anywhere

These resolve whatever has focus, except behind a dialog: a modal window binds its own keys and nothing else (SPEC §40).

| Keys | Action |
|---|---|
| `Ctrl+Q` | Quit, asking first when a tab has unsaved changes |
| `F1` | Show the keyboard shortcuts |
| `F6` | Cycle focus: editor → explorer → git panel |
| `F10` | Open the menu bar |
| `Ctrl+B` | Switch the sidebar between explorer and git |
| `Ctrl+Tab` / `Ctrl+PageDown` | Next tab |
| `Ctrl+Shift+Tab` / `Ctrl+PageUp` | Previous tab |
| `Ctrl+W` | Close the active tab, asking first when it is modified |
| `Ctrl+S` | Save the active file |
| `Ctrl+N` | New file — asks for a name |
| `Ctrl+O` | Open a file or a folder — browses for one |
| `Ctrl+F` | Open the find bar, seeded with the selection |
| `Ctrl+H` | Open the find bar with its replacement row |
| `Ctrl+G` | Go to a line — asks for the number |
| `Alt+Z` | Wrap long lines, or stop wrapping them |
| `F3` | Next match |
| `Shift+F3` | Previous match |

## Editor

The document pane. Every motion here also has a `Shift` form that extends the selection instead of moving the cursor.

| Keys | Action |
|---|---|
| `Esc` | Close the find bar |
| `Left` | Move the cursor left |
| `Right` | Move the cursor right |
| `Up` | Move the cursor up |
| `Down` | Move the cursor down |
| `Home` | Move the cursor to the start of the line |
| `End` | Move the cursor to the end of the line |
| `PageUp` | Move the cursor one page up |
| `PageDown` | Move the cursor one page down |
| `Ctrl+Left` | Move the cursor one word left |
| `Ctrl+Right` | Move the cursor one word right |
| `Ctrl+Home` | Move the cursor to the start of the document |
| `Ctrl+End` | Move the cursor to the end of the document |
| `Shift+Left` | Extend the selection left |
| `Shift+Right` | Extend the selection right |
| `Shift+Up` | Extend the selection up |
| `Shift+Down` | Extend the selection down |
| `Shift+Home` | Extend the selection to the start of the line |
| `Shift+End` | Extend the selection to the end of the line |
| `Shift+PageUp` | Extend the selection one page up |
| `Shift+PageDown` | Extend the selection one page down |
| `Ctrl+Shift+Left` | Extend the selection one word left |
| `Ctrl+Shift+Right` | Extend the selection one word right |
| `Ctrl+Shift+Home` | Extend the selection to the start of the document |
| `Ctrl+Shift+End` | Extend the selection to the end of the document |
| `Alt+Left` | Scroll the editor left |
| `Alt+Right` | Scroll the editor right |
| `Ctrl+A` | Select all |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |
| `Ctrl+C` | Copy the selection |
| `Ctrl+X` | Cut the selection |
| `Ctrl+V` | Paste |
| `Enter` | Insert a newline |
| `Backspace` | Delete the cluster before the cursor |
| `Delete` | Delete the cluster after the cursor |
| `Ctrl+D` | Delete the whole line, or every selected line |
| `Tab` | Insert a tab |
| `F5` | Re-read the active file from disk |
| `F4` | Show this file as a table, or as text |
| `F2` | Type into the selected cell |
| `Insert` | Add a row under this one |

## Explorer

The file tree (SPEC §18, §20). The operations act on the selected row, not on the file being edited.

| Keys | Action |
|---|---|
| `Up` | Move the sidebar selection up |
| `Down` | Move the sidebar selection down |
| `Enter` | Open the file, or fold the directory |
| `Right` | Expand the directory, or step into an open one |
| `Left` | Collapse the directory, or step out to its parent |
| `F2` | Rename what is selected in the explorer |
| `Delete` | Delete what is selected in the explorer (asks first) |
| `F5` | Re-read the tree from disk |

## Git panel

The changed-files list (SPEC §31, §33, §35). Everything that writes to the repository runs on a worker thread, so none of these keys blocks a frame (ADR-033).

| Keys | Action |
|---|---|
| `Up` | Move the sidebar selection up |
| `Down` | Move the sidebar selection down |
| `F5` | Re-read the repository status |
| `Esc` | Stop the running git operation |
| `Enter` | Open the selected changed file |
| `Space` | Stage the selected file, or unstage it when it is staged |
| `a` | Stage every change |
| `u` | Unstage every change |
| `c` | Commit what is staged — asks for a message |
| `b` | Switch branch — opens a picker |
| `m` | Merge a branch — opens a picker |
| `d` | Show the diff of the selected file |
| `l` | Show the repository's history |
| `h` | Show the history of this file |
| `g` | Open .git/config in a tab |

## Diff viewer

The read-only unified diff (SPEC §36). It opens in a tab of its own, beside the files being edited, and closes like one — so its keys are a pager's and nothing here types.

| Keys | Action |
|---|---|
| `Esc` | Close the diff tab |
| `Up` | Scroll up a line |
| `Down` | Scroll down a line |
| `PageUp` | Scroll up a page |
| `PageDown` / `Space` | Scroll down a page |
| `Home` | Go to the first line |
| `End` | Go to the last line |
| `Left` | Scroll left |
| `Right` | Scroll right |
| `F5` | Re-read the diff |
| `s` | Show the other side: staged or unstaged |

## Log viewer

A commit history — the repository's, one file's, or one range of lines' (ADR-068). It opens in a tab of its own beside the diffs, and it is a *list*: the arrows move a selection, and `Enter` opens the commit it is on as a diff.

| Keys | Action |
|---|---|
| `Esc` | Close the log tab |
| `Up` | Select the previous commit |
| `Down` | Select the next commit |
| `PageUp` | Move up a page |
| `PageDown` | Move down a page |
| `Home` | Select the newest commit |
| `End` | Select the oldest commit listed |
| `Enter` / `d` | Show the selected commit as a diff |
| `F5` | Re-read the history |
| `/` | Search the history |

## Image viewer

A PNG or a JPEG, drawn in coloured half-block characters — two rows of pixels to a terminal cell (ADR-078). It opens in a tab of its own beside the files being edited, and it is a *window*: the arrows move the window over the picture and the zoom keys change how much of it fits.

| Keys | Action |
|---|---|
| `Esc` | Close the image tab |
| `Up` | Pan up |
| `Down` | Pan down |
| `Left` | Pan left |
| `Right` | Pan right |
| `PageUp` | Pan up a pane |
| `PageDown` / `Space` | Pan down a pane |
| `Home` | Centre the image |
| `+` / `=` | Zoom in |
| `-` / `_` | Zoom out |
| `0` | Fit the whole image in the pane |
| `1` | Zoom to actual size |
| `m` | Show or hide the metadata column |
| `F5` | Re-read the image from disk |

## Log search field

The field `/` opens over a history. Typing narrows the commits already read, which is instant; `Enter` hands the text to `git log --grep`, which searches the whole message and the whole history.

| Keys | Action |
|---|---|
| `Esc` | Close the search field and show everything |
| `Enter` | Ask git to search the whole history |
| `Backspace` | Delete the cluster before the caret |
| `Delete` | Delete the cluster after the caret |
| `Left` | Move the caret left |
| `Right` | Move the caret right |
| `Home` | Move the caret to the start |
| `End` | Move the caret to the end |
| `Up` | Select the previous commit |
| `Down` | Select the next commit |
| `PageUp` | Move up a page |
| `PageDown` | Move down a page |

## Help screen

This screen (SPEC §6). A pager over the same tables, so its keys are the diff viewer's — two read-only panes that scrolled differently would be two things to remember instead of one.

| Keys | Action |
|---|---|
| `Esc` / `q` / `F1` | Close the help screen |
| `Up` | Scroll up a line |
| `Down` | Scroll down a line |
| `PageUp` | Scroll up a page |
| `PageDown` / `Space` | Scroll down a page |
| `Home` | Go to the first line |
| `End` | Go to the last line |

## Find bar

Open from `Ctrl+F` or `Ctrl+H`, and *not* modal: `Ctrl+S` still saves and the menu still opens while the caret is in it.

| Keys | Action |
|---|---|
| `Esc` | Close the find bar |
| `Enter` / `Down` | Next match |
| `Shift+Enter` / `Up` | Previous match |
| `Tab` / `Shift+Tab` | Move between the find and replace fields |
| `Left` | Move the caret left |
| `Right` | Move the caret right |
| `Home` | Move the caret to the start |
| `End` | Move the caret to the end |
| `Backspace` | Delete the cluster before the caret |
| `Delete` | Delete the cluster after the caret |
| `Alt+C` | Toggle match case |
| `Alt+R` | Replace the current match |
| `Alt+A` | Replace every match, as one undo step |

## Menu

While a menu is open. The items themselves are the menu bar's own, along the top of the screen.

| Keys | Action |
|---|---|
| `Esc` | Close the menu |
| `Left` | Previous menu |
| `Right` | Next menu |
| `Up` | Previous item |
| `Down` | Next item |
| `Enter` | Activate the item |

## Dialog

A message and a row of buttons, or a list to choose from (SPEC §33). `Left` and `Right` are the button row's axis and `Up` and `Down` are the list's; a dialog with no list ignores the latter two.

| Keys | Action |
|---|---|
| `Left` | Move back along the button row |
| `Right` / `Tab` | Move along the button row |
| `Up` | Move up the list |
| `Down` | Move down the list |
| `Enter` | Activate the selected button |
| `Esc` | Dismiss the dialog |

## Dialog with a text field

A dialog that asks for a name, and the two whose field filters a list under it: the Open browser (ADR-051) and the syntax and encoding pickers (ADR-058, ADR-059). These replace the plain dialog's keys while the field is open, which is why `Left` moves a caret here and a button selection there (ADR-019); `Up` and `Down` are still the list's, since the caret has no use for them.

| Keys | Action |
|---|---|
| `Left` | Move the caret left |
| `Right` | Move the caret right |
| `Home` | Move the caret to the start |
| `End` | Move the caret to the end |
| `Backspace` | Delete the cluster before the caret |
| `Delete` | Delete the cluster after the caret |
| `Up` | Move up the list |
| `Down` | Move down the list |
| `Tab` | Move along the button row |
| `Enter` | Activate the selected button |
| `Esc` | Dismiss the dialog |

## Typing

Printable characters are not in the tables — there is one row per binding and a million printable characters — but they resolve to commands like everything else (SPEC §25):

| Where | Action |
|---|---|
| Editor | Insert the character at the cursor |
| Find bar | Type into the field with the caret |
| Dialog with a text field | Type into the field |

A `Ctrl` or `Alt` modifier that is not bound types nothing: an unbound combination is a command that does not exist, not a letter.

## Menu bar

Every item is a command, and the *Shortcut* column is the same lookup the menu itself renders from, so an entry cannot advertise a key that is not bound (ADR-008). *Where* is the pane that key works in: an item reachable from any menu may have a key that is not — `Alt+C` needs the find bar, and `F2` the explorer.

| Menu | Item | Shortcut | Where |
|---|---|---|---|
| File | New File | `Ctrl+N` | anywhere |
| File | New Folder | — | — |
| File | Open… | `Ctrl+O` | anywhere |
| File | Save | `Ctrl+S` | anywhere |
| File | Save As… | — | — |
| File | Reload | `F5` | Editor |
| File | Rename… | `F2` | Explorer |
| File | Delete | `Delete` | Explorer |
| File | Close Tab | `Ctrl+W` | anywhere |
| File | Quit | `Ctrl+Q` | anywhere |
| Edit | Undo | `Ctrl+Z` | Editor |
| Edit | Redo | `Ctrl+Y` | Editor |
| Edit | Cut | `Ctrl+X` | Editor |
| Edit | Copy | `Ctrl+C` | Editor |
| Edit | Paste | `Ctrl+V` | Editor |
| Edit | Delete Line | `Ctrl+D` | Editor |
| Edit | Edit Cell | `F2` | Editor |
| Edit | Insert Row | `Insert` | Editor |
| Edit | Delete Row | — | — |
| Selection | Select All | `Ctrl+A` | Editor |
| Selection | Select Row | — | — |
| Selection | Select Column | — | — |
| Search | Find… | `Ctrl+F` | anywhere |
| Search | Replace… | `Ctrl+H` | anywhere |
| Search | Find Next | `F3` | anywhere |
| Search | Find Previous | `Shift+F3` | anywhere |
| Search | Go to Line… | `Ctrl+G` | anywhere |
| Search | Match Case | `Alt+C` | Find bar |
| Search | Replace Match | `Alt+R` | Find bar |
| Search | Replace All | `Alt+A` | Find bar |
| View | Toggle Sidebar | `Ctrl+B` | anywhere |
| View | Refresh Explorer | `F5` | Explorer |
| View | Hidden and Ignored Files | — | — |
| View | Word Wrap | `Alt+Z` | anywhere |
| View | Syntax Mode… | — | — |
| View | Line Endings… | — | — |
| View | Encoding… | — | — |
| View | Table View | `F4` | Editor |
| View | Column Delimiter… | — | — |
| View | Quote Character… | — | — |
| View | Scroll Left | `Alt+Left` | Editor |
| View | Scroll Right | `Alt+Right` | Editor |
| View | Focus Explorer | — | — |
| View | Focus Git | — | — |
| View | Focus Editor | — | — |
| View | Theme: Dark | — | — |
| View | Theme: Light | — | — |
| View | Theme: Dark Simple | — | — |
| View | Theme: Light Simple | — | — |
| View | Theme: Retro | — | — |
| Git | Refresh | `F5` | Git panel |
| Git | Stage | — | — |
| Git | Unstage | — | — |
| Git | Stage All | — | — |
| Git | Unstage All | — | — |
| Git | Commit… | — | — |
| Git | Pull | — | — |
| Git | Push | — | — |
| Git | Cancel | `Esc` | Git panel |
| Git | Branch… | — | — |
| Git | New Branch… | — | — |
| Git | Merge… | — | — |
| Git | Diff | — | — |
| Git | Log | — | — |
| Git | File History | — | — |
| Git | Line History | — | — |
| Git | Config | — | — |
| Help | Shortcuts | `F1` | anywhere |
| Help | About | — | — |
| Help | Check for Updates | — | — |
| Help | Check on Start | — | — |

## Mouse

Clicking a row of a dialog's list selects it without choosing it — unlike the explorer's rows (ADR-020), the confirm button is right there and choosing a branch by accident is a checkout. The Open browser is the exception: a click on the row that is *already* selected walks into it, so browsing costs one click a step, and the wheel over its rows moves the selection through them (ADR-051). Clicking in the editor places the cursor, dragging selects, and a double-click selects the word under the pointer. Over a table (SPEC §65) a click selects the cell instead, dragging selects a block of cells, a click on a column's name takes the whole column and one on a record's number takes the whole record; clicking away from a cell being typed into saves it, and the wheel scrolls the grid while `Shift`+wheel moves it sideways. Over a picture (ADR-078) the wheel pans it, `Shift`+wheel pans it sideways, `Ctrl`+wheel zooms about the middle of the pane, and dragging moves the picture under the pointer. The wheel scrolls the pane under the pointer without moving the cursor. Clicking a tab switches to it; clicking its `×`, or middle-clicking it, closes it. When there are more tabs than fit, the wheel over the tab strip scrolls it sideways and its `‹` and `›` arrows step it one tab at a time. Clicking a row in the explorer opens the file or folds the directory in one click (ADR-020). Clicking a row of the find bar puts the caret in that row's field, and its `[Aa]`, `[Replace]` and `[All]` are clickable — the fallback for terminals that swallow `Alt`. On the status bar, most of the right-hand readout is questions (ADR-058): `Ln 12, Col 8` opens Go to Line, the charset name the encoding picker (ADR-059), `LF` / `CRLF` the line-ending picker, the grammar's name the syntax picker, the branch name the branch picker, and — while a table is showing — `Delim` and `Quote` the two pickers that say how the file is split into columns. Each is a menu entry too, so none of them needs a mouse. What is left — the notification, the focus label, the selection count — reports where you already are and stays inert.

## Terminal limits

- `Ctrl+Z` is undo, not suspend: raw mode disables `ISIG`, so it arrives as a key event.
  There is no suspend-to-shell in the MVP.
- `Ctrl+Q` needs `IXON` off, which raw mode already handles.
- `Ctrl+Shift+Z` and `Ctrl+Shift+S` are deliberately unbound: a terminal without the
  kitty keyboard protocol cannot tell either from its unshifted form, and a key that
  redoes in one terminal and undoes in another is worse than one that always works
  (ADR-008). Redo is `Ctrl+Y`; Save As is a menu entry.
- `Ctrl+Tab` and `Ctrl+Shift+Tab` are not deliverable by every terminal, so
  `Ctrl+PageDown` and `Ctrl+PageUp` are bound to the same commands rather than as
  second-class fallbacks.
- `Alt` is not delivered at all by macOS Terminal.app without "Use Option as Meta";
  every `Alt` binding is also a menu item and a clickable label.
- `Cmd+…` is out of MVP scope (SPEC §2.2).
