# Test fixtures

## `unicode.txt`

The SPEC §48 fixture, and the thing `src/editor/coords.rs` is tested against. Eight
lines, each chosen for a different way that "byte, character, and terminal column" come
apart:

| Line | Content | What it catches |
|---|---|---|
| 1 | `Hello` | The trivial case — one byte, one char, one cell. |
| 2 | `Привіт` | Two bytes per character: a byte index is not a char index. |
| 3 | `Україна` | The same again, with a different letter set. |
| 4 | `日本語` | Two *cells* per character: a char index is not a column. |
| 5 | `🙂` | A non-BMP scalar: four bytes, one char, two cells. |
| 6 | `👨‍👩‍👧` | A ZWJ sequence: five chars, one cursor stop. |
| 7 | `é` | Precomposed U+00E9. |
| 8 | `é` | Decomposed `e` + U+0301: two chars, one cluster, one cell. |

Lines 7 and 8 look identical and are deliberately not the same bytes — they are what
proves that movement steps by grapheme cluster rather than by char (ADR-004).

The file is `include_str!`'d by the `coords` and `document` tests, so it needs no
filesystem access to be used, and it is written back byte for byte by the save test.
Keep it LF-terminated with a final newline; a CRLF checkout would change what those
tests mean.
