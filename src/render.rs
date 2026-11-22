//! Markdown → ANSI string. Streams pulldown-cmark events, collects inline
//! spans per block, wraps them and prefixes each line with the active
//! containers (block quotes, list items).

use std::borrow::Cow;
use std::fmt::Write as _;
use std::mem;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::heading::HeadingFont;
use crate::kitty;
use crate::mermaid;
use crate::style::{Color, Style, Underline};
use crate::term::Rgb;

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

/// Rendered output: escape sequences that must reach the terminal first
/// (image uploads), then the visible lines.
pub struct Document {
    pub preamble: String,
    pub lines: Vec<Line>,
    pub image_ids: Vec<u32>,
    /// Image id → mermaid source, so the pager can open a diagram alone.
    pub diagrams: Vec<(u32, String)>,
}

pub struct Line {
    /// Ready-to-print ANSI text.
    pub text: String,
    /// Set when the line is a row of image placeholders.
    pub image: Option<ImageRow>,
}

/// Enough about a placeholder row to redraw it shifted sideways.
#[derive(Debug, Clone, Copy)]
pub struct ImageRow {
    pub id: u32,
    pub row: u32,
    /// Image width in cells.
    pub cols: u32,
    /// Cells shown at once (the available width).
    pub visible: u32,
    /// Bytes of `text` taken by container prefixes (quote bars, bullets).
    pub prefix_len: usize,
}

impl ImageRow {
    /// How far this row can be panned.
    pub fn overflow(&self) -> u32 {
        self.cols.saturating_sub(self.visible).min(kitty::MAX_COL)
    }
}

impl Line {
    /// The line with images wider than the screen shifted left by `pan` cells.
    pub fn panned(&self, pan: u32) -> Cow<'_, str> {
        let Some(img) = self.image.filter(|i| i.overflow() > 0 && pan > 0) else {
            return Cow::Borrowed(&self.text);
        };
        let mut out = self.text[..img.prefix_len].to_owned();
        let (r, g, b) = kitty::id_color(img.id);
        Style::fg(Color::Rgb(r, g, b)).write_sgr(&mut out);
        out.push_str(&kitty::placeholder_row(
            img.row,
            pan.min(img.overflow()),
            img.visible,
        ));
        out.push_str("\x1b[0m");
        Cow::Owned(out)
    }
}

impl Document {
    /// Everything as one string, for printing without a pager.
    pub fn to_ansi(&self) -> String {
        let mut out = self.preamble.clone();
        for line in &self.lines {
            out.push_str(&line.text);
            out.push('\n');
        }
        out
    }

    /// Widest horizontal pan any line allows.
    pub fn max_pan(&self) -> u32 {
        self.lines
            .iter()
            .filter_map(|l| l.image.map(|i| i.overflow()))
            .max()
            .unwrap_or(0)
    }
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

#[derive(Clone, Debug)]
struct Span {
    text: String,
    style: Style,
}

impl Span {
    fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    fn width(&self) -> usize {
        self.text.width()
    }
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// A block quote or list item: what to print on its first line and on the rest.
struct Container {
    first: Vec<Span>,
    rest: Vec<Span>,
    used: bool,
}

impl Container {
    fn hanging(first: Vec<Span>) -> Self {
        let rest = vec![Span::new(" ".repeat(spans_width(&first)), Style::default())];
        Self {
            first,
            rest,
            used: false,
        }
    }

    fn repeating(prefix: Vec<Span>) -> Self {
        Self {
            first: prefix.clone(),
            rest: prefix,
            used: false,
        }
    }
}

struct CodeBlock {
    lang: String,
    text: String,
}

type Cell = Vec<Span>;

struct Table {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Cell>>,
    header_rows: usize,
    row: Vec<Cell>,
}

struct PendingImage {
    url: String,
    /// Alt-text spans in `Renderer::inline`.
    alt: Range<usize>,
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

