//! Syntax set loading and per-line highlighting.
//!
//! The set is the one `build.rs` dumped: syntect's defaults plus the grammars
//! in `assets/syntaxes/` (ADR-022). It is loaded once for the whole process —
//! it is immutable, several megabytes of context tables, and every open tab
//! would otherwise keep its own copy.
//!
//! Highlighting here stops at a *kind* rather than at a colour: syntect's own
//! themes are truecolor, and ADR-007 says the palette is 256-colour indexed and
//! lives in `Theme`. So the parser's scopes are folded into the small
//! `StyleKind` enum below and `ui/theme.rs` decides what each one looks like.

use std::path::Path;
use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

/// The syntax set, loaded from the dump `build.rs` wrote.
///
/// ~1.5 ms once per process, against ~105 ms to link the same grammars at
/// startup — which is the whole reason the dump exists.
pub fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        let dump = include_bytes!(concat!(env!("OUT_DIR"), "/syntaxes.packdump"));
        syntect::dumps::from_binary(dump)
    })
}

/// What a run of text *is*, as far as the editor's palette is concerned.
///
/// Deliberately coarse. A terminal has 256 colours and a status bar to share
/// them with; distinguishing `storage.modifier` from `keyword.control` would
/// cost a palette entry and buy nothing the eye can use at this size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StyleKind {
    /// Anything the grammar did not name: the theme's ordinary foreground.
    #[default]
    Text,
    Comment,
    String,
    Number,
    Constant,
    Keyword,
    Operator,
    Function,
    Type,
    Variable,
    /// A markup tag, a TOML table header, a Markdown heading.
    Tag,
    /// An attribute name, a link, a key.
    Attribute,
    Punctuation,
    /// The grammar itself says this is wrong — an unterminated string, say.
    Invalid,
}

/// One run of a line that shares a style.
///
/// `end` is a byte offset into the line, and a run starts where the previous
/// one ended — so a line costs one small vector rather than a string per run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub end: usize,
    pub kind: StyleKind,
}

/// Scope prefixes, most specific first, and what each one paints as.
///
/// Order is the whole logic: `constant.character.escape` has to be tested
/// before `constant`, and `keyword.operator` before `keyword`, or the general
/// entry swallows the specific one.
const SCOPE_KINDS: &[(&str, StyleKind)] = &[
    ("comment", StyleKind::Comment),
    ("constant.character.escape", StyleKind::Constant),
    ("constant.numeric", StyleKind::Number),
    ("string", StyleKind::String),
    ("markup.raw", StyleKind::String),
    ("constant", StyleKind::Constant),
    ("keyword.operator", StyleKind::Operator),
    ("keyword", StyleKind::Keyword),
    ("storage", StyleKind::Keyword),
    ("entity.name.function", StyleKind::Function),
    ("support.function", StyleKind::Function),
    ("entity.name.type", StyleKind::Type),
    ("entity.name.class", StyleKind::Type),
    ("entity.name.struct", StyleKind::Type),
    ("entity.name.enum", StyleKind::Type),
    ("entity.name.namespace", StyleKind::Type),
    ("support.type", StyleKind::Type),
    ("support.class", StyleKind::Type),
    ("support.constant", StyleKind::Constant),
    ("entity.name.tag", StyleKind::Tag),
    ("entity.name.section", StyleKind::Tag),
    ("markup.heading", StyleKind::Tag),
    ("entity.other.attribute-name", StyleKind::Attribute),
    ("entity.other.inherited-class", StyleKind::Type),
    ("markup.underline.link", StyleKind::Attribute),
    ("markup.list", StyleKind::Punctuation),
    ("markup.bold", StyleKind::Type),
    ("markup.italic", StyleKind::Variable),
    ("variable.function", StyleKind::Function),
    ("variable", StyleKind::Variable),
    ("punctuation", StyleKind::Punctuation),
    ("meta.separator", StyleKind::Punctuation),
    ("invalid", StyleKind::Invalid),
];

