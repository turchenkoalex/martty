//! Markdown → ANSI string. Streams pulldown-cmark events, collects inline
//! spans per block, wraps them and prefixes each line with the active
//! containers (block quotes, list items).

mod blocks;
mod deflist;
mod document;
mod emoji;
mod formulas;
mod highlight;
mod html;
mod images;
mod inline;
mod layout;
mod links;
mod scan;
mod slice;
mod span;
mod table;
#[cfg(test)]
mod tests;
mod upload;
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
pub use document::{Document, Heading, ImageRow, Line, Source, Wide};
use html::OpenTag;
use images::PendingImage;
use inline::link_style;
use layout::Container;
pub use links::percent_decode;
pub use scan::warm_up;
pub use span::visible_controls;
use span::{Pen, Span};
use table::Table;

const ACCENT: Color = Color::Ansi(6);
/// Bullets and the numbers of ordered lists: the theme's red.
const LIST_MARKER: Color = Color::Ansi(1);
const LINK: Color = Color::Ansi(4);
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

struct OrderedList {
    next: u64,
    /// Digits in the last number; markers are right-aligned to it.
    width: usize,
}

pub struct Config {
    pub width: usize,
    /// `false` under `MARTTY_NO_COLOR`: text keeps bold, italic and the like, but no colors.
    pub color: bool,
    /// Kitty text sizing protocol (OSC 66) for big headings.
    pub text_sizing: bool,
    /// Code blocks and tables keep their natural width instead of wrapping
    /// to `width`; the pager pans the lines that overflow.
    pub overflow: bool,
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
    /// The URL a downloaded document came from; relative image paths and
    /// links resolve against it instead of `base_dir`.
    pub base_url: Option<String>,
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
    /// The color of every heading: the theme's foreground, as text headings are.
    pub color: Rgb,
}

pub struct Images {
    /// Cell size in pixels, `(width, height)`.
    pub cell: (u32, u32),
    pub max_rows: u32,
    /// 1.0 fits the available width and `max_rows`; above that images
    /// overflow and can be panned.
    pub zoom: f32,
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_MATH
}

/// The source line (1-based) of every heading, in the order of
/// [`Document::headings`], so a place in the pager maps back to the file.
pub fn heading_lines(markdown: &str) -> Vec<usize> {
    Parser::new_ext(markdown, options())
        .into_offset_iter()
        .filter(|(event, _)| matches!(event, Event::Start(Tag::Heading { .. })))
        .map(|(_, range)| {
            let before = markdown.get(..range.start).unwrap_or_default();
            before.matches('\n').count() + 1
        })
        .collect()
}

pub fn render(markdown: &str, config: &Config) -> Document {
    let options = options();
    let markdown = deflist::escape_false_markers(markdown, options);
    // An escape splits the text around it; merged back, a `:rocket:` right
    // after an escaped colon is still one piece for shortcode replacement.
    let events: Vec<Event<'_>> =
        TextMergeStream::new(Parser::new_ext(&markdown, options)).collect();
    if config.images.is_some() {
        let base = config.base_url.as_deref();
        let urls: Vec<_> = scan::image_urls(&events)
            .into_iter()
            .map(|url| links::absolute(base, url))
            .collect();
        crate::remote::prefetch(&urls.iter().map(AsRef::as_ref).collect::<Vec<_>>());
    }
    if let Some(d) = &config.diagrams {
        crate::mermaid::prefetch(&scan::mermaid_sources(&events), &d.palette);
    }
    let mut renderer = Renderer::new(config);
    renderer.list_lengths = scan::ordered_list_lengths(&events);
    for event in events {
        renderer.event(event);
    }
    renderer.finish()
}

struct Renderer<'c> {
    cfg: &'c Config,
    out: String,
    /// Style state of the line being written to `out`.
    pen: Pen,
    preamble: String,
    uploads: Vec<upload::Upload>,
    image_ids: Vec<u32>,
    /// Lines written to `out` so far.
    line_no: usize,
    image_rows: Vec<(usize, ImageRow)>,
    /// Line number → bytes taken by its container prefix.
    prefixes: Vec<(usize, usize)>,
    wide_rows: Vec<(usize, Wide)>,
    sources: Vec<Source>,
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
            uploads: Vec::new(),
            image_ids: Vec::new(),
            line_no: 0,
            image_rows: Vec::new(),
            prefixes: Vec::new(),
            wide_rows: Vec::new(),
            sources: Vec::new(),
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
            body.split('\n').map(Line::new).collect()
        };
        for (n, row) in self.image_rows {
            if let Some(line) = lines.get_mut(n) {
                line.image = Some(row);
            }
        }
        for (n, len) in self.prefixes {
            if let Some(line) = lines.get_mut(n) {
                line.prefix_len = len.min(line.text.len());
            }
        }
        for (n, wide) in self.wide_rows {
            if let Some(line) = lines.get_mut(n) {
                line.wide = Some(wide);
            }
        }
        Document {
            preamble: self.preamble,
            uploads: self.uploads,
            lines,
            image_ids: self.image_ids,
            diagrams: self.diagrams,
            headings: self.headings,
            sources: self.sources,
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
                let url = links::target(self.cfg, &dest_url);
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
