use std::mem;

use pulldown_cmark::HeadingLevel;
use unicode_width::UnicodeWidthStr;

use super::highlight::highlight;
use super::layout::Container;
use super::span::{Span, spans_width};
use super::wrap::{hard_wrap_spans, wrap};
use super::{Heading, Renderer, Source};
use crate::style::{Color, Style, Underline};

pub(super) struct CodeBlock {
    pub(super) lang: String,
    pub(super) text: String,
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
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        let anchor = self.anchors.next(&text);
        let line = self.line_no;
        self.headings.push(Heading {
            line,
            level: level as u8,
            text: text.trim().to_owned(),
            anchor,
        });
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
                    self.pen.write(&mut self.out, span, Some(2));
                }
                self.pen.finish_line(&mut self.out);
                self.out.push_str("\n\n");
                self.line_no += 2;
            }
            self.wrote_any = true;
            return;
        }

        if self.image_heading(level, &spans) {
            self.add_source(line, text.trim(), false);
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
        let start = self.line_no;
        let text = code.text.strip_suffix('\n').unwrap_or(&code.text);
        self.code_lines(code, text);
        self.add_source(start, text, true);
    }

    /// The text behind the lines from `start` up to here, for copying.
    pub(super) fn add_source(&mut self, start: usize, text: &str, code: bool) {
        if self.line_no > start {
            self.sources.push(Source {
                lines: start..self.line_no,
                text: text.to_owned(),
                code,
            });
        }
    }

    fn code_lines(&mut self, code: &CodeBlock, text: &str) {
        if code.lang == "mermaid" && self.diagram(&code.text) {
            return;
        }
        if code.lang == "math" && self.math_block(&code.text) {
            return;
        }
        let lines = highlight(&code.lang, text).unwrap_or_else(|| {
            text.lines()
                .map(|l| vec![Span::new(l.replace('\t', "    "), Style::default())])
                .collect()
        });
        let chunks: Vec<Vec<Span>> = if self.cfg.overflow {
            lines
        } else {
            let avail = self.avail();
            lines
                .iter()
                .flat_map(|line| hard_wrap_spans(line, avail.saturating_sub(2)))
                .collect()
        };
        // Unwrapped, the panel is as wide as its longest line.
        let longest = chunks.iter().map(|c| spans_width(c) + 2).max().unwrap_or(0);
        let avail = self.avail().max(longest);
        // Without a panel there is no free row for the language label, and
        // it isn't worth a line of its own.
        let Some(bg) = self.cfg.code_bg else {
            let bar = Span::new("│ ", Style::fg(Color::Ansi(8)));
            for chunk in chunks {
                let mut line = vec![bar.clone()];
                line.extend(chunk);
                self.emit_wide_line(&line);
            }
            return;
        };

        let base = Style {
            bg: Some(bg),
            ..Style::default()
        };
        // The panel's top padding row carries the language, right-aligned.
        let label = Style {
            dim: true,
            italic: true,
            ..base.clone()
        };
        let pad = avail.saturating_sub(code.lang.width() + 1);
        self.emit_wide_line(&[
            Span::new(" ".repeat(pad), base.clone()),
            Span::new(code.lang.as_str(), label),
            Span::new(" ", base.clone()),
        ]);
        for mut chunk in chunks {
            let pad = avail.saturating_sub(spans_width(&chunk) + 1);
            chunk.iter_mut().for_each(|s| s.style.bg = Some(bg));
            let mut line = vec![Span::new(" ", base.clone())];
            line.extend(chunk);
            line.push(Span::new(" ".repeat(pad), base.clone()));
            self.emit_wide_line(&line);
        }
        self.emit_wide_line(&[Span::new(" ".repeat(avail), base.clone())]);
    }

    /// Front matter as aligned `key  value` lines, dimmed. `sep` is `:` for
    /// YAML and `=` for TOML; nested or unparsed lines are shown as they are.
    pub(super) fn render_front_matter(&mut self, sep: char, text: &str) {
        self.open_block();
        let entries: Vec<(Option<&str>, &str)> = text
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .map(|line| match front_matter_entry(line, sep) {
                Some((key, value)) => (Some(key), value),
                None => (None, line),
            })
            .collect();
        let key_width = entries
            .iter()
            .filter_map(|(k, _)| k.map(UnicodeWidthStr::width))
            .max()
            .unwrap_or(0);
        let key_style = Style {
            bold: true,
            ..Style::dim()
        };
        for (key, value) in entries {
            let label = match key {
                Some(key) if value.is_empty() => {
                    self.emit_line(&[Span::new(key, key_style.clone())]);
                    continue;
                }
                Some(key) => Span::new(format!("{key:key_width$}  "), key_style.clone()),
                None => {
                    let indent = value.len() - value.trim_start().len();
                    Span::new(&value[..indent], Style::default())
                }
            };
            let value = vec![Span::new(value.trim_start(), Style::dim())];
            self.containers.push(Container::hanging(vec![label]));
            for line in wrap(&value, self.avail()) {
                self.emit_line(&line);
            }
            self.containers.pop();
        }
    }
}

/// `key: value` (YAML) or `key = value` (TOML) at the top level, with
/// surrounding quotes dropped from the value.
fn front_matter_entry(line: &str, sep: char) -> Option<(&str, &str)> {
    if line.starts_with([' ', '\t', '-', '[']) {
        return None;
    }
    let (key, value) = line.split_once(sep)?;
    let key = key.trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let value = value.trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|q| value.strip_prefix(*q)?.strip_suffix(*q));
    Some((key, unquoted.unwrap_or(value)))
}