/// `SCOPE_KINDS` with the scopes parsed, built once.
fn scope_kinds() -> &'static [(Scope, StyleKind)] {
    static KINDS: OnceLock<Vec<(Scope, StyleKind)>> = OnceLock::new();
    KINDS.get_or_init(|| {
        SCOPE_KINDS
            .iter()
            .map(|(name, kind)| {
                (
                    Scope::new(name).expect("every scope in SCOPE_KINDS is well-formed"),
                    *kind,
                )
            })
            .collect()
    })
}

/// The kind a scope stack paints as.
///
/// The *table's* order decides, not the stack's depth: a `//` carries both
/// `comment.line` and `punctuation.definition.comment`, and painting it as
/// punctuation because that scope sits higher on the stack would give a comment
/// two colours. Reading the table in order instead means "a comment is a
/// comment, whatever the delimiter is scoped as" — which is why `comment` and
/// `string` are at the top of `SCOPE_KINDS` and `punctuation` near the bottom.
fn kind_of(stack: &ScopeStack) -> StyleKind {
    for (prefix, kind) in scope_kinds() {
        if stack.scopes.iter().any(|scope| prefix.is_prefix_of(*scope)) {
            return *kind;
        }
    }
    StyleKind::Text
}

/// Extensions syntect's set has no grammar for, and the one to use instead.
///
/// TypeScript and JSX are supersets of JavaScript, so the JavaScript grammar
/// colours the parts they share — which is most of a file — and leaves type
/// annotations and JSX tags as plain text rather than as nothing at all
/// (ADR-022).
const ALIASES: &[(&str, &str)] = &[
    ("ts", "js"),
    ("mts", "js"),
    ("cts", "js"),
    ("tsx", "js"),
    ("jsx", "js"),
    ("mjs", "js"),
    ("cjs", "js"),
];

/// The grammar for a file, by name first and extension second, falling back to
/// its first line and then to plain text.
///
/// The file *name* is tried first because it is the more specific of the two:
/// `Dockerfile` and `Makefile` have no extension, and `Cargo.lock`'s extension
/// (`lock`) says nothing while its full name says TOML (SPEC §21).
pub fn detect(path: Option<&Path>, first_line: &str) -> &'static SyntaxReference {
    let set = syntax_set();
    let name = path.and_then(|p| p.file_name()).and_then(|n| n.to_str());
    let extension = path.and_then(|p| p.extension()).and_then(|e| e.to_str());

    let by_name = name.and_then(|name| set.find_syntax_by_extension(name));
    let by_extension = extension.and_then(|extension| {
        set.find_syntax_by_extension(extension).or_else(|| {
            ALIASES
                .iter()
                .find(|(from, _)| *from == extension)
                .and_then(|(_, to)| set.find_syntax_by_extension(to))
        })
    });

    by_name
        .or(by_extension)
        // Only worth asking when nothing else knew: it runs a regex per
        // grammar, and a `#!/usr/bin/env python3` is the case it exists for.
        .or_else(|| set.find_syntax_by_first_line(first_line))
        .unwrap_or_else(|| set.find_syntax_plain_text())
}

/// The parser's position between two lines: everything needed to resume.
///
/// Cloning one is ~1.5 µs, which is what makes the checkpoint cache in
/// `cache.rs` affordable.
#[derive(Debug, Clone)]
pub struct LineState {
    parse: ParseState,
    stack: ScopeStack,
}

impl LineState {
    pub fn new(syntax: &SyntaxReference) -> Self {
        Self {
            parse: ParseState::new(syntax),
            stack: ScopeStack::new(),
        }
    }

    /// Parses one line and writes its runs into `tokens`, advancing the state
    /// onto the next line.
    ///
    /// `line` must carry its `\n`: the set is the newlines variant, and a
    /// grammar's `$` rules only match when the terminator is there. The tokens
    /// are clipped to `text_len` so the newline itself never becomes a run.
    ///
    /// A grammar that fails mid-line leaves the state unusable, so the error is
    /// passed up and the caller stops highlighting the document.
    pub fn highlight(
        &mut self,
        line: &str,
        text_len: usize,
        tokens: &mut Vec<Token>,
    ) -> Result<(), syntect::Error> {
        tokens.clear();
        let ops = self.parse.parse_line(line, syntax_set())?;

        let mut start = 0;
        let mut kind = kind_of(&self.stack);
        for (offset, op) in &ops {
            push_run(tokens, start.max(*offset), kind, text_len);
            self.stack.apply(op)?;
            kind = kind_of(&self.stack);
            start = *offset;
        }
        push_run(tokens, text_len, kind, text_len);
        Ok(())
    }
}

