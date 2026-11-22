//! Markdown → ANSI string. Streams pulldown-cmark events, collects inline
//! spans per block, wraps them and prefixes each line with the active
//! containers (block quotes, list items).

mod blocks;
mod document;
mod images;
mod inline;
mod layout;
mod span;
#[cfg(test)]
mod tests;
mod wrap;

use std::mem;
use std::path::PathBuf;
use std::rc::Rc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::heading::HeadingFont;
use crate::mermaid;
use crate::style::{Color, Style, Underline};
use crate::term::Rgb;

use blocks::{CodeBlock, Table};
pub use document::{Document, ImageRow, Line};
use images::PendingImage;
use layout::Container;
use span::Span;

const ACCENT: Color = Color::Ansi(6);
const LINK: Color = Color::Ansi(4);
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

pub struct Config {
    pub width: usize,
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
    /// Kitty image ids are allocated upwards from here (24-bit, non-zero).
    pub first_image_id: u32,
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
    /// Relative image paths resolve against this (the markdown file's dir).
    pub base_dir: PathBuf,
}

pub fn render(markdown: &str, config: &Config) -> Document {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut renderer = Renderer::new(config);
    for event in Parser::new_ext(markdown, options) {
        renderer.event(event);
    }
    renderer.finish()
}

struct Renderer<'c> {
    cfg: &'c Config,
    out: String,
    preamble: String,
    image_ids: Vec<u32>,
    /// Lines written to `out` so far.
    line_no: usize,
    image_rows: Vec<(usize, ImageRow)>,
    diagrams: Vec<(u32, String)>,
    inline: Vec<Span>,
    styles: Vec<Style>,
    containers: Vec<Container>,
    /// `Some(next_number)` for ordered lists.
    lists: Vec<Option<u64>>,
    heading: Option<HeadingLevel>,
    code: Option<CodeBlock>,
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
            preamble: String::new(),
            image_ids: Vec::new(),
            line_no: 0,
            image_rows: Vec::new(),
            diagrams: Vec::new(),
            inline: Vec::new(),
            styles: vec![Style::default()],
            containers: Vec::new(),
            lists: Vec::new(),
            heading: None,
            code: None,
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
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some(code) => code.text.push_str(&text),
                None => self.push_text(&text),
            },
            Event::Code(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.push_code(&text);
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                self.inline.push(Span::new(text.as_ref(), Style::dim()));
            }
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
            Tag::Paragraph | Tag::HtmlBlock => self.open_block(),
            Tag::Heading { level, .. } => {
                self.open_block();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.open_block();
                let bar = Span::new("▎ ", Style::fg(Color::Ansi(8)));
                self.containers.push(Container::repeating(vec![bar]));
            }
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
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.open_block();
                } else {
                    self.flush_inline();
                }
                self.lists.push(start);
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
            Tag::Link { dest_url, .. } => self.push_style(|s| {
                s.link = Some(Rc::from(dest_url.as_ref()));
                s.fg = Some(LINK);
                s.underline = Underline::Single;
                s.underline_color = Some(LINK);
            }),
            Tag::Image { dest_url, .. } => {
                let start = self.inline.len();
                self.images.push(PendingImage {
                    url: dest_url.to_string(),
                    alt: start..start,
                });
                self.push_style(|s| {
                    s.link = Some(Rc::from(dest_url.as_ref()));
                    s.fg = Some(Color::Ansi(5));
                    s.italic = true;
                });
                self.push_text("🖼 ");
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.close_block(),
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
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.pending_blank = true;
                }
            }
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
                    let end = self.inline.len();
                    if let Some(image) = self.images.last_mut() {
                        image.alt.end = end;
                    }
                }
                if self.styles.len() > 1 {
                    self.styles.pop();
                }
            }
            _ => {}
        }
    }
}
