//! Copying to the clipboard through OSC 52: a code block, the screen or the
//! whole document, as plain text without the layout around it.

use std::ops::Range;

use super::Pager;
use super::view::{View, plain_text};
use crate::kitty;

/// OSC 52 setting the clipboard (`c`) to `text`.
pub(super) fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x1b\\", kitty::base64(text.as_bytes()))
}

/// Text of `lines` as written, not as drawn: container prefixes (quote
/// bars, bullets, indents) are dropped, and blocks drawn as images or in a
/// panel give their source (code, a diagram's mermaid, alt text, a heading).
pub(super) fn plain_lines(view: &View, lines: Range<usize>) -> String {
    let doc = &view.doc;
    let mut out: Vec<String> = Vec::new();
    let mut i = lines.start;
    while i < lines.end.min(doc.lines.len()) {
        if let Some(source) = doc.sources.iter().find(|s| s.lines.contains(&i)) {
            out.push(source.text.clone());
            i = source.lines.end;
            continue;
        }
        let line = &doc.lines[i];
        let body = line.text.get(line.prefix_len..).unwrap_or(&line.text);
        out.push(plain_text(body).trim_end().to_owned());
        i += 1;
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

impl Pager<'_> {
    fn screen_lines(&self) -> Range<usize> {
        let view = self.view();
        view.top..(view.top + self.view_rows()).min(view.total())
    }

    /// The first code block (or diagram) with a line on screen.
    pub(super) fn copy_code(&mut self) -> Option<String> {
        let screen = self.screen_lines();
        let source = self
            .view()
            .doc
            .sources
            .iter()
            .find(|s| s.code && s.lines.start < screen.end && s.lines.end > screen.start);
        let Some(source) = source else {
            self.message = Some("No code block on screen".into());
            return None;
        };
        let text = source.text.clone();
        self.message = Some(format!("Copied {} of code", lines(&text)));
        Some(text)
    }

    pub(super) fn copy_screen(&mut self, all: bool) -> String {
        let range = if all {
            0..self.view().total()
        } else {
            self.screen_lines()
        };
        let text = plain_lines(self.view(), range);
        let what = if all { "document" } else { "screen" };
        self.message = Some(format!("Copied the {what}: {}", lines(&text)));
        text
    }
}

fn lines(text: &str) -> String {
    match text.trim_end_matches('\n').lines().count() {
        1 => "1 line".into(),
        n => format!("{n} lines"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Document, Line, Source};

    #[test]
    fn plain_lines_should_drop_prefixes_and_use_sources() {
        let mut quote = Line::new("\x1b[90m▎ \x1b[0mquoted ");
        quote.prefix_len = "\x1b[90m▎ ".len();
        let doc = Document {
            preamble: String::new(),
            lines: vec![
                quote,
                Line::new("  fn main() {}  "),
                Line::new("               "),
            ],
            image_ids: Vec::new(),
            diagrams: Vec::new(),
            headings: Vec::new(),
            sources: vec![Source {
                lines: 1..3,
                text: "fn main() {}".into(),
                code: true,
            }],
        };
        let view = View::new(doc, super::super::view::Kind::Document);
        assert_eq!(plain_lines(&view, 0..3), "quoted\nfn main() {}\n");
    }

    #[test]
    fn osc52_should_encode_text() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x1b\\");
    }
}
