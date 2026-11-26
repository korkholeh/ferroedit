//! Markdown, read into blocks for the preview (SPEC §72, ADR-086).
//!
//! The parse is `pulldown-cmark`'s: CommonMark plus the GitHub extensions a
//! README actually uses — tables, task lists, strikethrough, footnotes and the
//! `> [!NOTE]` alerts. What this module adds is a *shape*: the event stream is
//! folded into a tree of `Block`s whose text is a list of styled `Run`s, which
//! is the form a layout can wrap to a width without knowing anything about the
//! syntax it came from.
//!
//! Every block remembers the source line it started on. That is what lets the
//! preview open where the caret was, put the caret back where the preview was
//! scrolled to, and give the outline a line to name for each heading.
//!
//! Headless like the rest of `editor/`: no colours here, only what a run *is*
//! (emphasised, code, a link) — `ui::markdown` decides what that looks like.

use std::ops::Range;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

/// How a run of inline text is emphasised. Flags rather than an enum: `***a***`
/// is bold *and* italic, and a link can be either.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Emph {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// Inline code, which is also how inline maths is drawn.
    pub code: bool,
    /// An index into `Markdown::links`.
    pub link: Option<usize>,
    /// An image that could not be drawn as a picture: its alt text, in place.
    pub image: bool,
    /// A footnote reference, `[^1]`.
    pub footnote: bool,
}

/// A piece of inline text that shares one `Emph`. A hard line break is a run
/// whose text is `"\n"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub emph: Emph,
}

impl Run {
    fn new(text: impl Into<String>, emph: Emph) -> Self {
        Self {
            text: text.into(),
            emph,
        }
    }
}

/// The text of a list of runs, without any of its styling.
pub fn plain(runs: &[Run]) -> String {
    runs.iter()
        .map(|run| if run.text == "\n" { " " } else { &run.text })
        .collect()
}

/// A GitHub alert: `> [!NOTE]` and its four siblings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Alert {
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }
}

/// A table column's alignment, as its `:--:` row asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Centre,
    Right,
}

/// One picture in a paragraph that holds nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub alt: String,
    pub url: String,
    /// An index into `Markdown::links`, so a click on the caption follows it.
    pub link: usize,
}

/// One entry of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// `Some` for a task list entry: whether it is ticked.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        runs: Vec<Run>,
        /// What a `#fragment` link to it is spelled as.
        anchor: String,
        line: usize,
    },
    Paragraph {
        runs: Vec<Run>,
        /// Text that sat directly in a tight list's item, with no paragraph of
        /// its own — which is what makes the list tight.
        implicit: bool,
        line: usize,
    },
    /// A paragraph of pictures and nothing else, which is where the preview
    /// draws them.
    Images {
        images: Vec<ImageRef>,
        line: usize,
    },
    Code {
        /// The first word of the fence's info string.
        lang: Option<String>,
        lines: Vec<String>,
        line: usize,
    },
    Quote {
        alert: Option<Alert>,
        blocks: Vec<Block>,
        line: usize,
    },
    List {
        /// The first number, for an ordered list.
        start: Option<u64>,
        items: Vec<Item>,
        line: usize,
    },
    Table {
        aligns: Vec<Align>,
        header: Vec<Vec<Run>>,
        rows: Vec<Vec<Vec<Run>>>,
        line: usize,
    },
    Rule {
        line: usize,
    },
    Footnote {
        label: String,
        blocks: Vec<Block>,
        line: usize,
    },
}

impl Block {
    /// The source line, zero-based, the block starts on.
    pub fn line(&self) -> usize {
        match self {
            Self::Heading { line, .. }
            | Self::Paragraph { line, .. }
            | Self::Images { line, .. }
            | Self::Code { line, .. }
            | Self::Quote { line, .. }
            | Self::List { line, .. }
            | Self::Table { line, .. }
            | Self::Rule { line }
            | Self::Footnote { line, .. } => *line,
        }
    }
}

