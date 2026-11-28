//! Big headings as images. Ghostty has no OSC 66 yet, so we rasterize the
//! heading ourselves into a box of whole cells, so the terminal shows it
//! pixel-for-pixel. The font is the bold face of the terminal's font (Ghostty's
//! `font-family`, or the one chosen in Rex) when it is installed, else the
//! bundled JetBrains Mono (Ghostty's default).

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};
use resvg::usvg::fontdb;

use crate::picture::svg;
use crate::term::Rgb;

const BUNDLED_FAMILY: &str = "JetBrains Mono";

/// The font is parsed once and shared by every layout, and by the threads
/// that draw headings.
#[derive(Clone, Copy)]
pub struct HeadingFont(&'static Font);

pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA.
    pub rgba: Vec<u8>,
}

impl HeadingFont {
    /// The font, found and parsed once per process.
    pub fn load() -> Option<Self> {
        static FONT: OnceLock<Option<Font>> = OnceLock::new();
        // Tests measure headings in the bundled font, whatever is installed.
        FONT.get_or_init(|| if cfg!(test) { None } else { configured() }.or_else(bundled))
            .as_ref()
            .map(Self)
    }

    /// Starts finding the font in the background (asking the terminal's
    /// config, maybe loading system fonts), so it overlaps the terminal probe.
    pub fn prefetch() {
        std::thread::spawn(Self::load);
    }

    /// Whether every visible char has a glyph (no tofu for emoji etc.).
    pub fn supports(&self, text: &str) -> bool {
        text.chars()
            .all(|c| c.is_whitespace() || self.0.lookup_glyph_index(c) != 0)
    }

    /// Font size in px at which one text line is `scale` cells tall.
    fn px_for(&self, cell_h: u32, scale: f32) -> Option<f32> {
        let unit = self.0.horizontal_line_metrics(1.0)?.new_line_size;
        (unit > 0.0).then(|| cell_h as f32 * scale / unit)
    }

    /// Cells one line of `text` takes at `scale`, as [`Self::render`]
    /// draws it.
    pub fn cols(&self, text: &str, cell: (u32, u32), scale: f32) -> Option<u32> {
        let px = self.px_for(cell.1, scale)?;
        self.0.horizontal_line_metrics(px)?;
        let advance = self.advance(text, px);
        Some((advance / cell.0 as f32).ceil().max(1.0) as u32)
    }

    /// Renders one line of `text`, `rows` cells tall and vertically centred.
    /// The width is rounded up to whole cells.
    pub fn render(
        &self,
        text: &str,
        cell: (u32, u32),
        scale: f32,
        rows: u32,
        color: Rgb,
    ) -> Option<Bitmap> {
        let cols = self.cols(text, cell, scale)?;
        let px = self.px_for(cell.1, scale)?;
        let lm = self.0.horizontal_line_metrics(px)?;
        let (width, height) = (cols * cell.0, rows * cell.1);
        let (w, h) = (width as usize, height as usize);

        let mut rgba = [color.0, color.1, color.2, 0].repeat(w * h);
        let baseline = (height as f32 - (lm.ascent - lm.descent)) / 2.0 + lm.ascent;
        let mut pen = 0.0_f32;
        let mut prev = None;
        for ch in text.chars() {
            if let Some(p) = prev {
                pen += self.0.horizontal_kern(p, ch, px).unwrap_or(0.0);
            }
            let (m, coverage) = self.0.rasterize(ch, px);
            // fontdue's ymin is the glyph bottom relative to the baseline.
            let x0 = (pen + m.xmin as f32).round() as isize;
            let y0 = (baseline - (m.ymin as f32 + m.height as f32)).round() as isize;
            for (gy, row) in coverage.chunks_exact(m.width.max(1)).enumerate() {
                let Ok(y) = usize::try_from(y0 + gy as isize) else {
                    continue;
                };
                for (gx, &a) in row.iter().enumerate() {
                    let Ok(x) = usize::try_from(x0 + gx as isize) else {
                        continue;
                    };
                    if x < w && y < h {
                        let alpha = &mut rgba[(y * w + x) * 4 + 3];
                        *alpha = (*alpha).max(a);
                    }
                }
            }
            pen += m.advance_width;
            prev = Some(ch);
        }
        Some(Bitmap {
            width,
            height,
            rgba,
        })
    }

    fn advance(&self, text: &str, px: f32) -> f32 {
        let mut prev = None;
        text.chars()
            .map(|ch| {
                let kern = prev
                    .and_then(|p| self.0.horizontal_kern(p, ch, px))
                    .unwrap_or(0.0);
                prev = Some(ch);
                kern + self.0.metrics(ch, px).advance_width
            })
            .sum()
    }
}

fn bundled() -> Option<Font> {
    Font::from_bytes(svg::FONT_BOLD, FontSettings::default()).ok()
}

/// The bold face of the terminal's font, when it isn't the bundled one and
/// the system has it. Only then are the system fonts loaded, which takes a
/// while.
fn configured() -> Option<Font> {
    let family = crate::terminal::current().font_family()?;
    if family.eq_ignore_ascii_case(BUNDLED_FAMILY) {
        return None;
    }
    let db = svg::fonts();
    let id = db.query(&fontdb::Query {
        families: &[fontdb::Family::Name(&family)],
        weight: fontdb::Weight::BOLD,
        ..fontdb::Query::default()
    })?;
    db.with_face_data(id, |data, index| {
        let settings = FontSettings {
            collection_index: index,
            ..FontSettings::default()
        };
        Font::from_bytes(data, settings).ok()
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> HeadingFont {
        HeadingFont::load().unwrap_or_else(|| panic!("bundled font must load"))
    }

    #[test]
    fn render_should_fill_exact_cell_box() {
        let bmp = font().render("Hello", (10, 20), 2.0, 2, (255, 0, 0));
        let (w, h) = bmp.map(|b| (b.width, b.height)).unwrap_or_default();
        assert_eq!((w % 10, h), (0, 40));
    }

    #[test]
    fn render_should_draw_something() {
        let bmp = font().render("Hi", (10, 20), 2.0, 2, (255, 0, 0));
        let inked = bmp.is_some_and(|b| b.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        assert!(inked);
    }

    #[test]
    fn cols_should_match_rendered_width() {
        let f = font();
        let cols = f.cols("Heading", (10, 20), 1.5);
        let bmp = f.render("Heading", (10, 20), 1.5, 2, (0, 0, 0));
        assert_eq!(cols, bmp.map(|b| b.width / 10));
    }

    #[test]
    fn supports_should_accept_cyrillic() {
        assert!(font().supports("Привет мир"));
    }

    #[test]
    fn supports_should_reject_emoji() {
        assert!(!font().supports("ship it 🚀"));
    }
}
