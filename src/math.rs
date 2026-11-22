//! LaTeX formulas: latex-rust lays them out with TeX's rules in STIX Two Math
//! and emits SVG with glyph outlines as paths, resvg rasterizes that at the
//! terminal's text size into whole cells, and kitty graphics shows it.

use resvg::{tiny_skia, usvg};

use crate::picture::svg;
use crate::term::Rgb;

/// Terminal cells are roughly this much taller than the font size (as in
/// `mermaid.rs`).
const CELL_TO_FONT: f32 = 1.25;
/// Text baseline, as a fraction of the cell height from its top: where
/// Ghostty puts it for JetBrains Mono, which centres ascent + descent
/// (1.32 em) in the cell.
const BASELINE: f32 = 0.79;
/// An inline formula taller than a row is shrunk to fit, but not below
/// this much of the text size; past that it is shown as code.
const MIN_INLINE_SCALE: f32 = 0.6;
/// Font size handed to latex-rust; only the ratio to it matters.
const EM_PT: i64 = 12;

pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
    /// Straight (non-premultiplied) RGBA, transparent around the ink.
    pub rgba: Vec<u8>,
}

/// A display formula (`$$…$$`): TeX display style, at the terminal's text
/// size, as many rows as it needs. `None` when latex-rust can't render it.
pub fn display(latex: &str, color: Rgb, cell: (u32, u32)) -> Option<Raster> {
    let svg = to_svg(latex, color, true)?;
    let (cw, ch) = (cell.0.max(1) as f32, cell.1.max(1) as f32);
    let scale = px_per_pt(ch);
    let (w, h) = (svg.width * scale, svg.height * scale);
    let cols = (w / cw).ceil().max(1.0) as u32;
    let rows = (h / ch).ceil().max(1.0) as u32;
    let dx = (cols as f32 * cw - w) / 2.0;
    let dy = (rows as f32 * ch - h) / 2.0;
    rasterize(&svg, cell, (cols, rows), scale, (dx, dy))
}

/// An inline formula (`$…$`) in one row, its baseline on the text's.
/// `None` when it can't be rendered or would be too small to read.
pub fn inline(latex: &str, color: Rgb, cell: (u32, u32)) -> Option<Raster> {
    let svg = to_svg(latex, color, false)?;
    let (cw, ch) = (cell.0.max(1) as f32, cell.1.max(1) as f32);
    let natural = px_per_pt(ch);
    let scale = natural.min(ch / svg.height);
    if scale < natural * MIN_INLINE_SCALE {
        return None;
    }
    let (w, h) = (svg.width * scale, svg.height * scale);
    let cols = (w / cw).ceil().max(1.0) as u32;
    let dx = (cols as f32 * cw - w) / 2.0;
    let dy = (BASELINE * ch - svg.baseline * scale).clamp(0.0, ch - h);
    rasterize(&svg, cell, (cols, 1), scale, (dx, dy))
}

/// Pixels per point that make the formula's em the terminal's font size.
fn px_per_pt(cell_h: f32) -> f32 {
    cell_h / CELL_TO_FONT / EM_PT as f32
}

struct Svg {
    tree: usvg::Tree,
    /// Size of the ink box, in points.
    width: f32,
    height: f32,
    /// From the top of the ink box down to the baseline, in points.
    baseline: f32,
}

#[cfg(feature = "math")]
fn to_svg(latex: &str, color: Rgb, display: bool) -> Option<Svg> {
    use std::sync::OnceLock;

    use latex_rust::{Dim, MathFont, MathStyle, SvgOptions};

    static FONT: OnceLock<Option<MathFont>> = OnceLock::new();
    let font = FONT
        .get_or_init(|| MathFont::stix_two_math().ok())
        .as_ref()?;
    let style = if display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let mut options = SvgOptions::new();
    options.font_size_pt = Dim::from_i64(EM_PT);
    options.color = latex_rust::Color::rgb(color.0, color.1, color.2);
    let ast = latex_rust::parse(latex.trim()).ok()?;
    let boxed =
        latex_rust::layout_with_em_size_pt(&ast, font, style, &options.font_size_pt).ok()?;
    let image = latex_rust::render_svg_with_metrics(&boxed, font, &options).ok()?;
    // Glyphs are paths, so usvg needs no fonts.
    let tree = usvg::Tree::from_str(&image.svg, &usvg::Options::default()).ok()?;
    let pt = |d: &Dim| f32::from_bits(d.to_ieee32_bits());
    let (width, height) = (pt(&image.width_pt), pt(&image.height_pt));
    (width > 0.0 && height > 0.0).then(|| Svg {
        tree,
        width,
        height,
        baseline: pt(&image.baseline_pt),
    })
}

#[cfg(not(feature = "math"))]
fn to_svg(_latex: &str, _color: Rgb, _display: bool) -> Option<Svg> {
    None
}

/// Draws `svg` at `scale` pixels per point, shifted by `offset` pixels,
/// into a transparent box of whole cells.
fn rasterize(
    svg: &Svg,
    cell: (u32, u32),
    (cols, rows): (u32, u32),
    scale: f32,
    offset: (f32, f32),
) -> Option<Raster> {
    let (width, height) = (cols * cell.0, rows * cell.1);
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    // The SVG size is given in `pt`, which usvg turns into CSS pixels
    // (96 dpi); the transform maps those to ours.
    let fit = scale * svg.width / svg.tree.size().width();
    let transform = tiny_skia::Transform::from_row(fit, 0.0, 0.0, fit, offset.0, offset.1);
    resvg::render(&svg.tree, transform, &mut pixmap.as_mut());
    let mut rgba = pixmap.take();
    svg::unpremultiply(&mut rgba);
    Some(Raster {
        width,
        height,
        cols,
        rows,
        rgba,
    })
}

#[cfg(all(test, feature = "math"))]
mod tests {
    use super::*;

    const CELL: (u32, u32) = (10, 24);

    #[test]
    fn inline_should_fill_one_row_of_whole_cells() {
        let r = inline("e = mc^2", (255, 255, 255), CELL);
        let dims = r.map(|r| (r.width, r.height, r.cols, r.rows));
        let Some((w, h, cols, rows)) = dims else {
            panic!("formula should render");
        };
        assert_eq!((w, h, rows), (cols * 10, 24, 1));
    }

    #[test]
    fn display_should_take_several_rows_for_a_fraction() {
        let r = display(r"\frac{a}{b}", (255, 255, 255), CELL);
        assert!(r.is_some_and(|r| r.rows >= 2));
    }

    #[test]
    fn inline_should_refuse_tall_formula() {
        let tall = r"\begin{pmatrix} a \\ b \\ c \\ d \end{pmatrix}";
        assert!(inline(tall, (255, 255, 255), CELL).is_none());
    }

    #[test]
    fn unknown_command_should_not_render() {
        assert!(display(r"\notacommand x", (255, 255, 255), CELL).is_none());
    }
}