/// A whole document, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Markdown {
    pub blocks: Vec<Block>,
    /// Every link and image target, in the order they appear. Runs and image
    /// references point into it by index.
    pub links: Vec<String>,
}

impl Markdown {
    pub fn parse(text: &str) -> Self {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_HEADING_ATTRIBUTES
            | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
            | Options::ENABLE_GFM
            | Options::ENABLE_MATH;
        let events = Parser::new_ext(text, options).into_offset_iter().collect();
        let mut builder = Builder {
            events,
            at: 0,
            starts: line_starts(text),
            links: Vec::new(),
            task: None,
            anchors: Vec::new(),
        };
        let blocks = builder.blocks();
        Self {
            blocks,
            links: builder.links,
        }
    }
}

/// The byte each line starts at, for turning an event's range into a line.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

/// The GitHub spelling of a heading's anchor: lower case, spaces as hyphens,
/// and no punctuation but `-` and `_`.
pub fn slug(text: &str) -> String {
    text.trim()
        .chars()
        .filter_map(|ch| {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                Some(ch.to_lowercase().next().unwrap_or(ch))
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

struct Builder<'a> {
    events: Vec<(Event<'a>, Range<usize>)>,
    at: usize,
    starts: Vec<usize>,
    links: Vec<String>,
    /// The task marker of the item being read, until the item takes it.
    task: Option<bool>,
    /// The anchors handed out so far, so that a second `## Usage` gets
    /// `usage-1` the way GitHub numbers it.
    anchors: Vec<String>,
}

/// Whether a tag is inline — something a paragraph contains — rather than a
/// block of its own.
fn is_inline(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

fn is_inline_event(event: &Event) -> bool {
    match event {
        Event::Start(tag) => is_inline(tag),
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineMath(_)
        | Event::InlineHtml(_)
        | Event::FootnoteReference(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::TaskListMarker(_) => true,
        _ => false,
    }
}

impl<'a> Builder<'a> {
    fn line_of(&self, byte: usize) -> usize {
        match self.starts.binary_search(&byte) {
            Ok(line) => line,
            Err(next) => next.saturating_sub(1),
        }
    }

    fn peek(&self) -> Option<&(Event<'a>, Range<usize>)> {
        self.events.get(self.at)
    }

    fn next(&mut self) -> Option<(Event<'a>, Range<usize>)> {
        let event = self.events.get(self.at).cloned();
        self.at += 1;
        event
    }

    fn link(&mut self, url: &str) -> usize {
        self.links.push(url.to_string());
        self.links.len() - 1
    }

    fn anchor(&mut self, explicit: Option<&str>, runs: &[Run]) -> String {
        let base = explicit.map_or_else(|| slug(&plain(runs)), str::to_string);
        let mut anchor = base.clone();
        let mut n = 0;
        while self.anchors.contains(&anchor) {
            n += 1;
            anchor = format!("{base}-{n}");
        }
        self.anchors.push(anchor.clone());
        anchor
    }

    /// Blocks until the end of the container they are in, whose closing event
    /// is consumed — or until the end of the document.
    fn blocks(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        while let Some((event, range)) = self.peek().cloned() {
            let line = self.line_of(range.start);
            if is_inline_event(&event) {
                let runs = self.inlines(true);
                if !runs.is_empty() {
                    blocks.push(paragraph(runs, true, line));
                }
                continue;
            }
            self.at += 1;
            match event {
                Event::End(_) => return blocks,
                Event::Rule => blocks.push(Block::Rule { line }),
                Event::DisplayMath(text) => blocks.push(Block::Code {
                    lang: Some("math".into()),
                    lines: text.trim_end().lines().map(str::to_string).collect(),
                    line,
                }),
                Event::Html(text) => {
                    let html = self.html_blocks(&text, line);
                    blocks.extend(html);
                }
                Event::Start(tag) => self.block(tag, line, &mut blocks),
                _ => {}
            }
        }
        blocks
    }

    fn block(&mut self, tag: Tag<'a>, line: usize, blocks: &mut Vec<Block>) {
        match tag {
            Tag::Paragraph => {
                let runs = self.inlines(false);
                blocks.push(paragraph(runs, false, line));
            }
            Tag::Heading { level, id, .. } => {
                let runs = self.inlines(false);
                let anchor = self.anchor(id.as_deref(), &runs);
                blocks.push(Block::Heading {
                    level: heading_level(level),
                    runs,
                    anchor,
                    line,
                });
            }
            Tag::BlockQuote(kind) => {
                let inner = self.blocks();
                blocks.push(Block::Quote {
                    alert: kind.map(alert),
                    blocks: inner,
                    line,
                });
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '{')
                        .next()
                        .filter(|word| !word.is_empty())
                        .map(str::to_string),
                    CodeBlockKind::Indented => None,
                };
                let text = self.text_until_end();
                blocks.push(Block::Code {
                    lang,
                    lines: code_lines(&text),
                    line,
                });
            }
            Tag::MetadataBlock(_) => {
                let text = self.text_until_end();
                blocks.push(Block::Code {
                    lang: Some("yaml".into()),
                    lines: code_lines(&text),
                    line,
                });
            }
            Tag::HtmlBlock => {
                let text = self.text_until_end();
                let html = self.html_blocks(&text, line);
                blocks.extend(html);
            }
            Tag::List(start) => {
                let mut items = Vec::new();
                while let Some((event, _)) = self.next() {
                    match event {
                        Event::Start(Tag::Item) => {
                            let outer = self.task.take();
                            let blocks = self.blocks();
                            let task = self.task.take();
                            self.task = outer;
                            items.push(Item { task, blocks });
                        }
                        Event::End(_) => break,
                        _ => {}
                    }
                }
                blocks.push(Block::List { start, items, line });
            }
            Tag::Table(aligns) => {
                let aligns = aligns
                    .iter()
                    .map(|align| match align {
                        Alignment::Center => Align::Centre,
                        Alignment::Right => Align::Right,
                        Alignment::Left | Alignment::None => Align::Left,
                    })
                    .collect();
                let mut header = Vec::new();
                let mut rows = Vec::new();
                while let Some((event, _)) = self.next() {
                    match event {
                        Event::Start(Tag::TableHead) => header = self.cells(),
                        Event::Start(Tag::TableRow) => rows.push(self.cells()),
                        Event::End(TagEnd::Table) => break,
                        _ => {}
                    }
                }
                blocks.push(Block::Table {
                    aligns,
                    header,
                    rows,
                    line,
                });
            }
            Tag::FootnoteDefinition(label) => {
                let inner = self.blocks();
                blocks.push(Block::Footnote {
                    label: label.to_string(),
                    blocks: inner,
                    line,
                });
            }
            // Definition lists and anything a later version adds: what is
            // inside is still worth showing, so it is read as if the container
            // were not there.
            _ => {
                let inner = self.blocks();
                blocks.extend(inner);
            }
        }
    }

    /// The cells of a table row, through the row's own end.
    fn cells(&mut self) -> Vec<Vec<Run>> {
        let mut cells = Vec::new();
        while let Some((event, _)) = self.next() {
            match event {
                Event::Start(Tag::TableCell) => cells.push(self.inlines(false)),
                Event::End(_) => break,
                _ => {}
            }
        }
        cells
    }

    /// Every piece of text up to the end of the current container.
    fn text_until_end(&mut self) -> String {
        let mut text = String::new();
        while let Some((event, _)) = self.next() {
            match event {
                Event::Text(t) | Event::Html(t) | Event::Code(t) => text.push_str(&t),
                Event::End(_) => break,
                _ => {}
            }
        }
        text
    }

    /// Inline content. `loose` is text sitting straight in a tight list item:
    /// it ends at the first block rather than at a closing event of its own,
    /// and neither is consumed.
    fn inlines(&mut self, loose: bool) -> Vec<Run> {
        let mut runs: Vec<Run> = Vec::new();
        let mut stack: Vec<Emph> = vec![Emph::default()];
        // What inline HTML tags have turned on, over whatever Markdown has.
        let mut html = HtmlStyle::default();
        // Inside an image, every piece of text is its alt text.
        let mut image: Option<(String, String)> = None;
        while let Some((event, _)) = self.peek().cloned() {
            let depth = stack.len() - 1;
            if loose && !is_inline_event(&event) && !(depth > 0 && matches!(event, Event::End(_))) {
                break;
            }
            self.at += 1;
            let emph = html.apply(*stack.last().unwrap_or(&Emph::default()));
            if let Some((alt, _)) = image.as_mut() {
                match event {
                    Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                    Event::End(TagEnd::Image) => {
                        let (alt, url) = image.take().unwrap_or_default();
                        let link = self.link(&url);
                        let emph = Emph {
                            image: true,
                            link: Some(link),
                            ..emph
                        };
                        push(&mut runs, Run::new(alt, emph));
                        stack.pop();
                    }
                    _ => {}
                }
                continue;
            }
            match event {
                Event::Text(text) => push(&mut runs, Run::new(text.to_string(), emph)),
                Event::Code(text) | Event::InlineMath(text) => push(
                    &mut runs,
                    Run::new(text.to_string(), Emph { code: true, ..emph }),
                ),
                Event::InlineHtml(text) | Event::Html(text) => {
                    for token in html_tokens(&text) {
                        match token {
                            HtmlToken::Text(text) => push(&mut runs, Run::new(text, emph)),
                            HtmlToken::Tag {
                                name,
                                closing,
                                attrs,
                            } => match name.as_str() {
                                "br" => runs.push(Run::new("\n", emph)),
                                "img" => {
                                    let url = attr(&attrs, "src").unwrap_or_default();
                                    let alt = attr(&attrs, "alt").unwrap_or_default();
                                    let link = self.link(&url);
                                    let emph = Emph {
                                        image: true,
                                        link: Some(link),
                                        ..emph
                                    };
                                    push(&mut runs, Run::new(alt, emph));
                                }
                                "a" if !closing => {
                                    html.link = attr(&attrs, "href").map(|url| self.link(&url));
                                }
                                _ => html.tag(&name, closing),
                            },
                        }
                    }
                }
                Event::FootnoteReference(label) => push(
                    &mut runs,
                    Run::new(
                        format!("[{label}]"),
                        Emph {
                            footnote: true,
                            ..emph
                        },
                    ),
                ),
                Event::SoftBreak => push(&mut runs, Run::new(" ", emph)),
                Event::HardBreak => runs.push(Run::new("\n", emph)),
                Event::TaskListMarker(done) => self.task = Some(done),
                Event::Start(tag) => {
                    let next = match tag {
                        Tag::Emphasis => Emph {
                            italic: true,
                            ..emph
                        },
                        Tag::Strong => Emph { bold: true, ..emph },
                        Tag::Strikethrough => Emph {
                            strike: true,
                            ..emph
                        },
                        Tag::Link { dest_url, .. } => Emph {
                            link: Some(self.link(&dest_url)),
                            ..emph
                        },
                        Tag::Image { dest_url, .. } => {
                            image = Some((String::new(), dest_url.to_string()));
                            emph
                        }
                        _ => emph,
                    };
                    stack.push(next);
                }
                Event::End(_) => {
                    if depth == 0 {
                        break;
                    }
                    stack.pop();
                }
                _ => {}
            }
        }
        runs
    }
}

/// The inline styles HTML tags have switched on, counted so that `<b><b>x</b>`
/// is still bold after the first close.
#[derive(Debug, Clone, Copy, Default)]
struct HtmlStyle {
    bold: u8,
    italic: u8,
    code: u8,
    strike: u8,
    link: Option<usize>,
}

impl HtmlStyle {
    fn tag(&mut self, name: &str, closing: bool) {
        let counter = match name {
            "b" | "strong" => &mut self.bold,
            "i" | "em" | "cite" => &mut self.italic,
            "code" | "kbd" | "tt" | "samp" => &mut self.code,
            "s" | "del" | "strike" => &mut self.strike,
            "a" if closing => {
                self.link = None;
                return;
            }
            _ => return,
        };
        *counter = if closing {
            counter.saturating_sub(1)
        } else {
            counter.saturating_add(1)
        };
    }

    fn apply(self, emph: Emph) -> Emph {
        Emph {
            bold: emph.bold || self.bold > 0,
            italic: emph.italic || self.italic > 0,
            code: emph.code || self.code > 0,
            strike: emph.strike || self.strike > 0,
            link: emph.link.or(self.link),
            ..emph
        }
    }
}

/// A piece of HTML, read loosely: the preview wants what a README's HTML
/// *shows* — its pictures, its headings, its words — and not a DOM.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HtmlToken {
    Text(String),
    Tag {
        name: String,
        closing: bool,
        attrs: String,
    },
}

/// Splits HTML into text and tags, dropping comments and decoding the common
/// entities in the text.
fn html_tokens(text: &str) -> Vec<HtmlToken> {
    let mut tokens = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        let Some(open) = rest.find('<') else {
            tokens.push(HtmlToken::Text(entities(rest)));
            break;
        };
        if open > 0 {
            tokens.push(HtmlToken::Text(entities(&rest[..open])));
        }
        let tail = &rest[open + 1..];
        let looks_like_tag = tail
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '/' || ch == '!');
        let Some(close) = tail.find('>').filter(|_| looks_like_tag) else {
            tokens.push(HtmlToken::Text("<".into()));
            rest = tail;
            continue;
        };
        let inner = tail[..close].trim_end_matches('/');
        let (closing, inner) = match inner.strip_prefix('/') {
            Some(inner) => (true, inner),
            None => (false, inner),
        };
        let name_end = inner
            .find(|ch: char| ch.is_whitespace())
            .unwrap_or(inner.len());
        tokens.push(HtmlToken::Tag {
            name: inner[..name_end].to_ascii_lowercase(),
            closing,
            attrs: inner[name_end..].to_string(),
        });
        rest = &tail[close + 1..];
    }
    tokens
}

