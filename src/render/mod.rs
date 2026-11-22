//! Markdown → ANSI string. Streams pulldown-cmark events, collects inline
//! spans per block, wraps them and prefixes each line with the active
//! containers (block quotes, list items).

mod blocks;
mod deflist;
mod document;
mod emoji;
mod formulas;
#[cfg(feature = "highlight")]
mod highlight;
mod html;
mod images;
mod inline;
mod layout;
mod links;
mod span;
mod table;
#[cfg(test)]
mod tests;
mod wrap;

use std::collections::VecDeque;
use std::mem;
use std::path::PathBuf;

use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, MetadataBlockKind, Options, Parser, Tag, TagEnd,
    TextMergeStream,
};

use crate::heading::HeadingFont;
use crate::mermaid;
use crate::style::{Color, Style};
use crate::term::Rgb;

use blocks::CodeBlock;
pub use document::{Document, Heading, ImageRow, Line};
use html::OpenTag;
use images::PendingImage;
use inline::link_style;
use layout::Container;
use span::{Pen, Span};
use table::Table;

const ACCENT: Color = Color::Ansi(6);
const LINK: Color = Color::Ansi(4);
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

struct OrderedList {
    next: u64,
    /// Digits in the last number; markers are right-aligned to it.
    width: usize,
}

pub struct Config {
    pub width: usize,
    /// `false` under `NO_COLOR`: text keeps bold, italic and the like, but no colors.
    pub color: bool,
    /// Kitty text sizing protocol (OSC 66) for big headings.
    pub text_sizing: bool,
    /// Background for code; `None` falls back to a left bar.
    pub code_bg: Option<Color>,
    /// Inline images via kitty graphics; `None` shows alt text instead.
    pub images: Option<Images>,
    /// H1/H2 rasterized and shown via kitty graphics, when OSC 66 is absent.
    pub headings: Option<HeadingImages>,
    /// Mermaid code blocks rendered as images.
    pub diagrams: Option<Diagrams>,
    /// LaTeX formulas rendered as images; `None` shows them as code.
    pub math: Option<MathImages>,
    /// Kitty image ids are allocated upwards from here (24-bit, non-zero).
    pub first_image_id: u32,
    /// Relative image paths and links resolve against this (the Markdown
    /// file's directory).
    pub base_dir: PathBuf,
}

pub struct Diagrams {
    pub palette: mermaid::Palette,
    /// Cell size in pixels, `(width, height)`.
    pub cell: (u32, u32),
    pub max_rows: u32,
    /// 1.0 fits the available width and `max_rows`; above that diagrams overflow and can be panned.
    pub zoom: f32,
    /// Cap on diagram text size relative to terminal text at zoom 1.
    pub max_upscale: f32,
    /// Centre diagrams narrower than the available width.
    pub center: bool,
}

pub struct MathImages {
    /// Cell size in pixels, `(width, height)`.
    pub cell: (u32, u32),
    pub color: Rgb,
}

pub struct HeadingImages {
    pub font: HeadingFont,
    /// Cell size in pixels, `(width, height)`.
    pub cell: (u32, u32),
    /// Colors for H1, H2 and H3.
    pub colors: [Rgb; 3],
}

pub struct Images {
    /// Cell size in pixels, `(width, height)`.
    pub cell: (u32, u32),
    pub max_rows: u32,
    /// 1.0 fits the available width and `max_rows`; above that images
    /// overflow and can be panned.
    pub zoom: f32,
}

pub fn render(markdown: &str, config: &Config) -> Document {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_MATH;
    let markdown = deflist::escape_false_markers(markdown, options);
    // An escape splits the text around it; merged back, a `:rocket:` right
    // after an escaped colon is still one piece for shortcode replacement.
    let events: Vec<Event<'_>> =
        TextMergeStream::new(Parser::new_ext(&markdown, options)).collect();
    if config.images.is_some() {
        crate::remote::prefetch(&image_urls(&events));
    }
    let mut renderer = Renderer::new(config);
    renderer.list_lengths = ordered_list_lengths(&events);
    for event in events {
        renderer.event(event);
    }
    renderer.finish()
}

