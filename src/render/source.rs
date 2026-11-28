//! The document as its Markdown source (`s` in the pager): highlighted as
//! Markdown, with line numbers in a gutter, to see what the renderer was
//! given. Headings get the anchors the rendered document gives them, so the
//! pager keeps the place when switching between the two.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::deflist::escape_false_markers;
use super::emoji::replace_shortcodes;
use super::highlight::highlight;
use super::links::Anchors;
use super::span::{Pen, Span, spans_width};
use super::wrap::hard_wrap_spans;
use super::{Config, Document, Heading, Line, Source, Wide, options};
use crate::style::{Color, Style};

pub fn source(markdown: &str, config: &Config) -> Document {
    // A CRLF file would otherwise show `␍` at the end of every line.
    let markdown = markdown.replace("\r\n", "\n");
    let originals: Vec<&str> = markdown.lines().collect();
    let highlighted = highlight("markdown", &markdown).unwrap_or_else(|| {
        originals
            .iter()
            .map(|l| vec![Span::new(l.replace('\t', "    "), Style::default())])
            .collect()
    });
    let digits = originals.len().max(1).to_string().len();
    // `12 │ `, in cells.
    let gutter = digits + 3;
    let avail = config.width.saturating_sub(gutter).max(8);
    let to_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);

    let mut doc = Document::default();
    // Source line → its first row on screen.
    let mut first_row = Vec::with_capacity(originals.len());
    for (i, spans) in highlighted.into_iter().enumerate() {
        let start = doc.lines.len();
        first_row.push(start);
        let mut chunks = if config.overflow {
            vec![spans]
        } else {
            hard_wrap_spans(&spans, avail)
        };
        if chunks.is_empty() {
            chunks.push(Vec::new());
        }
        for (k, chunk) in chunks.iter().enumerate() {
            let number = if k == 0 {
                format!("{:>digits$} │ ", i + 1)
            } else {
                format!("{:digits$} │ ", "")
            };
            let mut pen = Pen::new(config.color);
            let mut text = String::new();
            pen.write(
                &mut text,
                &Span::new(number, Style::fg(Color::Ansi(8))),
                None,
            );
            pen.finish_line(&mut text);
            let prefix_len = text.len();
            for span in chunk {
                pen.write(&mut text, span, None);
            }
            pen.finish_line(&mut text);
            let cols = spans_width(chunk);
            doc.lines.push(Line {
                text,
                image: None,
                prefix_len,
                wide: (cols > avail).then(|| Wide {
                    cols: to_u32(cols),
                    visible: to_u32(avail),
                }),
            });
        }
        // A wrapped line is copied as the one line it is in the file.
        if doc.lines.len() - start > 1
            && let Some(original) = originals.get(i)
        {
            doc.sources.push(Source {
                lines: start..doc.lines.len(),
                text: (*original).to_owned(),
                code: false,
            });
        }
    }
    doc.headings = headings(&markdown, &first_row);
    doc
}

/// Headings with the text and anchors `render` gives them, at the row
/// where their source line starts.
fn headings(markdown: &str, first_row: &[usize]) -> Vec<Heading> {
    let options = options();
    // As `render` parses it, so both find the same headings.
    let parsed = escape_false_markers(markdown, options);
    let mut anchors = Anchors::default();
    let mut headings = Vec::new();
    let mut open: Option<(u8, usize, String)> = None;
    for (event, range) in Parser::new_ext(&parsed, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let before = parsed.get(..range.start).unwrap_or_default();
                open = Some((level as u8, before.matches('\n').count(), String::new()));
            }
            Event::Text(t) => {
                if let Some((_, _, text)) = &mut open {
                    text.push_str(&replace_shortcodes(&t));
                }
            }
            Event::Code(t) => {
                if let Some((_, _, text)) = &mut open {
                    text.push_str(&t);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, line, text)) = open.take() {
                    let last = first_row.last().copied().unwrap_or(0);
                    headings.push(Heading {
                        line: first_row.get(line).copied().unwrap_or(last),
                        level,
                        anchor: anchors.next(&text),
                        text: text.trim().to_owned(),
                    });
                }
            }
            _ => {}
        }
    }
    headings
}

#[cfg(test)]
mod tests {
    use super::super::tests::{plain_config, strip_ansi};
    use super::*;

    fn rows(doc: &Document) -> Vec<String> {
        doc.lines.iter().map(|l| strip_ansi(&l.text)).collect()
    }

    #[test]
    fn source_should_number_lines_and_keep_markup() {
        let doc = source("# Title\n\n- **a**\r\n", &plain_config());
        assert_eq!(rows(&doc), ["1 │ # Title", "2 │ ", "3 │ - **a**"]);
        let line = &doc.lines[2];
        assert_eq!(strip_ansi(&line.text[..line.prefix_len]), "3 │ ");
    }

    #[test]
    fn source_should_wrap_long_lines_under_a_blank_number() {
        let mut config = plain_config();
        config.width = 12;
        let doc = source("abcdefghijkl\n", &config);
        assert_eq!(rows(&doc), ["1 │ abcdefgh", "  │ ijkl"]);
        assert_eq!(doc.sources[0].text, "abcdefghijkl");
    }

    #[test]
    fn source_should_pan_long_lines_with_overflow() {
        let mut config = plain_config();
        config.width = 12;
        config.overflow = true;
        let doc = source("abcdefghijkl\n", &config);
        assert_eq!(doc.lines.len(), 1);
        assert_eq!(doc.max_pan(), 4);
    }

    #[test]
    fn headings_should_match_rendered_anchors_and_rows() {
        let mut config = plain_config();
        config.width = 12;
        let md = "abcdefghijkl\n\n## `Two` :rocket:\n\n```\n# no\n```\n\nThree\n---\n";
        let doc = source(md, &config);
        let rendered = super::super::render(md, &config);
        let anchors =
            |d: &Document| -> Vec<String> { d.headings.iter().map(|h| h.anchor.clone()).collect() };
        assert_eq!(anchors(&doc), anchors(&rendered));
        // Line 1 wraps to two rows and line 3 to three.
        let lines: Vec<usize> = doc.headings.iter().map(|h| h.line).collect();
        assert_eq!(lines, [3, 11]);
    }

    #[test]
    fn source_should_highlight_markdown() {
        let doc = source("# Title\n", &plain_config());
        assert!(doc.lines[0].text[doc.lines[0].prefix_len..].contains("\x1b["));
    }
}
