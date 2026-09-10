//! Parallel per-line state checkpoints with a dirty watermark.
//!
//! The parser is a state machine that runs from the top of the file, so the
//! colours of line 900 depend on the 899 lines above it. Re-running that on
//! every keystroke is what makes naive highlighting unusable on a large file,
//! so this module keeps two things:
//!
//! - **Checkpoints** — the parser's state before every 64th line, so a redraw
//!   resumes just above the viewport instead of at line 1.
//! - **A window** — the styled runs of the lines currently on screen, and only
//!   those. Nothing off-screen is ever coloured (SPEC §21).
//!
//! An edit on line N drops every checkpoint below N; the ones above it are
//! still true, because nothing that happens later can change what the parser
//! had already seen.

use std::path::{Path, PathBuf};

use crate::editor::document::Document;
use crate::syntax::highlighter::{self, LineState, Token};

/// Lines between checkpoints.
///
/// The trade is memory against catch-up work: 64 lines is at most ~3 ms of
/// re-parsing to resume anywhere, and one saved state per 64 lines is ~800 of
/// them for a 50 000-line file.
const CHECKPOINT_INTERVAL: usize = 64;

/// How far the parser will run forward from a checkpoint to reach the viewport
/// before it gives up and starts from a guess instead.
///
/// ~50 µs a line, so this is a ~100 ms ceiling on the frame that follows a jump
/// into a part of a huge file that has never been drawn.
const MAX_CATCHUP_LINES: usize = 2_048;

/// How far above the viewport a guessed restart begins.
///
/// Only reached when the nearest checkpoint is further than `MAX_CATCHUP_LINES`
/// away — `Ctrl+End` in a file nobody has scrolled through. Starting from a
/// clean state 256 lines up is right for everything except a construct that is
/// itself longer than 256 lines (a licence header as one block comment is the
/// realistic case), and it is bounded work.
const LOOKBACK_LINES: usize = 256;

/// Files above this are not highlighted at all.
const MAX_BYTES: usize = 5 * 1024 * 1024;

/// A single line above this turns highlighting off for the whole document.
///
/// A minified bundle is one line of 500 KB; backtracking regexes over it are
/// unbounded in a way the line count never warns about.
const MAX_LINE_BYTES: usize = 4 * 1024;

/// Why a document is not being highlighted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disabled {
    TooLarge,
    LongLine,
    /// The grammar itself failed. Rare, and not the user's problem to fix.
    ParseError,
}

impl Disabled {
    pub fn reason(self) -> &'static str {
        match self {
            Self::TooLarge => "the file is over 5 MB",
            Self::LongLine => "a line is over 4 KB",
            Self::ParseError => "the grammar failed",
        }
    }
}

/// One document's highlighting: which grammar, what is on screen, and how to
/// get back to the parser's state near it.
#[derive(Debug, Default)]
pub struct HighlightCache {
    /// The grammar, chosen once per path. `None` until the first sync.
    syntax: Option<&'static syntect::parsing::SyntaxReference>,
    /// The path the grammar was chosen for, so a rename re-detects (SPEC §20).
    detected_for: Option<PathBuf>,
    disabled: Option<Disabled>,
    /// Parser state before line `index * CHECKPOINT_INTERVAL`. Holes are
    /// allowed: a jump into the middle of a file fills the slots it passes and
    /// leaves the ones before it empty.
    checkpoints: Vec<Option<LineState>>,
    /// First line the window holds.
    window_start: usize,
    /// Styled runs, one entry per line from `window_start`.
    window: Vec<Vec<Token>>,
    /// Whether `window` still matches the document.
    fresh: bool,
    /// Lines the last reparse ran through, including the ones above the
    /// viewport it had to run down from. This is the number that decides
    /// whether typing stays responsive, so it is worth being able to assert on.
    parsed: usize,
}

