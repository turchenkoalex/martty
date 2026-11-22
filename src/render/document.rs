use std::borrow::Cow;

use crate::kitty;
use crate::style::{Color, Style};

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