/// The value of attribute `name` in a tag's attribute text.
fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        let before_ok = at == 0 || lower[..at].ends_with(|ch: char| ch.is_whitespace());
        let after = lower[from..].trim_start();
        if !before_ok || !after.starts_with('=') {
            continue;
        }
        let value_at = attrs.len() - after.len() + 1;
        let value = attrs[value_at..].trim_start();
        let quote = value.chars().next()?;
        let value = if quote == '"' || quote == '\'' {
            let body = &value[1..];
            &body[..body.find(quote).unwrap_or(body.len())]
        } else {
            value
                .split(|ch: char| ch.is_whitespace())
                .next()
                .unwrap_or("")
        };
        return Some(entities(value));
    }
    None
}

/// The handful of entities HTML in a Markdown file actually uses.
fn entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let decoded = tail.find(';').filter(|end| *end <= 10).and_then(|end| {
            let name = &tail[1..end];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                "copy" => Some('©'),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|ch| (ch, end))
        });
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Tags that end whatever paragraph of text is being gathered.
const HTML_BREAKS: &[&str] = &[
    "p",
    "div",
    "center",
    "section",
    "article",
    "header",
    "footer",
    "table",
    "tr",
    "td",
    "th",
    "ul",
    "ol",
    "li",
    "details",
    "summary",
    "blockquote",
    "pre",
    "figure",
    "figcaption",
    "dl",
    "dt",
    "dd",
    "nav",
];