/// Every image a document references, from Markdown and from `<img>` tags.
fn image_urls<'a>(events: &'a [Event<'_>]) -> Vec<&'a str> {
    let mut urls = Vec::new();
    for event in events {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => urls.push(dest_url.as_ref()),
            Event::Html(html) | Event::InlineHtml(html) => urls.extend(html::img_sources(html)),
            _ => {}
        }
    }
    urls
}

/// Item count of every ordered list, in the order the lists start, so
/// markers can be padded to the widest number before it is reached.
fn ordered_list_lengths(events: &[Event<'_>]) -> VecDeque<u64> {
    let mut lengths = VecDeque::new();
    let mut open: Vec<Option<usize>> = Vec::new();
    for event in events {
        match event {
            Event::Start(Tag::List(Some(_))) => {
                open.push(Some(lengths.len()));
                lengths.push_back(0);
            }
            Event::Start(Tag::List(None)) => open.push(None),
            Event::End(TagEnd::List(_)) => {
                open.pop();
            }
            Event::Start(Tag::Item) => {
                if let Some(n) = open
                    .last()
                    .copied()
                    .flatten()
                    .and_then(|i| lengths.get_mut(i))
                {
                    *n += 1;
                }
            }
            _ => {}
        }
    }
    lengths
}

struct Renderer<'c> {
    cfg: &'c Config,
    out: String,
    /// Style state of the line being written to `out`.
    pen: Pen,
    preamble: String,
    image_ids: Vec<u32>,
    /// Lines written to `out` so far.
    line_no: usize,
    image_rows: Vec<(usize, ImageRow)>,
    diagrams: Vec<(u32, String)>,
    headings: Vec<Heading>,
    anchors: links::Anchors,
    inline: Vec<Span>,
    styles: Vec<Style>,
    html_tags: Vec<OpenTag>,
    in_html_comment: bool,
    containers: Vec<Container>,
    lists: Vec<Option<OrderedList>>,
    list_lengths: VecDeque<u64>,
    heading: Option<HeadingLevel>,
    code: Option<CodeBlock>,
    /// Front matter being collected, with its key/value separator.
    metadata: Option<(char, String)>,
    table: Option<Table>,
    images: Vec<PendingImage>,
    pending_blank: bool,
    wrote_any: bool,
}

impl<'c> Renderer<'c> {
    fn new(cfg: &'c Config) -> Self {
        Self {
            cfg,
            out: String::new(),
            pen: Pen::new(cfg.color),
            preamble: String::new(),
            image_ids: Vec::new(),
            line_no: 0,
            image_rows: Vec::new(),
            diagrams: Vec::new(),
            headings: Vec::new(),
            anchors: links::Anchors::default(),
            inline: Vec::new(),
            styles: vec![Style::default()],
            html_tags: Vec::new(),
            in_html_comment: false,
            containers: Vec::new(),
            lists: Vec::new(),
            list_lengths: VecDeque::new(),
            heading: None,
            code: None,
            metadata: None,
            table: None,
            images: Vec::new(),
            pending_blank: false,
            wrote_any: false,
        }
    }

