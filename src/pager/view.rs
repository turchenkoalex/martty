use crate::render::Document;

/// One screenful of content with its own scroll, pan and zoom.
pub(super) struct View {
    pub(super) doc: Document,
    /// Text of each line for searching (images contribute nothing).
    pub(super) plain: Vec<String>,
    pub(super) top: usize,
    /// Horizontal shift of overflowing images, in cells.
    pub(super) pan: u32,
    /// Diagram zoom; 1.0 fits the screen.
    pub(super) zoom: f32,
    /// Set for a full-screen diagram.
    pub(super) source: Option<String>,
}

impl View {
    pub(super) fn new(doc: Document, source: Option<String>) -> Self {
        let plain = doc.lines.iter().map(|l| plain_text(&l.text)).collect();
        Self {
            doc,
            plain,
            top: 0,
            pan: 0,
            zoom: 1.0,
            source,
        }
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

/// Visible text of a rendered line: escape sequences removed, image
/// placeholder lines reduced to nothing.
pub(super) fn plain_text(line: &str) -> String {
    if line.contains('\u{10EEEE}') {
        return String::new();
    }
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
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
}
