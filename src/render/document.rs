use std::borrow::Cow;
use std::ops::Range;

use super::slice::slice_cells;
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
    pub headings: Vec<Heading>,
    /// Text behind blocks whose lines don't hold it as plain text: code
    /// (panel padding, wrapping) and everything drawn as an image.
    pub sources: Vec<Source>,
}

/// Where a heading starts, for `#anchor` links and the table of contents.
pub struct Heading {
    pub line: usize,
    /// 1 for H1 … 6 for H6.
    pub level: u8,
    pub text: String,
    /// GitHub-style anchor, without the `#`.
    pub anchor: String,
}

/// The original text of the block drawn on `lines`, for copying.
pub struct Source {
    pub lines: Range<usize>,
    pub text: String,
    /// Code blocks and diagrams, which `y` copies; otherwise an image's alt
    /// text, a heading or a formula.
    pub code: bool,
}

pub struct Line {
    /// Ready-to-print ANSI text.
    pub text: String,
    /// Set when the line is a row of image placeholders.
    pub image: Option<ImageRow>,
    /// Bytes of `text` taken by container prefixes (quote bars, bullets,
    /// indents), which copying leaves out.
    pub prefix_len: usize,
    /// Set when the text after the prefix is wider than the screen (code
    /// and tables laid out without wrapping).
    pub wide: Option<Wide>,
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

/// A text line that overflows the screen, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wide {
    pub cols: u32,
    pub visible: u32,
}

impl ImageRow {
    /// How far this row can be panned.
    pub fn overflow(&self) -> u32 {
        self.cols.saturating_sub(self.visible).min(kitty::MAX_COL)
    }
}

impl Wide {
    pub fn overflow(self) -> u32 {
        self.cols.saturating_sub(self.visible)
    }
}

impl Line {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image: None,
            prefix_len: 0,
            wide: None,
        }
    }

    /// The line with images and text wider than the screen shifted left by
    /// `pan` cells.
    pub fn panned(&self, pan: u32) -> Cow<'_, str> {
        self.pan_text(&self.text, pan)
    }

    /// Like `panned`, for `text`: this line's text with extra escape
    /// sequences (highlights) inserted after the prefix.
    pub fn pan_text<'t>(&self, text: &'t str, pan: u32) -> Cow<'t, str> {
        if let Some(wide) = self.wide.filter(|w| w.overflow() > 0 && pan > 0) {
            let at = self.prefix_len.min(text.len());
            let shift = pan.min(wide.overflow()) as usize;
            let mut out = text[..at].to_owned();
            out.push_str(&slice_cells(&text[at..], shift, wide.visible as usize));
            return Cow::Owned(out);
        }
        let Some(img) = self.image.filter(|i| i.overflow() > 0 && pan > 0) else {
            return Cow::Borrowed(text);
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

    fn overflow(&self) -> u32 {
        let image = self.image.map_or(0, |i| i.overflow());
        image.max(self.wide.map_or(0, Wide::overflow))
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
        self.lines.iter().map(Line::overflow).max().unwrap_or(0)
    }
}