    // ── inline ────────────────────────────────────────────────────────────

    fn style(&self) -> Style {
        self.styles.last().cloned().unwrap_or_default()
    }

    fn push_style(&mut self, f: impl FnOnce(&mut Style)) {
        let mut style = self.style();
        f(&mut style);
        self.styles.push(style);
    }

    fn push_text(&mut self, text: &str) {
        let style = self.style();
        self.inline.push(Span::new(text, style));
    }

    fn push_code(&mut self, text: &str) {
        let mut style = self.style();
        match self.cfg.code_bg {
            Some(bg) => {
                style.bg = Some(bg);
                // NBSP padding sticks to neighbours, so `code`, never wraps
                // or leaves a gap before the comma.
                self.inline
                    .push(Span::new(format!("\u{a0}{text}\u{a0}"), style));
            }
            None => {
                style.fg = Some(Color::Ansi(3));
                self.inline.push(Span::new(text, style));
            }
        }
    }

    fn task_marker(&mut self, done: bool) {
        let (mark, style) = if done {
            ("☑", Style::fg(Color::Ansi(2)))
        } else {
            ("☐", Style::dim())
        };
        let unordered = matches!(self.lists.last(), Some(None));
        match self.containers.last_mut() {
            Some(item) if unordered => item.first = vec![Span::new(format!("{mark} "), style)],
            _ => self.inline.push(Span::new(format!("{mark} "), style)),
        }
    }

    fn next_marker(&mut self) -> Span {
        let depth = self.lists.len();
        match self.lists.last_mut() {
            Some(Some(n)) => {
                let marker = format!("{n}. ");
                *n += 1;
                Span::new(marker, Style::fg(ACCENT))
            }
            _ => {
                let bullet = BULLETS[depth.saturating_sub(1) % BULLETS.len()];
                Span::new(format!("{bullet} "), Style::fg(ACCENT))
            }
        }
    }

    // ── block plumbing ───────────────────────────────────────────────────

    fn open_block(&mut self) {
        self.flush_inline();
        if self.pending_blank && self.wrote_any {
            self.blank_line();
        }
        self.pending_blank = false;
    }

    fn close_block(&mut self) {
        self.flush_inline();
        self.pending_blank = true;
    }

    fn flush_inline(&mut self) {
        let spans = mem::take(&mut self.inline);
        let images = mem::take(&mut self.images);
        let in_alt = |i: usize| images.iter().any(|im| im.alt.contains(&i));
        // A paragraph that is just images replaces its alt text with them.
        let only_images = self.cfg.images.is_some()
            && !images.is_empty()
            && spans
                .iter()
                .enumerate()
                .all(|(i, s)| in_alt(i) || s.text.trim().is_empty());

        if !only_images && !spans.iter().all(|s| s.text.trim().is_empty()) {
            for line in wrap(&spans, self.avail()) {
                self.emit_line(&line);
            }
        }
        for image in &images {
            let shown = self.cfg.images.is_some() && self.emit_image(&image.url);
            if only_images && !shown {
                let alt = spans.get(image.alt.clone()).unwrap_or_default();
                for line in wrap(alt, self.avail()) {
                    self.emit_line(&line);
                }
            }
        }
    }

    /// Reserves rows (so container prefixes are drawn), then places the
    /// image over them. Returns `false` when it can't be shown.
    fn emit_image(&mut self, url: &str) -> bool {
        let cfg = self.cfg;
        let Some(ctx) = &cfg.images else {
            return false;
        };
        let Some(data) = resolve(&ctx.base_dir, url).and_then(|p| std::fs::read(p).ok()) else {
            return false;
        };
        let Some(px) = kitty::png_size(&data) else {
            return false;
        };
        let max_cols = u32::try_from(self.avail()).unwrap_or(u32::MAX);
        let (cols, rows) = kitty::fit(px, ctx.cell, max_cols, ctx.max_rows);
        self.place(cols, rows, |out, id| {
            kitty::write_png(out, id, &data, cols, rows);
        });
        true
    }

