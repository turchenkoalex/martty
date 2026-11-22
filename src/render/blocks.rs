use std::mem;

use pulldown_cmark::{Alignment, HeadingLevel};
use unicode_width::UnicodeWidthStr;

use super::Renderer;
use super::span::{Span, spans_width, write_span};
use super::wrap::{hard_wrap, wrap};
use crate::style::{Color, Style, Underline};

pub(super) struct CodeBlock {
    pub(super) lang: String,
    pub(super) text: String,
}

pub(super) type Cell = Vec<Span>;

pub(super) struct Table {
    pub(super) aligns: Vec<Alignment>,
    pub(super) rows: Vec<Vec<Cell>>,
    pub(super) header_rows: usize,
    pub(super) row: Vec<Cell>,
}

impl Renderer<'_> {
    pub(super) fn rule(&mut self) {
        self.open_block();
        let line = Span::new("─".repeat(self.avail()), Style::fg(Color::Ansi(8)));
        self.emit_line(&[line]);
        self.pending_blank = true;
    }

    pub(super) fn render_heading(&mut self) {
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

    pub(super) fn render_code(&mut self, code: &CodeBlock) {
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

    pub(super) fn render_table(&mut self, table: &Table) {
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
