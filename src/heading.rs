//! Big headings as images. Ghostty has no OSC 66 yet, so we rasterize the
//! heading ourselves with JetBrains Mono (Ghostty's default font) into a
//! box of whole cells, so the terminal shows it pixel-for-pixel.

use fontdue::{Font, FontSettings};

use crate::term::Rgb;

static FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");

pub struct HeadingFont(Font);

pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    /// Straight (non-premultiplied) RGBA.
    pub rgba: Vec<u8>,
}

impl HeadingFont {
    pub fn load() -> Option<Self> {
        Font::from_bytes(FONT_BYTES, FontSettings::default())
            .ok()
            .map(Self)
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
        let px = self.px_for(cell.1, scale)?;
        let lm = self.0.horizontal_line_metrics(px)?;
        let advance = self.advance(text, px);
        let cols = (advance / cell.0 as f32).ceil().max(1.0) as u32;
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
            cols,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> HeadingFont {
        HeadingFont::load().unwrap_or_else(|| panic!("bundled font must load"))
    }

    #[test]
    fn render_should_fill_exact_cell_box() {
        let bmp = font().render("Hello", (10, 20), 2.0, 2, (255, 0, 0));
        let (w, h, cols) = bmp.map(|b| (b.width, b.height, b.cols)).unwrap_or_default();
        assert_eq!((w, h), (cols * 10, 40));
    }

    #[test]
    fn render_should_draw_something() {
        let bmp = font().render("Hi", (10, 20), 2.0, 2, (255, 0, 0));
        let inked = bmp.is_some_and(|b| b.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        assert!(inked);
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