    /// Uploads an image (via `transmit`, into the preamble) and emits its
    /// placeholder rows as regular lines, after any container prefixes.
    fn place(&mut self, cols: u32, rows: u32, transmit: impl FnOnce(&mut String, u32)) -> u32 {
        let n = u32::try_from(self.image_ids.len()).unwrap_or(0);
        let id = (self.cfg.first_image_id.wrapping_add(n) & 0x00FF_FFFF).max(1);
        self.image_ids.push(id);
        transmit(&mut self.preamble, id);
        let (r, g, b) = kitty::id_color(id);
        let style = Style::fg(Color::Rgb(r, g, b));
        let visible = cols.min(u32::try_from(self.avail()).unwrap_or(u32::MAX));
        for row in 0..rows.min(kitty::MAX_ROWS) {
            let start = self.out.len();
            self.write_prefix();
            let prefix_len = self.out.len() - start;
            let cells = Span::new(kitty::placeholder_row(row, 0, visible), style.clone());
            write_span(&mut self.out, &cells, None);
            self.end_line();
            let info = ImageRow {
                id,
                row,
                cols,
                visible,
                prefix_len,
            };
            self.image_rows.push((self.line_no - 1, info));
        }
        id
    }

    /// Renders a mermaid block as an image. On a parse error prints it and
    /// returns `false`, so the source is shown as a regular code block.
    fn diagram(&mut self, source: &str) -> bool {
        let cfg = self.cfg;
        let Some(d) = &cfg.diagrams else {
            return false;
        };
        let view = (u32::try_from(self.avail()).unwrap_or(u32::MAX), d.max_rows);
        match mermaid::render(source, &d.palette, d.cell, view, d.zoom, d.max_upscale) {
            Ok(r) => {
                let id = self.place(r.cols, r.rows, |out, id| {
                    kitty::write_rgba(out, id, &r.rgba, (r.width, r.height), r.cols, r.rows);
                });
                self.diagrams.push((id, source.to_owned()));
                true
            }
            Err(e) => {
                let msg = format!("mermaid: {}", e.lines().next().unwrap_or_default());
                self.emit_line(&[Span::new(msg, Style::fg(Color::Ansi(1)))]);
                false
            }
        }
    }

    /// Draws H1/H2 as bitmaps two rows tall. Returns `false` (having written
    /// nothing) when styled text should be used instead.
    fn image_heading(&mut self, level: HeadingLevel, spans: &[Span]) -> bool {
        const ROWS: u32 = 2;
        let cfg = self.cfg;
        let Some(h) = &cfg.headings else {
            return false;
        };
        let (scale, color) = match level {
            HeadingLevel::H1 => (2.0, h.colors[0]),
            HeadingLevel::H2 => (1.5, h.colors[1]),
            HeadingLevel::H3 => (1.25, h.colors[2]),
            _ => return false,
        };
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        if !self.containers.is_empty() || !h.font.supports(&text) {
            return false;
        }

        let avail = self.avail();
        let per_line = ((avail as f32 / scale).floor() as usize).max(1);
        let mut bitmaps = Vec::new();
        for line in wrap(spans, per_line) {
            let line: String = line.iter().map(|s| s.text.as_str()).collect();
            match h.font.render(line.trim_end(), h.cell, scale, ROWS, color) {
                Some(bitmap) => bitmaps.push(bitmap),
                None => return false,
            }
        }
        let max_cols = u32::try_from(avail).unwrap_or(u32::MAX);
        for b in bitmaps {
            let cols = b.cols.min(max_cols);
            self.place(cols, ROWS, |out, id| {
                kitty::write_rgba(out, id, &b.rgba, (b.width, b.height), cols, ROWS);
            });
        }
        true
    }

    fn prefix_width(&self) -> usize {
        self.containers.iter().map(|c| spans_width(&c.rest)).sum()
    }