fn flush_text(runs: &mut Vec<Run>, blocks: &mut Vec<Block>, line: usize) {
    let mut taken = std::mem::take(runs);
    if let Some(first) = taken.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = taken.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    taken.retain(|run| !run.text.is_empty());
    if !plain(&taken).trim().is_empty() {
        blocks.push(paragraph(taken, false, line));
    }
}

fn flush_images(images: &mut Vec<ImageRef>, blocks: &mut Vec<Block>, line: usize) {
    if !images.is_empty() {
        blocks.push(Block::Images {
            images: std::mem::take(images),
            line,
        });
    }
}

impl Builder<'_> {
    /// A block of HTML, as the blocks it shows: `<img>` is a picture, `<h1>`
    /// is a heading (and so an outline entry), `<hr>` is a rule, the text is
    /// paragraphs, and every other tag is dropped. A README that centres its
    /// logo with `<p align="center">` is the common case, and it should look
    /// like a logo and not like the markup that centres one.
    fn html_blocks(&mut self, text: &str, line: usize) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut runs: Vec<Run> = Vec::new();
        let mut images: Vec<ImageRef> = Vec::new();
        let mut heading: Option<u8> = None;
        let mut style = HtmlStyle::default();

        for token in html_tokens(text) {
            match token {
                HtmlToken::Text(text) => {
                    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if collapsed.is_empty() {
                        if !runs.is_empty() && !text.is_empty() {
                            push(&mut runs, Run::new(" ", style.apply(Emph::default())));
                        }
                        continue;
                    }
                    flush_images(&mut images, &mut blocks, line);
                    let lead = if text.starts_with(char::is_whitespace) {
                        " "
                    } else {
                        ""
                    };
                    let trail = if text.ends_with(char::is_whitespace) {
                        " "
                    } else {
                        ""
                    };
                    push(
                        &mut runs,
                        Run::new(
                            format!("{lead}{collapsed}{trail}"),
                            style.apply(Emph::default()),
                        ),
                    );
                }
                HtmlToken::Tag {
                    name,
                    closing,
                    attrs,
                } => match name.as_str() {
                    "img" => {
                        flush_text(&mut runs, &mut blocks, line);
                        let url = attr(&attrs, "src").unwrap_or_default();
                        let alt = attr(&attrs, "alt").unwrap_or_default();
                        let link = self.link(&url);
                        images.push(ImageRef { alt, url, link });
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        if closing {
                            if let Some(level) = heading.take() {
                                let mut taken = std::mem::take(&mut runs);
                                for run in &mut taken {
                                    run.text = run.text.trim().to_string();
                                }
                                taken.retain(|run| !run.text.is_empty());
                                let anchor = self.anchor(None, &taken);
                                blocks.push(Block::Heading {
                                    level,
                                    runs: taken,
                                    anchor,
                                    line,
                                });
                            }
                        } else {
                            flush_text(&mut runs, &mut blocks, line);
                            flush_images(&mut images, &mut blocks, line);
                            heading = name[1..].parse().ok();
                        }
                    }
                    "hr" => {
                        flush_text(&mut runs, &mut blocks, line);
                        flush_images(&mut images, &mut blocks, line);
                        blocks.push(Block::Rule { line });
                    }
                    "br" => runs.push(Run::new("\n", style.apply(Emph::default()))),
                    "a" if !closing => {
                        style.link = attr(&attrs, "href").map(|url| self.link(&url));
                    }
                    name if HTML_BREAKS.contains(&name) && heading.is_none() => {
                        flush_text(&mut runs, &mut blocks, line);
                        flush_images(&mut images, &mut blocks, line);
                    }
                    name => style.tag(name, closing),
                },
            }
        }
        flush_text(&mut runs, &mut blocks, line);
        flush_images(&mut images, &mut blocks, line);
        blocks
    }
}