/// Appends a run ending at `end`, merging it into the previous one when they
/// share a kind — which most of a line's runs do, because a scope stack changes
/// far more often than the handful of kinds it folds into.
///
/// Empty runs are dropped rather than stored: the parser emits a stack
/// operation at offset 0 on most lines, and a `0..0` run would be one vector
/// entry per line that no renderer can ever draw.
fn push_run(tokens: &mut Vec<Token>, end: usize, kind: StyleKind, text_len: usize) {
    let end = end.min(text_len);
    match tokens.last_mut() {
        _ if end == 0 => {}
        Some(last) if last.end >= end => {}
        Some(last) if last.kind == kind => last.end = end,
        _ => tokens.push(Token { end, kind }),
    }
}

/// The kind at a byte offset, for a line's tokens. `O(runs)`, and a line has
/// few.
pub fn kind_at(tokens: &[Token], byte: usize) -> StyleKind {
    tokens
        .iter()
        .find(|token| byte < token.end)
        .map_or(StyleKind::Text, |token| token.kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A line's runs as `(text, kind)` pairs — what the renderer will draw.
    fn runs(syntax_name: &str, line: &str) -> Vec<(String, StyleKind)> {
        let syntax = syntax_set()
            .find_syntax_by_name(syntax_name)
            .expect("the test names a grammar that is in the set");
        let mut state = LineState::new(syntax);
        let mut tokens = Vec::new();
        let with_newline = format!("{line}\n");
        state
            .highlight(&with_newline, line.len(), &mut tokens)
            .expect("the fixtures parse");
        let mut out = Vec::new();
        let mut start = 0;
        for token in &tokens {
            out.push((line[start..token.end].to_string(), token.kind));
            start = token.end;
        }
        out
    }

    #[test]
    fn the_dump_has_every_language_the_spec_asks_for() {
        // SPEC §21, by the extension a user would actually open.
        for (extension, expected) in [
            ("rs", "Rust"),
            ("py", "Python"),
            ("go", "Go"),
            ("js", "JavaScript"),
            ("ts", "JavaScript"),
            ("jsx", "JavaScript"),
            ("tsx", "JavaScript"),
            ("json", "JSON"),
            ("yaml", "YAML"),
            ("toml", "TOML"),
            ("md", "Markdown"),
            ("html", "HTML"),
            ("css", "CSS"),
            ("sh", "Bourne Again Shell (bash)"),
        ] {
            let path = PathBuf::from(format!("file.{extension}"));
            assert_eq!(
                detect(Some(&path), "").name,
                expected,
                ".{extension} must highlight"
            );
        }
        assert_eq!(detect(Some(Path::new("Dockerfile")), "").name, "Dockerfile");
    }

    #[test]
    fn a_file_name_beats_an_extension() {
        // `lock` is not a language; `Cargo.lock` is TOML.
        assert_eq!(detect(Some(Path::new("Cargo.lock")), "").name, "TOML");
        assert_eq!(detect(Some(Path::new("Makefile")), "").name, "Makefile");
    }

    #[test]
    fn an_unknown_extension_falls_back_to_the_first_line() {
        let path = PathBuf::from("deploy");
        assert_eq!(detect(Some(&path), "#!/usr/bin/env python3").name, "Python");
    }

    #[test]
    fn a_buffer_with_no_path_is_plain_text() {
        assert_eq!(detect(None, "fn main() {}").name, "Plain Text");
        assert_eq!(detect(Some(Path::new("notes.qqq")), "").name, "Plain Text");
    }

    #[test]
    fn rust_splits_into_keyword_function_and_string() {
        assert_eq!(
            runs("Rust", "fn main() { \"hi\" }"),
            vec![
                ("fn".into(), StyleKind::Keyword),
                (" ".into(), StyleKind::Text),
                ("main".into(), StyleKind::Function),
                ("()".into(), StyleKind::Punctuation),
                (" ".into(), StyleKind::Text),
                ("{".into(), StyleKind::Punctuation),
                (" ".into(), StyleKind::Text),
                // The quotes are part of the string, not punctuation of their
                // own: that is the table-order rule in `kind_of`.
                ("\"hi\"".into(), StyleKind::String),
                (" ".into(), StyleKind::Text),
                ("}".into(), StyleKind::Punctuation),
            ]
        );
    }

    #[test]
    fn a_comment_is_one_run_to_the_end_of_the_line() {
        let runs = runs("Rust", "let x = 1; // why");
        let comment = runs
            .iter()
            .find(|(_, kind)| *kind == StyleKind::Comment)
            .expect("the comment is highlighted");
        assert_eq!(comment.0, "// why");
    }

    #[test]
    fn the_newline_is_never_part_of_a_run() {
        let syntax = syntax_set().find_syntax_by_name("Rust").unwrap();
        let mut state = LineState::new(syntax);
        let mut tokens = Vec::new();
        state.highlight("// x\n", 4, &mut tokens).unwrap();
        assert_eq!(tokens.last().map(|t| t.end), Some(4));
    }

    #[test]
    fn an_empty_line_produces_no_runs_at_all() {
        let syntax = syntax_set().find_syntax_by_name("Rust").unwrap();
        let mut state = LineState::new(syntax);
        let mut tokens = Vec::new();
        state.highlight("\n", 0, &mut tokens).unwrap();
        assert!(tokens.is_empty(), "there is nothing to paint");
    }

    #[test]
    fn a_block_comment_carries_across_lines() {
        let syntax = syntax_set().find_syntax_by_name("Rust").unwrap();
        let mut state = LineState::new(syntax);
        let mut tokens = Vec::new();
        state.highlight("/* open\n", 7, &mut tokens).unwrap();
        state.highlight("still\n", 5, &mut tokens).unwrap();
        assert_eq!(
            tokens,
            vec![Token {
                end: 5,
                kind: StyleKind::Comment
            }],
            "the second line is inside the comment the first one opened"
        );
    }

    #[test]
    fn the_hand_written_toml_grammar_colours_a_cargo_manifest() {
        assert_eq!(
            runs("TOML", "[package]"),
            vec![
                ("[".into(), StyleKind::Punctuation),
                ("package".into(), StyleKind::Tag),
                ("]".into(), StyleKind::Punctuation),
            ]
        );
        assert_eq!(
            runs("TOML", "name = \"ferroedit\""),
            vec![
                ("name".into(), StyleKind::Tag),
                (" ".into(), StyleKind::Text),
                ("=".into(), StyleKind::Operator),
                (" ".into(), StyleKind::Text),
                ("\"ferroedit\"".into(), StyleKind::String),
            ]
        );
        let number = runs("TOML", "port = 8080");
        assert!(number.contains(&("8080".into(), StyleKind::Number)));
    }

    #[test]
    fn the_hand_written_dockerfile_grammar_colours_instructions() {
        let line = runs("Dockerfile", "FROM rust:1.75 AS build");
        assert_eq!(line[0], ("FROM".into(), StyleKind::Keyword));
        assert!(line.contains(&("AS".into(), StyleKind::Operator)));
    }

    #[test]
    fn kind_at_reads_the_run_a_byte_falls_in() {
        let tokens = [
            Token {
                end: 2,
                kind: StyleKind::Keyword,
            },
            Token {
                end: 5,
                kind: StyleKind::Text,
            },
        ];
        assert_eq!(kind_at(&tokens, 0), StyleKind::Keyword);
        assert_eq!(kind_at(&tokens, 1), StyleKind::Keyword);
        assert_eq!(kind_at(&tokens, 2), StyleKind::Text);
        assert_eq!(kind_at(&tokens, 4), StyleKind::Text);
        // Past the end of the line, and for a line with no tokens at all.
        assert_eq!(kind_at(&tokens, 9), StyleKind::Text);
        assert_eq!(kind_at(&[], 0), StyleKind::Text);
    }
}