    fn avail(&self) -> usize {
        self.cfg.width.saturating_sub(self.prefix_width()).max(8)
    }

    fn emit_line(&mut self, content: &[Span]) {
        self.write_prefix();
        for span in content {
            write_span(&mut self.out, span, None);
        }
        self.end_line();
    }

    fn write_prefix(&mut self) {
        for c in &mut self.containers {
            let prefix = if c.used { &c.rest } else { &c.first };
            for span in prefix {
                write_span(&mut self.out, span, None);
            }
            c.used = true;
        }
    }

    fn end_line(&mut self) {
        self.out.push_str("\x1b[0m\n");
        self.line_no += 1;
        self.wrote_any = true;
    }

    /// Blank separator: keeps quote bars, never consumes a pending bullet.
    fn blank_line(&mut self) {
        let mut line = String::new();
        for c in self.containers.iter().take_while(|c| c.used) {
            for span in &c.rest {
                write_span(&mut line, span, None);
            }
        }
        self.out.push_str(line.trim_end_matches(' '));
        self.out.push_str("\x1b[0m\n");
        self.line_no += 1;
    }

    // ── blocks ───────────────────────────────────────────────────────────

    fn rule(&mut self) {
        self.open_block();
        let line = Span::new("─".repeat(self.avail()), Style::fg(Color::Ansi(8)));
        self.emit_line(&[line]);
        self.pending_blank = true;
    }

    fn render_heading(&mut self) {
        let level = self.heading.take().unwrap_or(HeadingLevel::H1);
        self.images.clear();
        let mut spans = mem::take(&mut self.inline);
        let (color, underline) = match level {
            HeadingLevel::H1 => (Color::Ansi(5), Underline::Double),
            HeadingLevel::H2 => (Color::Ansi(4), Underline::Single),
            HeadingLevel::H3 => (Color::Ansi(6), Underline::None),
            HeadingLevel::H4 => (Color::Ansi(2), Underline::None),
            _ => (Color::Ansi(3), Underline::None),
        };
        for span in &mut spans {
            span.style.bold = true;
            span.style.fg.get_or_insert(color);
        }

        let big = self.cfg.text_sizing
            && self.containers.is_empty()
            && matches!(level, HeadingLevel::H1 | HeadingLevel::H2);
        if big {
            for line in wrap(&spans, self.avail() / 2) {
                // The scaled line covers two rows; the empty line after it
                // is the space for its lower half.
                for span in &line {
                    write_span(&mut self.out, span, Some(2));
                }
                self.out.push_str("\x1b[0m\n\n");
                self.line_no += 2;
            }
            self.wrote_any = true;
            return;
        }

        if self.image_heading(level, &spans) {
            return;
        }
        for span in &mut spans {
            span.style.underline = underline;
        }
        for line in wrap(&spans, self.avail()) {
            self.emit_line(&line);
        }
    }

    fn render_code(&mut self, code: &CodeBlock) {
        if code.lang == "mermaid" && self.diagram(&code.text) {
            return;
        }
        let avail = self.avail();
        let text = code.text.strip_suffix('\n').unwrap_or(&code.text);
        let Some(bg) = self.cfg.code_bg else {
            let bar = Span::new("│ ", Style::fg(Color::Ansi(8)));
            if !code.lang.is_empty() {
                self.emit_line(&[bar.clone(), Span::new(code.lang.as_str(), Style::dim())]);
            }
            for raw in text.lines() {
                for chunk in hard_wrap(&raw.replace('\t', "    "), avail.saturating_sub(2)) {
                    self.emit_line(&[bar.clone(), Span::new(chunk, Style::default())]);
                }
            }
            return;
        };

        let base = Style {
            bg: Some(bg),
            ..Style::default()
        };
        let padded = |text: String, style: Style| {
            let pad = avail.saturating_sub(text.width() + 1);
            vec![
                Span::new(" ", base.clone()),
                Span::new(text, style),
                Span::new(" ".repeat(pad), base.clone()),
            ]
        };
        let label = Style {
            dim: true,
            italic: true,
            ..base.clone()
        };
        self.emit_line(&padded(code.lang.clone(), label));
        for raw in text.lines() {
            for chunk in hard_wrap(&raw.replace('\t', "    "), avail.saturating_sub(2)) {
                self.emit_line(&padded(chunk, base.clone()));
            }
        }
        self.emit_line(&padded(String::new(), base.clone()));
    }

