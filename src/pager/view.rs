use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::SystemTime;

use unicode_width::UnicodeWidthChar;

use super::links::{self, Link};
use super::search::Hit;
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
    pub(super) links: Vec<Link>,
    /// Index into `links` of the link Tab moved to.
    pub(super) selected: Option<usize>,
    pub(super) top: usize,
    /// Horizontal shift of overflowing images, in cells.
    pub(super) pan: u32,
    /// Diagram zoom; 1.0 fits the screen.
    pub(super) zoom: f32,
    pub(super) kind: Kind,
    /// `m<letter>` marks: letter → line. `'` is the line before the last jump.
    pub(super) marks: HashMap<char, usize>,
    /// Matches of the query the hits were found for, in document order.
    pub(super) hits: Option<(String, Vec<Hit>)>,
    /// Line of the match `n`/`N` moved to last.
    pub(super) current_hit: Option<usize>,
    /// Modification time of the file shown, for following changes.
    pub(super) modified: Option<SystemTime>,
    /// For a full-screen diagram: its index in the `diagrams` of the view
    /// below, for switching to its neighbours.
    pub(super) diagram: Option<usize>,
}

impl View {
    pub(super) fn new(doc: Document, kind: Kind) -> Self {
        let links = links::find(&doc.lines);
        Self {
            doc,
            links,
            selected: None,
            top: 0,
            pan: 0,
            zoom: 1.0,
            kind,
            marks: HashMap::new(),
            hits: None,
            current_hit: None,
            modified: None,
            diagram: None,
        }
    }

    pub(super) fn is_diagram(&self) -> bool {
        matches!(self.kind, Kind::Diagram(_))
    }

    /// Byte range of the selected link's text on line `index`, if any.
    pub(super) fn selected_range(&self, index: usize) -> Option<Range<usize>> {
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

    /// Whether a screen starting at `top` splits a heading drawn as an
    /// image between the screen and the lines above or below it.
    pub(super) fn cuts_heading(&self, top: usize, rows: usize) -> bool {
        let lines = &self.doc.lines;
        // Lines `i` and `i + 1` are rows of the same heading image.
        let joined = |i: usize| {
            let image = |i: usize| lines.get(i).and_then(|l| l.image);
            let (Some(a), Some(b)) = (image(i), image(i + 1)) else {
                return false;
            };
            a.id == b.id
                && i.checked_sub(a.row as usize).is_some_and(|start| {
                    self.doc
                        .headings
                        .binary_search_by_key(&start, |h| h.line)
                        .is_ok()
                })
        };
        top.checked_sub(1).is_some_and(joined) || joined(top + rows - 1)
    }

    /// Index into `doc.diagrams` of the image on line `index`, if it is a
    /// diagram.
    pub(super) fn diagram_at(&self, index: usize) -> Option<usize> {
        let id = self.doc.lines.get(index)?.image?.id;
        self.doc.diagrams.iter().position(|(d, _)| *d == id)
    }

    /// Where `line` of `old` (the same content laid out differently, or an
    /// edited version of it) is in this view: the same distance into the
    /// same section, as a share of the section's length, so the reader
    /// stays at the paragraph they were reading. Without headings in
    /// common, the same share of the whole document.
    pub(super) fn follow_line(&self, old: &View, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        let section = |view: &View, i: usize| {
            let start = view.doc.headings[i].line;
            let end = view
                .doc
                .headings
                .get(i + 1)
                .map_or(view.total(), |h| h.line);
            (start, end.saturating_sub(start).max(1))
        };
        let same = old
            .doc
            .headings
            .iter()
            .rposition(|h| h.line <= line)
            .and_then(|i| {
                let anchor = &old.doc.headings[i].anchor;
                let j = self.doc.headings.iter().position(|h| &h.anchor == anchor)?;
                Some((section(old, i), section(self, j)))
            });
        let new = match same {
            Some(((start, len), (new_start, new_len))) => {
                new_start + (line - start) * new_len / len
            }
            None => line * self.total() / old.total().max(1),
        };
        new.min(self.total().saturating_sub(1))
    }
}

/// Visible characters of a rendered line with their byte ranges in it:
/// escape sequences and image placeholder cells (with their row/column
/// diacritics) left out.
pub(super) fn visible_chars(line: &str) -> Vec<(char, Range<usize>)> {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == kitty::PLACEHOLDER {
            while chars.next_if(|(_, d)| d.width() == Some(0)).is_some() {}
            continue;
        }
        if c != '\x1b' {
            out.push((c, i..i + c.len_utf8()));
            continue;
        }
        match chars.next() {
            Some((_, '[')) => {
                for (_, c) in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some((_, ']' | '_')) => {
                while let Some((_, c)) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if(|(_, d)| *d == '\\').is_some())
                    {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

pub(super) fn plain_text(line: &str) -> String {
    visible_chars(line).into_iter().map(|(c, _)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Heading, Line};

    fn doc(total: usize, headings: &[(usize, &str)]) -> View {
        let doc = Document {
            preamble: String::new(),
            uploads: Vec::new(),
            lines: (0..total).map(|_| Line::new("x")).collect(),
            image_ids: Vec::new(),
            diagrams: Vec::new(),
            headings: headings
                .iter()
                .map(|&(line, anchor)| Heading {
                    line,
                    level: 2,
                    text: anchor.to_owned(),
                    anchor: anchor.to_owned(),
                })
                .collect(),
            sources: Vec::new(),
        };
        View::new(doc, Kind::Document)
    }

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

    #[test]
    fn follow_line_should_keep_place_within_section() {
        let old = doc(100, &[(0, "a"), (50, "b")]);
        let new = doc(200, &[(0, "a"), (120, "b")]);
        // Halfway through section `b` stays halfway through it.
        assert_eq!(new.follow_line(&old, 75), 160);
    }

    #[test]
    fn follow_line_should_fall_back_to_share_of_document() {
        let old = doc(100, &[]);
        let new = doc(50, &[]);
        assert_eq!(new.follow_line(&old, 40), 20);
    }
}