/// Appends a run, merging it into the last one when they look the same.
fn push(runs: &mut Vec<Run>, run: Run) {
    if run.text.is_empty() {
        return;
    }
    match runs.last_mut() {
        Some(last) if last.emph == run.emph && last.text != "\n" && run.text != "\n" => {
            last.text.push_str(&run.text);
        }
        _ => runs.push(run),
    }
}

/// A paragraph, or the pictures it consists of.
fn paragraph(runs: Vec<Run>, implicit: bool, line: usize) -> Block {
    let pictures = runs.iter().any(|run| run.emph.image)
        && runs
            .iter()
            .all(|run| run.emph.image || run.text.trim().is_empty());
    if pictures {
        let images = runs
            .into_iter()
            .filter(|run| run.emph.image)
            .map(|run| ImageRef {
                alt: run.text,
                url: String::new(),
                link: run.emph.link.unwrap_or(0),
            })
            .collect();
        return Block::Images { images, line };
    }
    Block::Paragraph {
        runs,
        implicit,
        line,
    }
}

fn code_lines(text: &str) -> Vec<String> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    text.split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alert(kind: BlockQuoteKind) -> Alert {
    match kind {
        BlockQuoteKind::Note => Alert::Note,
        BlockQuoteKind::Tip => Alert::Tip,
        BlockQuoteKind::Important => Alert::Important,
        BlockQuoteKind::Warning => Alert::Warning,
        BlockQuoteKind::Caution => Alert::Caution,
    }
}