    fn render_table(&mut self, table: &Table) {
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let widths: Vec<usize> = (0..cols)
            .map(|i| {
                table
                    .rows
                    .iter()
                    .filter_map(|row| row.get(i))
                    .map(|cell| spans_width(cell))
                    .max()
                    .unwrap_or(0)
                    .max(1)
            })
            .collect();
        let border = Style::fg(Color::Ansi(8));
        let rule = |l: &str, m: &str, r: &str| {
            let inner: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            vec![Span::new(
                format!("{l}{}{r}", inner.join(m)),
                border.clone(),
            )]
        };

        self.emit_line(&rule("┌", "┬", "┐"));
        for (r, row) in table.rows.iter().enumerate() {
            if r == table.header_rows && r > 0 {
                self.emit_line(&rule("├", "┼", "┤"));
            }
            let mut line = vec![Span::new("│", border.clone())];
            for (i, width) in widths.iter().enumerate() {
                let mut cell = row.get(i).cloned().unwrap_or_default();
                if r < table.header_rows {
                    cell.iter_mut().for_each(|s| s.style.bold = true);
                }
                let gap = width - spans_width(&cell).min(*width);
                let left = match table.aligns.get(i) {
                    Some(Alignment::Right) => gap,
                    Some(Alignment::Center) => gap / 2,
                    _ => 0,
                };
                line.push(Span::new(" ".repeat(left + 1), Style::default()));
                line.extend(cell);
                line.push(Span::new(" ".repeat(gap - left + 1), Style::default()));
                line.push(Span::new("│", border.clone()));
            }
            self.emit_line(&line);
        }
        self.emit_line(&rule("└", "┴", "┘"));
    }
}

/// Local path for an image URL; remote images are not supported.
fn resolve(base: &Path, url: &str) -> Option<PathBuf> {
    let url = url.strip_prefix("file://").unwrap_or(url);
    if url.contains("://") || url.starts_with("data:") {
        return None;
    }
    let path = PathBuf::from(url.replace("%20", " "));
    Some(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

fn write_span(out: &mut String, span: &Span, scale: Option<u8>) {
    span.style.write_sgr(out);
    if let Some(url) = &span.style.link {
        let _ = write!(out, "\x1b]8;;{url}\x1b\\");
    }
    match scale {
        Some(s) => {
            let _ = write!(out, "\x1b]66;s={s};{}\x07", span.text);
        }
        None => out.push_str(&span.text),
    }
    if span.style.link.is_some() {
        out.push_str("\x1b]8;;\x1b\\");
    }
}

// ── wrapping ─────────────────────────────────────────────────────────────

enum Token {
    /// May span several styles, e.g. `**bold**ness,`.
    Word(Vec<Span>),
    Space(Span),
    Newline,
}

fn tokenize(spans: &[Span]) -> Vec<Token> {
    #[derive(PartialEq)]
    enum Kind {
        Newline,
        Space,
        Text,
    }
    let kind = |c: char| match c {
        '\n' => Kind::Newline,
        ' ' | '\t' => Kind::Space,
        _ => Kind::Text,
    };

    let mut tokens = Vec::new();
    let mut word = Vec::new();
    for span in spans {
        let mut rest = span.text.as_str();
        while let Some(c) = rest.chars().next() {
            let k = kind(c);
            let end = match k {
                Kind::Newline => 1,
                _ => rest.find(|ch| kind(ch) != k).unwrap_or(rest.len()),
            };
            let (piece, tail) = rest.split_at(end);
            rest = tail;
            if k == Kind::Text {
                word.push(Span::new(piece, span.style.clone()));
                continue;
            }
            if !word.is_empty() {
                tokens.push(Token::Word(mem::take(&mut word)));
            }
            tokens.push(match k {
                Kind::Newline => Token::Newline,
                _ => Token::Space(Span::new(" ", span.style.clone())),
            });
        }
    }
    if !word.is_empty() {
        tokens.push(Token::Word(word));
    }
    tokens
}

/// Greedy word wrap by display width; overlong words are split by chars.
fn wrap(spans: &[Span], width: usize) -> Vec<Vec<Span>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line: Vec<Span> = Vec::new();
    let mut used = 0;
    let mut space: Option<Span> = None;

    for token in tokenize(spans) {
        match token {
            Token::Newline => {
                lines.push(mem::take(&mut line));
                used = 0;
                space = None;
            }
            Token::Space(s) => {
                if used > 0 {
                    space = Some(s);
                }
            }
            Token::Word(parts) => {
                let word_width = spans_width(&parts);
                let gap = usize::from(space.is_some());
                if used > 0 && used + gap + word_width > width {
                    lines.push(mem::take(&mut line));
                    used = 0;
                    space = None;
                }
                if let Some(s) = space.take() {
                    line.push(s);
                    used += 1;
                }
                for part in parts {
                    if used + part.width() <= width {
                        used += part.width();
                        line.push(part);
                        continue;
                    }
                    let mut chunk = String::new();
                    for ch in part.text.chars() {
                        let cw = ch.width().unwrap_or(0);
                        if used + cw > width && used > 0 {
                            if !chunk.is_empty() {
                                line.push(Span::new(mem::take(&mut chunk), part.style.clone()));
                            }
                            lines.push(mem::take(&mut line));
                            used = 0;
                        }
                        chunk.push(ch);
                        used += cw;
                    }
                    if !chunk.is_empty() {
                        line.push(Span::new(chunk, part.style));
                    }
                }
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > width && used > 0 {
            out.push(String::new());
            used = 0;
        }
        if let Some(last) = out.last_mut() {
            last.push(ch);
        }
        used += cw;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shadows the glob-imported `render` so assertions see plain output.
    fn render(md: &str, cfg: &Config) -> String {
        super::render(md, cfg).to_ansi()
    }

    fn plain(lines: &[Vec<Span>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\x1b' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn render_plain(md: &str) -> String {
        let cfg = Config {
            width: 40,
            text_sizing: false,
            code_bg: None,
            images: None,
            headings: None,
            diagrams: None,
            first_image_id: 1,
        };
        strip_ansi(&render(md, &cfg))
    }

    #[test]
    fn wrap_should_break_at_word_boundaries() {
        let spans = [Span::new("aaa bbb ccc", Style::default())];
        assert_eq!(plain(&wrap(&spans, 7)), ["aaa bbb", "ccc"]);
    }

    #[test]
    fn wrap_should_keep_styled_word_parts_together() {
        let spans = [
            Span::new("xx ", Style::default()),
            Span::new("bold", Style::dim()),
            Span::new("ness", Style::default()),
        ];
        assert_eq!(plain(&wrap(&spans, 8)), ["xx", "boldness"]);
    }

    #[test]
    fn wrap_should_split_overlong_words() {
        let spans = [Span::new("abcdefgh", Style::default())];
        assert_eq!(plain(&wrap(&spans, 3)), ["abc", "def", "gh"]);
    }

    #[test]
    fn render_should_indent_list_continuation_lines() {
        let out = render_plain("- one two three four five six seven eight nine ten eleven");
        assert_eq!(
            out,
            "• one two three four five six seven\n  eight nine ten eleven\n"
        );
    }

    #[test]
    fn render_should_separate_paragraphs_with_blank_line() {
        assert_eq!(render_plain("a\n\nb"), "a\n\nb\n");
    }

    #[test]
    fn render_should_not_put_blank_lines_inside_tight_list() {
        assert_eq!(render_plain("- a\n  - b\n- c"), "• a\n  ◦ b\n• c\n");
    }

    #[test]
    fn render_should_keep_padded_inline_code_attached_to_punctuation() {
        let cfg = Config {
            width: 40,
            text_sizing: false,
            code_bg: Some(Color::Ansi(0)),
            images: None,
            headings: None,
            diagrams: None,
            first_image_id: 1,
        };
        let out = strip_ansi(&render("a `b c`, d", &cfg));
        assert_eq!(out, "a \u{a0}b c\u{a0}, d\n");
    }

    fn images_config() -> Config {
        Config {
            width: 40,
            text_sizing: false,
            code_bg: None,
            images: Some(Images {
                cell: (10, 20),
                max_rows: 10,
                base_dir: PathBuf::from("/nonexistent"),
            }),
            headings: None,
            diagrams: None,
            first_image_id: 1,
        }
    }

    #[test]
    fn render_should_fall_back_to_alt_text_when_image_missing() {
        let out = strip_ansi(&render("![cat](cat.png)", &images_config()));
        assert_eq!(out, "🖼 cat\n");
    }

    #[test]
    fn render_should_show_png_via_kitty_graphics() {
        let dir = std::env::temp_dir().join(format!("martty-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 0, 20, 0, 0, 0, 40]);
        std::fs::write(dir.join("a.png"), &png).ok();
        let mut cfg = images_config();
        if let Some(images) = &mut cfg.images {
            images.base_dir = dir;
        }
        let out = render("![a](a.png)", &cfg);
        std::fs::remove_dir_all(
            cfg.images
                .as_ref()
                .map(|i| i.base_dir.clone())
                .unwrap_or_default(),
        )
        .ok();
        assert!(out.contains("\x1b_Ga=T,U=1,i=1,f=100,t=d,q=2,c=2,r=2,"));
    }

    fn heading_config() -> Config {
        Config {
            headings: Some(HeadingImages {
                font: HeadingFont::load().unwrap_or_else(|| panic!("font")),
                cell: (10, 20),
                colors: [(255, 0, 255), (0, 0, 255), (0, 255, 255)],
            }),
            ..images_config()
        }
    }

    #[test]
    fn render_should_draw_h1_as_compressed_rgba_image() {
        let out = render("# Hello", &heading_config());
        assert!(out.contains("\x1b_Ga=T,U=1,i=1,f=32,o=z,"));
    }

    #[test]
    fn render_should_emit_image_as_placeholder_lines() {
        let doc = super::render("# Hello", &heading_config());
        let rows = doc.lines.iter().filter(|l| l.image.is_some()).count();
        assert_eq!(rows, 2);
    }

    #[test]
    fn render_should_keep_text_heading_when_glyphs_missing() {
        let out = strip_ansi(&render("# ship 🚀", &heading_config()));
        assert_eq!(out, "ship 🚀\n");
    }

    #[test]
    fn render_should_draw_h3_as_image() {
        let out = render("### Three", &heading_config());
        assert!(out.contains("\x1b_Ga=T,U=1,"));
    }

    #[test]
    fn render_should_show_h4_as_text_without_hashes() {
        let out = strip_ansi(&render("#### Four", &heading_config()));
        assert_eq!(out, "Four\n");
    }

    #[test]
    fn resolve_should_reject_remote_urls() {
        assert_eq!(resolve(Path::new("/x"), "https://e.com/a.png"), None);
    }

    #[test]
    fn render_should_prefix_quote_lines() {
        assert_eq!(render_plain("> a\n>\n> b"), "▎ a\n▎\n▎ b\n");
    }
}
