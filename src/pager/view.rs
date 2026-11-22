use unicode_width::UnicodeWidthChar;

use std::path::PathBuf;

use super::links::{self, Link};
use crate::kitty;
use crate::render::Document;

/// What a view shows, and so how to lay it out again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Kind {
    /// The document martty was started with.
    Document,
    /// A Markdown file opened from a link.
    File(PathBuf),
    /// One diagram, full screen.
    Diagram(String),
}

/// One screenful of content with its own scroll, pan and zoom.
pub(super) struct View {
    pub(super) doc: Document,
    /// Text of each line for searching (images contribute nothing).
    pub(super) plain: Vec<String>,
    pub(super) links: Vec<Link>,
    /// Index into `links` of the link Tab moved to.
    pub(super) selected: Option<usize>,
    pub(super) top: usize,
    /// Horizontal shift of overflowing images, in cells.
    pub(super) pan: u32,
    /// Diagram zoom; 1.0 fits the screen.
    pub(super) zoom: f32,
    pub(super) kind: Kind,
}

impl View {
    pub(super) fn new(doc: Document, kind: Kind) -> Self {
        let plain = doc.lines.iter().map(|l| plain_text(&l.text)).collect();
        let links = links::find(&doc.lines);
        Self {
            doc,
            plain,
            links,
            selected: None,
            top: 0,
            pan: 0,
            zoom: 1.0,
            kind,
        }
    }

    pub(super) fn is_diagram(&self) -> bool {
        matches!(self.kind, Kind::Diagram(_))
    }

    /// Byte range of the selected link's text on line `index`, if any.
    pub(super) fn selected_range(&self, index: usize) -> Option<std::ops::Range<usize>> {
        let link = self.links.get(self.selected?)?;
        link.parts
            .iter()
            .find(|(line, _)| *line == index)
            .map(|(_, range)| range.clone())
    }

    /// Line where the heading with this GitHub-style anchor starts.
    pub(super) fn anchor_line(&self, anchor: &str) -> Option<usize> {
        let anchor = anchor.to_lowercase();
        self.doc
            .headings
            .iter()
            .find(|h| h.anchor == anchor)
            .map(|h| h.line)
    }

    pub(super) fn total(&self) -> usize {
        self.doc.lines.len()
    }

    /// Diagram source of the image on line `index`, if it is a diagram.
    pub(super) fn diagram_at(&self, index: usize) -> Option<&str> {
        let id = self.doc.lines.get(index)?.image?.id;
        self.doc
            .diagrams
            .iter()
            .find(|(d, _)| *d == id)
            .map(|(_, src)| src.as_str())
    }
}

/// Visible text of a rendered line: escape sequences and image placeholder
/// cells (with their row/column diacritics) removed.
pub(super) fn plain_text(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == kitty::PLACEHOLDER {
            while chars.next_if(|d| d.width() == Some(0)).is_some() {}
            continue;
        }
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']' | '_') => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_should_strip_sgr_and_hyperlinks() {
        let line = "\x1b[0;1mbold\x1b[0m \x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\";
        assert_eq!(plain_text(line), "bold link");
    }

    #[test]
    fn plain_text_should_keep_text_around_placeholders() {
        let line = format!("a {} b", kitty::placeholder_row(0, 0, 3));
        assert_eq!(plain_text(&line), "a  b");
    }
}