impl Markdown {
    /// Fills in each picture's URL from the link table. Done once after the
    /// parse rather than while building, because the builder hands out the
    /// index before it knows the run is a picture of its own.
    fn resolve_images(blocks: &mut [Block], links: &[String]) {
        for block in blocks {
            match block {
                Block::Images { images, .. } => {
                    for image in images {
                        image.url = links.get(image.link).cloned().unwrap_or_default();
                    }
                }
                Block::Quote { blocks, .. } | Block::Footnote { blocks, .. } => {
                    Self::resolve_images(blocks, links);
                }
                Block::List { items, .. } => {
                    for item in items {
                        Self::resolve_images(&mut item.blocks, links);
                    }
                }
                _ => {}
            }
        }
    }

    /// `parse`, with every picture's URL filled in.
    pub fn read(text: &str) -> Self {
        let mut doc = Self::parse(text);
        let links = doc.links.clone();
        Self::resolve_images(&mut doc.blocks, &links);
        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(block: &Block) -> &[Run] {
        match block {
            Block::Paragraph { runs, .. } | Block::Heading { runs, .. } => runs,
            other => panic!("not text: {other:?}"),
        }
    }

    #[test]
    fn headings_carry_level_line_and_anchor() {
        let doc = Markdown::read("# Title\n\ntext\n\n## Getting started!\n\n## Getting started!\n");
        let headings: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading {
                    level,
                    anchor,
                    line,
                    ..
                } => Some((*level, anchor.as_str(), *line)),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            vec![
                (1, "title", 0),
                (2, "getting-started", 4),
                (2, "getting-started-1", 6)
            ]
        );
    }