    fn finish(mut self) -> Document {
        self.flush_inline();
        let body = self.out.strip_suffix('\n').unwrap_or(&self.out);
        let mut lines: Vec<Line> = if body.is_empty() {
            Vec::new()
        } else {
            body.split('\n')
                .map(|text| Line {
                    text: text.to_owned(),
                    image: None,
                })
                .collect()
        };
        for (n, row) in self.image_rows {
            if let Some(line) = lines.get_mut(n) {
                line.image = Some(row);
            }
        }
        Document {
            preamble: self.preamble,
            lines,
            image_ids: self.image_ids,
            diagrams: self.diagrams,
            headings: self.headings,
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if let Some(code) = &mut self.code {
                    code.text.push_str(&text);
                } else if let Some((_, meta)) = &mut self.metadata {
                    meta.push_str(&text);
                } else {
                    self.push_prose(&text);
                }
            }
            Event::Code(text) => self.push_code(&text),
            Event::InlineMath(text) => self.inline_math(&text),
            Event::DisplayMath(text) => self.display_math(&text),
            Event::Html(text) => self.html(&text, true),
            Event::InlineHtml(text) => self.html(&text, false),
            Event::FootnoteReference(name) => {
                self.inline
                    .push(Span::new(format!("[{name}]"), Style::fg(ACCENT)));
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_text("\n"),
            Event::Rule => self.rule(),
            Event::TaskListMarker(done) => self.task_marker(done),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.open_block(),
            Tag::HtmlBlock => self.flush_inline(),
            Tag::Heading { level, .. } => {
                self.open_block();
                self.heading = Some(level);
            }
            Tag::BlockQuote(kind) => self.start_quote(kind),
            Tag::CodeBlock(kind) => {
                self.open_block();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(CodeBlock {
                    lang,
                    text: String::new(),
                });
            }
            Tag::MetadataBlock(kind) => {
                let sep = match kind {
                    MetadataBlockKind::YamlStyle => ':',
                    MetadataBlockKind::PlusesStyle => '=',
                };
                self.metadata = Some((sep, String::new()));
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.open_block();
                } else {
                    self.flush_inline();
                }
                let list = start.map(|next| {
                    let len = self.list_lengths.pop_front().unwrap_or(1).max(1);
                    let last = next.saturating_add(len - 1);
                    OrderedList {
                        next,
                        width: last.to_string().len(),
                    }
                });
                self.lists.push(list);
            }
            Tag::DefinitionList => self.open_block(),
            Tag::DefinitionListTitle => {
                self.flush_inline();
                self.push_style(|s| s.bold = true);
            }
            Tag::DefinitionListDefinition => {
                self.flush_inline();
                let indent = Span::new("  ", Style::default());
                self.containers.push(Container::hanging(vec![indent]));
            }
            Tag::Item => {
                self.flush_inline();
                let marker = self.next_marker();
                self.containers.push(Container::hanging(vec![marker]));
            }
            Tag::FootnoteDefinition(name) => {
                self.open_block();
                let label = Span::new(format!("[{name}] "), Style::fg(ACCENT));
                self.containers.push(Container::hanging(vec![label]));
            }
            Tag::Table(aligns) => {
                self.open_block();
                self.table = Some(Table {
                    aligns,
                    rows: Vec::new(),
                    header_rows: 0,
                    row: Vec::new(),
                });
            }
            Tag::Emphasis => self.push_style(|s| s.italic = true),
            Tag::Strong => self.push_style(|s| s.bold = true),
            Tag::Strikethrough => self.push_style(|s| s.strike = true),
            Tag::Link { dest_url, .. } => {
                let url = links::resolve(&self.cfg.base_dir, &dest_url);
                self.push_style(|s| link_style(s, &url));
            }
            Tag::Image { dest_url, .. } => self.start_image(&dest_url),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.close_block(),
            TagEnd::HtmlBlock => self.end_html_block(),
            TagEnd::Heading(_) => {
                self.render_heading();
                self.pending_blank = true;
            }
            TagEnd::BlockQuote(_) | TagEnd::FootnoteDefinition => {
                self.flush_inline();
                self.containers.pop();
                self.pending_blank = true;
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.render_code(&code);
                }
                self.pending_blank = true;
            }
            TagEnd::MetadataBlock(_) => {
                if let Some((sep, text)) = self.metadata.take() {
                    self.render_front_matter(sep, &text);
                }
                self.pending_blank = true;
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.pending_blank = true;
                }
            }
            TagEnd::DefinitionListTitle => {
                self.flush_inline();
                if self.styles.len() > 1 {
                    self.styles.pop();
                }
            }
            TagEnd::DefinitionListDefinition => {
                self.flush_inline();
                self.containers.pop();
            }
            TagEnd::DefinitionList => self.close_block(),
            TagEnd::Item => {
                self.flush_inline();
                self.containers.pop();
            }
            TagEnd::TableCell => {
                self.images.clear();
                let cell = mem::take(&mut self.inline);
                if let Some(t) = &mut self.table {
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    let row = mem::take(&mut t.row);
                    t.rows.push(row);
                    if tag == TagEnd::TableHead {
                        t.header_rows += 1;
                    }
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(&table);
                }
                self.pending_blank = true;
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => {
                if tag == TagEnd::Image {
                    self.end_image_alt();
                }
                if self.styles.len() > 1 {
                    self.styles.pop();
                }
            }
            _ => {}
        }
    }
}