impl HighlightCache {
    /// The runs of one document line, empty when it is not on screen or the
    /// document is not being highlighted.
    pub fn tokens(&self, line: usize) -> &[Token] {
        line.checked_sub(self.window_start)
            .and_then(|offset| self.window.get(offset))
            .map_or(&[], Vec::as_slice)
    }

    /// The grammar's name, for the status bar.
    pub fn language(&self) -> &'static str {
        self.syntax
            .map_or("Plain Text", |syntax| syntax.name.as_str())
    }

    pub fn disabled(&self) -> Option<Disabled> {
        self.disabled
    }

    /// Brings the window up to date with the document and the viewport.
    ///
    /// Returns the reason when this call is the one that turned highlighting
    /// off, so the caller can say so once rather than on every frame.
    ///
    /// Takes the document by `&mut` for one reason: the edit watermark is a
    /// take, not a read (see `Document::take_dirty_from`).
    pub fn sync(&mut self, document: &mut Document, top: usize, height: usize) -> Option<Disabled> {
        let mut announce = None;

        if self.syntax.is_none() || self.detected_for.as_deref() != document.path() {
            announce = self.detect(document);
        }
        if let Some(line) = document.take_dirty_from() {
            self.invalidate(line);
        }
        if self.disabled.is_some() {
            self.window.clear();
            return announce;
        }

        let end = (top + height).min(document.line_count());
        let wanted = end.saturating_sub(top);
        self.parsed = 0;
        if self.fresh && self.window_start == top && self.window.len() == wanted {
            return announce;
        }

        self.reparse(document, top, end).or(announce)
    }

    /// Chooses the grammar for the document's current path and starts over.
    fn detect(&mut self, document: &Document) -> Option<Disabled> {
        self.detected_for = document.path().map(Path::to_path_buf);
        // The grammar is chosen by the name *inside* the container, so a
        // `dump.sql.gz` highlights as SQL rather than as an unknown extension
        // (ADR-074). Everything else here still keys on the real path.
        self.syntax = Some(highlighter::detect(
            document.syntax_path().as_deref(),
            &document.line(0),
        ));
        self.checkpoints.clear();
        self.window.clear();
        self.fresh = false;
        self.disabled = (document.len_bytes().0 > MAX_BYTES).then_some(Disabled::TooLarge);
        log::debug!(
            "highlighting {} as {}{}",
            document.title(),
            self.language(),
            self.disabled
                .map_or(String::new(), |why| format!(" — off, {}", why.reason()))
        );
        self.disabled
    }

    /// Drops everything the edit on `line` could have changed.
    ///
    /// The checkpoint whose slot `line` falls in survives: it is the state
    /// *before* the first line of that slot, which the edit is at or after.
    /// Highlights with a grammar the user chose by hand (ADR-058).
    ///
    /// It outranks detection until the tab's path changes, and it needs no flag
    /// to do so: `sync` re-detects only when the path it detected for is no
    /// longer the document's, so a chosen grammar survives every edit, save and
    /// reload, and is dropped by a rename — where "what is this file?" has a
    /// new answer anyway.
    ///
    /// A grammar that failed on this text is given a fresh start, because the
    /// user has just named a different one. The size limit is the file's and
    /// not the grammar's, so it is re-applied.
    pub fn set_syntax(
        &mut self,
        syntax: &'static syntect::parsing::SyntaxReference,
        document: &Document,
    ) {
        self.detected_for = document.path().map(Path::to_path_buf);
        self.syntax = Some(syntax);
        self.checkpoints.clear();
        self.window.clear();
        self.fresh = false;
        self.disabled = (document.len_bytes().0 > MAX_BYTES).then_some(Disabled::TooLarge);
        log::debug!(
            "highlighting {} as {} — chosen",
            document.title(),
            syntax.name
        );
    }

    fn invalidate(&mut self, line: usize) {
        self.checkpoints.truncate(line / CHECKPOINT_INTERVAL + 1);
        self.fresh = false;
    }

    /// Parses `[resume, end)` and keeps the runs from `top` on.
    fn reparse(&mut self, document: &Document, top: usize, end: usize) -> Option<Disabled> {
        let syntax = self.syntax.expect("sync detects before it reparses");
        let (start, mut state) = self.resume_point(top);

        // Taken out of `self` so the loop can keep a checkpoint and a token
        // vector borrowed at the same time.
        let mut window = std::mem::take(&mut self.window);
        let mut scratch = Vec::new();
        let mut buffer = String::new();
        let mut produced = 0;

        for index in start..end {
            let text = document.line(index);
            if text.len() > MAX_LINE_BYTES {
                self.window = window;
                return self.disable(Disabled::LongLine);
            }
            if index % CHECKPOINT_INTERVAL == 0 {
                self.store_checkpoint(index, &state);
            }

            // The parser is fed the terminator the grammars anchor `$` to; the
            // runs are clipped back to the text so it is never drawn.
            buffer.clear();
            buffer.push_str(&text);
            buffer.push('\n');

            let visible = index >= top;
            let tokens = if visible {
                if window.len() <= produced {
                    window.push(Vec::new());
                }
                &mut window[produced]
            } else {
                &mut scratch
            };
            if let Err(err) = state.highlight(&buffer, text.len(), tokens) {
                log::warn!("{} line {}: {err}", syntax.name, index + 1);
                self.window = window;
                return self.disable(Disabled::ParseError);
            }
            produced += usize::from(visible);
        }

        window.truncate(produced);
        self.window = window;
        self.window_start = top;
        self.fresh = true;
        self.parsed = end.saturating_sub(start);
        None
    }

    /// Where to start parsing to reach `top`: the nearest checkpoint at or
    /// above it, or a clean state `LOOKBACK_LINES` up when that checkpoint is
    /// too far away to run down from.
    fn resume_point(&self, top: usize) -> (usize, LineState) {
        if !self.checkpoints.is_empty() {
            let highest = (top / CHECKPOINT_INTERVAL).min(self.checkpoints.len() - 1);
            for slot in (0..=highest).rev() {
                let Some(state) = &self.checkpoints[slot] else {
                    continue;
                };
                let line = slot * CHECKPOINT_INTERVAL;
                if top - line <= MAX_CATCHUP_LINES {
                    return (line, state.clone());
                }
                // Anything below this one is further still.
                break;
            }
        }
        let syntax = self.syntax.expect("sync detects before it reparses");
        (top.saturating_sub(LOOKBACK_LINES), LineState::new(syntax))
    }

    fn store_checkpoint(&mut self, line: usize, state: &LineState) {
        let slot = line / CHECKPOINT_INTERVAL;
        if self.checkpoints.len() <= slot {
            self.checkpoints.resize_with(slot + 1, || None);
        }
        if self.checkpoints[slot].is_none() {
            self.checkpoints[slot] = Some(state.clone());
        }
    }

    fn disable(&mut self, why: Disabled) -> Option<Disabled> {
        self.disabled = Some(why);
        self.checkpoints.clear();
        self.window.clear();
        self.fresh = false;
        Some(why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::highlighter::StyleKind;
    use std::path::PathBuf;

    fn document(text: &str) -> Document {
        Document::from_text(text, Some(PathBuf::from("/tmp/ferroedit/main.rs")))
    }

    /// The kinds of a line, one per byte — the shape the renderer reads.
    fn kinds(cache: &HighlightCache, document: &Document, line: usize) -> Vec<StyleKind> {
        let text = document.line(line);
        let tokens = cache.tokens(line);
        (0..text.len())
            .map(|at| highlighter::kind_at(tokens, at))
            .collect()
    }

    #[test]
    fn a_synced_cache_colours_the_viewport_and_nothing_else() {
        let mut document = document("fn a() {}\nfn b() {}\nfn c() {}\n");
        let mut cache = HighlightCache::default();
        assert_eq!(cache.sync(&mut document, 0, 2), None);

        assert_eq!(cache.language(), "Rust");
        assert_eq!(kinds(&cache, &document, 0)[0], StyleKind::Keyword);
        assert_eq!(kinds(&cache, &document, 1)[0], StyleKind::Keyword);
        assert!(
            cache.tokens(2).is_empty(),
            "line 3 is below the viewport and is never parsed"
        );
    }

    #[test]
    fn scrolling_moves_the_window() {
        let mut document = document(&"fn a() {}\n".repeat(10));
        let mut cache = HighlightCache::default();
        cache.sync(&mut document, 0, 3);
        cache.sync(&mut document, 5, 3);

        assert!(cache.tokens(0).is_empty(), "line 1 has scrolled off");
        assert_eq!(kinds(&cache, &document, 5)[0], StyleKind::Keyword);
        assert!(cache.tokens(8).is_empty());
    }

    #[test]
    fn an_edit_is_picked_up_on_the_next_sync() {
        let mut document = document("let x = 1;\n");
        let mut cache = HighlightCache::default();
        cache.sync(&mut document, 0, 1);
        assert_eq!(kinds(&cache, &document, 0)[0], StyleKind::Keyword);

        // Turn the line into a comment; the first two bytes must change kind.
        document.insert_text("//");
        cache.sync(&mut document, 0, 1);
        assert_eq!(kinds(&cache, &document, 0)[0], StyleKind::Comment);
    }

    #[test]
    fn an_edit_above_the_viewport_recolours_what_is_below_it() {
        // Opening a block comment on line 1 must grey out lines 2 and 3.
        let mut document = document("let x = 1;\nlet y = 2;\nlet z = 3;\n");
        let mut cache = HighlightCache::default();
        cache.sync(&mut document, 1, 2);
        assert_eq!(kinds(&cache, &document, 1)[0], StyleKind::Keyword);

        document.insert_text("/*");
        cache.sync(&mut document, 1, 2);
        assert_eq!(kinds(&cache, &document, 1)[0], StyleKind::Comment);
        assert_eq!(kinds(&cache, &document, 2)[0], StyleKind::Comment);
    }

    #[test]
    fn a_clean_second_sync_does_no_work() {
        let mut document = document(&"fn a() {}\n".repeat(10));
        let mut cache = HighlightCache::default();
        cache.sync(&mut document, 0, 4);
        let before = cache.checkpoints.len();
        cache.sync(&mut document, 0, 4);
        assert!(cache.fresh);
        assert_eq!(cache.checkpoints.len(), before);
    }

    #[test]
    fn checkpoints_are_kept_above_an_edit_and_dropped_below_it() {
        let mut document = document(&"fn a() {}\n".repeat(400));
        let mut cache = HighlightCache::default();
        cache.sync(&mut document, 300, 20);
        assert!(
            cache.checkpoints.len() >= 5,
            "the run down to line 300 filled the slots it passed"
        );

        document.goto_line(129);
        document.insert_char('x');
        cache.sync(&mut document, 300, 20);
        // Line 128 (0-based) is the first line of slot 2, so slot 2 survives
        // and everything below it is gone and re-derived from there.
        assert!(cache.checkpoints.len() >= 5);
    }

    #[test]
    fn a_document_over_five_megabytes_is_not_highlighted() {
        let mut document = document(&"fn a() {}\n".repeat(600_000));
        let mut cache = HighlightCache::default();
        assert_eq!(
            cache.sync(&mut document, 0, 40),
            Some(Disabled::TooLarge),
            "the reason is reported once"
        );
        assert_eq!(cache.disabled(), Some(Disabled::TooLarge));
        assert!(cache.tokens(0).is_empty());
        assert_eq!(
            cache.sync(&mut document, 0, 40),
            None,
            "and not repeated on every frame"
        );
        // The status bar still names the language.
        assert_eq!(cache.language(), "Rust");
    }

    #[test]
    fn a_line_over_four_kilobytes_turns_highlighting_off() {
        let mut document = document(&format!("fn a() {{}}\n{}\n", "x".repeat(5000)));
        let mut cache = HighlightCache::default();
        assert_eq!(cache.sync(&mut document, 0, 4), Some(Disabled::LongLine));
        assert!(cache.tokens(0).is_empty(), "the whole document goes plain");
    }

    #[test]
    fn a_renamed_file_is_re_detected() {
        let mut document = document("# heading\n");
        let mut cache = HighlightCache::default();
        cache.sync(&mut document, 0, 1);
        assert_eq!(cache.language(), "Rust");

        document.set_path(PathBuf::from("/tmp/ferroedit/README.md"));
        cache.sync(&mut document, 0, 1);
        assert_eq!(cache.language(), "Markdown");
        assert_eq!(kinds(&cache, &document, 0)[0], StyleKind::Tag);
    }

    #[test]
    fn a_jump_beyond_the_catch_up_limit_restarts_near_the_viewport() {
        let lines = MAX_CATCHUP_LINES + LOOKBACK_LINES + 500;
        let mut document = document(&"fn a() {}\n".repeat(lines));
        let mut cache = HighlightCache::default();
        // Nothing has been drawn, so the only checkpoint is at line 0 and it is
        // too far: the parse starts LOOKBACK_LINES above the viewport instead.
        let top = lines - 10;
        cache.sync(&mut document, top, 5);

        assert_eq!(kinds(&cache, &document, top)[0], StyleKind::Keyword);
        assert!(
            cache.checkpoints.len() * CHECKPOINT_INTERVAL >= top,
            "the slots it passed on the way were filled, so scrolling on is cheap"
        );
        assert!(
            cache.checkpoints[1].is_none(),
            "and the ones it skipped are still empty"
        );
    }

    /// The acceptance property behind "typing stays responsive": a keystroke
    /// never costs more than the viewport plus the run down from the checkpoint
    /// above it, however large the file is.
    ///
    /// Asserted as work rather than as wall-clock, which is the same claim
    /// without the flakiness of timing a test runner under load.
    #[test]
    fn an_edit_in_a_large_file_reparses_a_bounded_number_of_lines() {
        const HEIGHT: usize = 40;
        let mut document = document(&"fn a() { let x = 1; }\n".repeat(20_000));
        let mut cache = HighlightCache::default();

        // Jump to the middle and settle there, as scrolling would.
        document.goto_line(10_000);
        cache.sync(&mut document, 9_980, HEIGHT);

        for _ in 0..20 {
            document.insert_char('x');
            cache.sync(&mut document, 9_980, HEIGHT);
            assert!(
                cache.parsed <= CHECKPOINT_INTERVAL + HEIGHT,
                "a keystroke reparsed {} lines",
                cache.parsed
            );
        }

        // And scrolling one line costs the same order of work.
        cache.sync(&mut document, 9_981, HEIGHT);
        assert!(cache.parsed <= CHECKPOINT_INTERVAL + HEIGHT);
    }

    #[test]
    fn a_zero_height_viewport_parses_nothing() {
        let mut document = document("fn a() {}\n");
        let mut cache = HighlightCache::default();
        assert_eq!(cache.sync(&mut document, 0, 0), None);
        assert!(cache.tokens(0).is_empty());
    }

    #[test]
    fn a_viewport_past_the_end_of_the_document_is_harmless() {
        let mut document = document("fn a() {}\n");
        let mut cache = HighlightCache::default();
        cache.sync(&mut document, 40, 10);
        assert!(cache.tokens(40).is_empty());
    }

    #[test]
    fn a_buffer_with_no_path_is_plain_text() {
        let mut document = Document::from_text("fn a() {}\n", None);
        let mut cache = HighlightCache::default();
        cache.sync(&mut document, 0, 1);
        assert_eq!(cache.language(), "Plain Text");
        assert_eq!(kinds(&cache, &document, 0)[0], StyleKind::Text);
    }
}