    #[test]
    fn emphasis_nests_into_runs() {
        let doc = Markdown::read("plain **bold _both_** `code` ~~gone~~\n");
        let runs = runs(&doc.blocks[0]);
        assert_eq!(runs[0].text, "plain ");
        assert!(runs[1].emph.bold && !runs[1].emph.italic);
        assert!(runs[2].emph.bold && runs[2].emph.italic);
        assert_eq!(runs[2].text, "both");
        assert!(runs.iter().any(|r| r.emph.code && r.text == "code"));
        assert!(runs.iter().any(|r| r.emph.strike && r.text == "gone"));
    }

    #[test]
    fn links_point_into_the_table() {
        let doc = Markdown::read("see [the docs](docs/A.md) and <https://x.dev>\n");
        let linked: Vec<_> = runs(&doc.blocks[0])
            .iter()
            .filter_map(|r| {
                r.emph
                    .link
                    .map(|i| (r.text.as_str(), doc.links[i].as_str()))
            })
            .collect();
        assert_eq!(
            linked,
            vec![
                ("the docs", "docs/A.md"),
                ("https://x.dev", "https://x.dev")
            ]
        );
    }

    #[test]
    fn a_paragraph_of_pictures_is_an_image_block() {
        let doc = Markdown::read("![shot](img/a.png) ![two](b.jpg)\n\ntext ![inline](c.png)\n");
        match &doc.blocks[0] {
            Block::Images { images, .. } => {
                assert_eq!(images.len(), 2);
                assert_eq!(images[0].alt, "shot");
                assert_eq!(images[0].url, "img/a.png");
                assert_eq!(images[1].url, "b.jpg");
            }
            other => panic!("{other:?}"),
        }
        let inline = runs(&doc.blocks[1]);
        assert!(inline.iter().any(|r| r.emph.image && r.text == "inline"));
    }

