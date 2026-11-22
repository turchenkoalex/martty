use std::mem;

use pulldown_cmark::BlockQuoteKind;

use super::span::{Pen, Span, spans_width};
use super::wrap::wrap;
use super::{Renderer, Wide};
use crate::style::{Color, Style};

/// A block quote or list item: what to print on its first line and on the rest.
pub(super) struct Container {
    pub(super) first: Vec<Span>,
    pub(super) rest: Vec<Span>,
    pub(super) used: bool,
}

impl Container {
    pub(super) fn hanging(first: Vec<Span>) -> Self {
        let rest = vec![Span::new(" ".repeat(spans_width(&first)), Style::default())];
        Self {
            first,
            rest,
            used: false,
        }
    }

    pub(super) fn repeating(prefix: Vec<Span>) -> Self {
        Self {
            first: prefix.clone(),
            rest: prefix,
            used: false,
        }
    }
}

impl Renderer<'_> {
    /// A block quote, or a GitHub alert (`> [!NOTE]`): a bar in the alert's
    /// color and a title line with its icon.
    pub(super) fn start_quote(&mut self, kind: Option<BlockQuoteKind>) {
        self.open_block();
        let Some(kind) = kind else {
            let bar = Span::new("▎ ", Style::fg(Color::Ansi(8)));
            self.containers.push(Container::repeating(vec![bar]));
            return;
        };
        // Text-presentation symbols, one cell wide, so they take the color.
        let (icon, title, color) = match kind {
            BlockQuoteKind::Note => ("ⓘ", "Note", 4),
            BlockQuoteKind::Tip => ("✦", "Tip", 2),
            BlockQuoteKind::Important => ("‼", "Important", 5),
            BlockQuoteKind::Warning => ("⚠", "Warning", 3),
            BlockQuoteKind::Caution => ("⊘", "Caution", 1),
        };
        let color = Color::Ansi(color);
        let bar = Span::new("▎ ", Style::fg(color));
        self.containers.push(Container::repeating(vec![bar]));
        let style = Style {
            bold: true,
            ..Style::fg(color)
        };
        self.emit_line(&[Span::new(format!("{icon} {title}"), style)]);
    }

    pub(super) fn open_block(&mut self) {
        self.flush_inline();
        if self.pending_blank && self.wrote_any {
            self.blank_line();
        }
        self.pending_blank = false;
    }

    pub(super) fn close_block(&mut self) {
        self.flush_inline();
        self.reset_html();
        self.pending_blank = true;
    }

    pub(super) fn flush_inline(&mut self) {
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
            let start = self.line_no;
            let shown = self.cfg.images.is_some() && self.emit_image(&image.url);
            if shown {
                let alt = spans.get(image.alt.clone()).unwrap_or_default();
                let alt: String = alt.iter().map(|s| s.text.as_str()).collect();
                self.add_source(start, alt.trim(), false);
            }
            if only_images && !shown {
                let alt = spans.get(image.alt.clone()).unwrap_or_default();
                for line in wrap(alt, self.avail()) {
                    self.emit_line(&line);
                }
            }
        }
    }

    pub(super) fn prefix_width(&self) -> usize {
        self.containers.iter().map(|c| spans_width(&c.rest)).sum()
    }

    pub(super) fn avail(&self) -> usize {
        self.cfg.width.saturating_sub(self.prefix_width()).max(8)
    }

    pub(super) fn emit_line(&mut self, content: &[Span]) {
        self.write_prefix();
        for span in content {
            self.pen.write(&mut self.out, span, None);
        }
        self.end_line();
    }

    /// A line of code or table that may be wider than the available width
    /// (`Config::overflow`); the pager pans it.
    pub(super) fn emit_wide_line(&mut self, content: &[Span]) {
        let cols = spans_width(content);
        let visible = self.avail();
        if cols > visible {
            let to_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
            let wide = Wide {
                cols: to_u32(cols),
                visible: to_u32(visible),
            };
            self.wide_rows.push((self.line_no, wide));
        }
        self.emit_line(content);
    }

    pub(super) fn write_prefix(&mut self) {
        let start = self.out.len();
        for c in &mut self.containers {
            let prefix = if c.used { &c.rest } else { &c.first };
            for span in prefix {
                self.pen.write(&mut self.out, span, None);
            }
            c.used = true;
        }
        if self.out.len() > start {
            self.prefixes.push((self.line_no, self.out.len() - start));
        }
    }

    pub(super) fn end_line(&mut self) {
        self.pen.finish_line(&mut self.out);
        self.out.push('\n');
        self.line_no += 1;
        self.wrote_any = true;
    }

    /// Blank separator: keeps quote bars, never consumes a pending bullet.
    pub(super) fn blank_line(&mut self) {
        let mut line = String::new();
        let mut pen = Pen::new(self.cfg.color);
        for c in self.containers.iter().take_while(|c| c.used) {
            for span in &c.rest {
                pen.write(&mut line, span, None);
            }
        }
        let line = line.trim_end_matches(' ');
        if !line.is_empty() {
            self.prefixes.push((self.line_no, line.len()));
        }
        self.out.push_str(line);
        pen.finish_line(&mut self.out);
        self.out.push('\n');
        self.line_no += 1;
    }
}
