# FerroEdit — Roadmap

Phases are ordered by dependency, not by importance. Each phase ends green on
`cargo fmt --check && cargo clippy -- -D warnings && cargo test`, with
`docs/PROGRESS.md` updated. The detailed reasoning behind the ordering lives in
`docs/PLAN.md` §5.

## MVP

| Phase | Scope | Acceptance | Status |
|---|---|---|---|
| 0 | Bootstrap: cargo project, module skeleton, lint config, CI, 4-target release stub, docs | All four targets link; CI green | ✅ done |
| 1 | TUI shell: `TerminalGuard`, panic hook, input thread, event loop, five-zone layout, mouse capture | `Ctrl+Q` exits; terminal restored after quit *and* panic; resize to 60×20 | ✅ done |
| 2 | Editor core: `coords.rs` + Unicode fixtures, `Document`, `Cursor`, viewport, basic editing, open/save with line-ending preservation | `ferroedit test.txt` edits and saves; §48 fixtures pass | ✅ done |
| 3 | Selection + clipboard: Shift+navigation, `Ctrl+A`, mouse drag and double-click, OSC52, bracketed paste | Cut/copy/paste round-trips; drag-select correct across wide chars | ✅ done |
| 4 | Undo/redo: transactions, coalescing, cursor restore | A typed word undoes as a word; memory flat over 100k keystrokes | ✅ done |
| 5 | Tabs: switching, dirty markers, close-with-confirm, reuse existing tab | 10+ files open and switch by mouse and keyboard | ✅ done |
| 6 | Explorer: lazy `ignore`-based tree, expand/collapse, file operations | `ferroedit .` navigates a real project; ignored files hidden | ✅ done |
| 7 | Syntax highlighting: build-time syntax dump, checkpoint cache, big-file fallback | SPEC §21 languages highlight; a 5 MB file stays responsive | ✅ done |
| 8 | Search/replace: `Ctrl+F`/`Ctrl+H`, next/prev, case toggle, replace-all as one undo | Replace-all undoes in a single step | ✅ done |
| 9 | Menus + command wiring: full `Command` enum, keymap table, generated `SHORTCUTS.md` | Every menu item and shortcut resolves to a `Command` | ✅ done |
| 10 | Git status: `GitService`, porcelain=v2 parser, sidebar, non-repo state | Status matches `git status` on fixture repos | ✅ done |
| 11 | Git actions: worker thread, stage/unstage, commit, pull, push | A push never blocks the UI; prompt failures are actionable | ✅ done |
| 12 | Branches + merge: picker, switch, create, merge, conflict display | A branch switches from the picker; a conflicted merge is finished in the editor | ✅ done |
| 13 | Diff viewer: read-only unified diff with +/− coloring | `d` in the panel shows the selected file's diff, worktree or staged, and follows it as it changes | ✅ done |
| 14 | Polish: help screen, small-terminal layout, error pass, profiling, release binaries | SPEC §60 acceptance list passes end to end | 🔨 in progress — help screen and responsive status bar done |

## Explicitly out of MVP scope

Windows support, LSP, tree-sitter, command palette, split editor, integrated terminal,
git history/blame/stash, formatters, plugin API. The architecture should not *prevent*
these (SPEC §62), but none of them is built now.