    #[test]
    fn tight_and_loose_lists_and_tasks() {
        let doc = Markdown::read("- [x] done\n- [ ] todo\n  - nested\n\n1. a\n\n2. b\n");
        let Block::List { start, items, .. } = &doc.blocks[0] else {
            panic!()
        };
        assert_eq!(*start, None);
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        assert!(matches!(
            items[0].blocks[0],
            Block::Paragraph { implicit: true, .. }
        ));
        assert!(matches!(items[1].blocks[1], Block::List { .. }));
        let Block::List { start, items, .. } = &doc.blocks[1] else {
            panic!()
        };
        assert_eq!(*start, Some(1));
        assert!(matches!(
            items[0].blocks[0],
            Block::Paragraph {
                implicit: false,
                ..
            }
        ));
    }

    #[test]
    fn tables_keep_alignment_and_cells() {
        let doc = Markdown::read("| a | b |\n|:--|--:|\n| 1 | **2** |\n");
        let Block::Table {
            aligns,
            header,
            rows,
            ..
        } = &doc.blocks[0]
        else {
            panic!()
        };
        assert_eq!(aligns, &vec![Align::Left, Align::Right]);
        assert_eq!(plain(&header[1]), "b");
        assert_eq!(rows.len(), 1);
        assert!(rows[0][1][0].emph.bold);
    }

    #[test]
    fn code_quotes_and_alerts() {
        let doc = Markdown::read(
            "```rust,ignore\nfn main() {}\n\n```\n\n> [!WARNING]\n> careful\n\n> plain\n",
        );
        assert_eq!(
            doc.blocks[0],
            Block::Code {
                lang: Some("rust".into()),
                lines: vec!["fn main() {}".into(), String::new()],
                line: 0
            }
        );
        assert!(matches!(
            doc.blocks[1],
            Block::Quote {
                alert: Some(Alert::Warning),
                ..
            }
        ));
        assert!(matches!(doc.blocks[2], Block::Quote { alert: None, .. }));
    }

    #[test]
    fn html_shows_its_pictures_headings_and_words() {
        let doc = Markdown::read(
            "<p align=\"center\">\n  <img src=\"assets/hero.png\" alt=\"Hero &amp; logo\" width=\"900\">\n</p>\n\n\
<h1 align=\"center\">FerroEdit</h1>\n\n<p><em>A modern</em> editor<br>done</p>\n\n<!-- hidden -->\n",
        );
        match &doc.blocks[0] {
            Block::Images { images, .. } => {
                assert_eq!(images[0].url, "assets/hero.png");
                assert_eq!(images[0].alt, "Hero & logo");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            &doc.blocks[1],
            Block::Heading { level: 1, anchor, .. } if anchor == "ferroedit"
        ));
        let text = runs(&doc.blocks[2]);
        assert!(text[0].emph.italic && text[0].text == "A modern");
        assert_eq!(plain(text), "A modern editor done");
        assert_eq!(doc.blocks.len(), 3, "the comment shows nothing");
    }

    #[test]
    fn inline_html_styles_rather_than_shows() {
        let doc = Markdown::read("press <kbd>F7</kbd> or <b>F8</b><br>now\n");
        let text = runs(&doc.blocks[0]);
        assert!(text.iter().any(|r| r.emph.code && r.text == "F7"));
        assert!(text.iter().any(|r| r.emph.bold && r.text == "F8"));
        assert!(text.iter().any(|r| r.text == "\n"));
        assert!(!plain(text).contains('<'));
    }

    #[test]
    fn slugs_follow_github() {
        assert_eq!(slug("Hello, World — 2!"), "hello-world--2");
        assert_eq!(slug("ADR-062: The CSV view"), "adr-062-the-csv-view");
    }
}
