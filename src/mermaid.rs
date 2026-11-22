//! Mermaid diagrams: mermaid-rs-renderer lays them out as SVG, resvg
//! rasterizes that at the terminal's own text size into a box of whole
//! cells, and kitty graphics shows it. No browser involved.

use std::sync::OnceLock;

use mermaid_rs_renderer::{RenderOptions, Theme};
use resvg::{tiny_skia, usvg};

use crate::kitty;
use crate::term::{Rgb, luma, mix};

const FONT_FAMILY: &str = "\"JetBrains Mono\", Menlo, monospace";
const FONT_SIZE: f32 = 14.0;
/// Terminal cells are roughly this much taller than the font size.
const CELL_TO_FONT: f32 = 1.25;
/// Bound on the raster (RGBA), whatever the zoom.
const MAX_PIXELS: f32 = 16_000_000.0;

/// Terminal colors the diagram theme is derived from.
pub struct Palette {
    pub bg: Rgb,
    pub fg: Rgb,
    /// Panel behind the diagram (same as code blocks).
    pub panel: Rgb,
    pub accent: Rgb,
}

pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
    /// Fully opaque, so premultiplied and straight alpha coincide.
    pub rgba: Vec<u8>,
}

/// Renders `source`. At `zoom` 1 it is as large as fits `view` (cols, rows)
/// with text at most `max_upscale` × the terminal's; other zooms scale from
/// there and may overflow the view by up to [`kitty::MAX_COL`] columns, for
/// the pager to pan.
pub fn render(
    source: &str,
    palette: &Palette,
    cell: (u32, u32),
    view: (u32, u32),
    zoom: f32,
    max_upscale: f32,
) -> Result<Raster, String> {
    let svg = mermaid_rs_renderer::render_with_options(source, options(palette))
        .map_err(|e| e.to_string())?;
    let svg = hide_label_borders(&svg);
    let tree = usvg::Tree::from_str(&svg, svg_options()).map_err(|e| e.to_string())?;

    let (cw, ch) = (cell.0.max(1) as f32, cell.1.max(1) as f32);
    let size = tree.size();
    let (sw, sh) = (size.width(), size.height());
    let natural = ch / CELL_TO_FONT / FONT_SIZE;
    let fit = (view.0 as f32 * cw / sw)
        .min(view.1 as f32 * ch / sh)
        .min(natural * max_upscale);
    let max_cols = view.0.max(1) + kitty::MAX_COL;
    let scale = (fit * zoom)
        .min(max_cols as f32 * cw / sw)
        .min(kitty::MAX_ROWS as f32 * ch / sh)
        .min((MAX_PIXELS / (sw * sh)).sqrt());
    let (w, h) = (sw * scale, sh * scale);
    let cols = ((w / cw).ceil() as u32).clamp(1, max_cols);
    let rows = ((h / ch).ceil() as u32).clamp(1, kitty::MAX_ROWS);
    let (width, height) = (cols * cell.0, rows * cell.1);

    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("empty diagram")?;
    let (r, g, b) = palette.panel;
    pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
    // Centred inside the cell-aligned box, so the terminal never rescales it.
    let dx = (width as f32 - w) / 2.0;
    let dy = (height as f32 - h) / 2.0;
    let transform = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    Ok(Raster {
        width,
        height,
        cols,
        rows,
        rgba: pixmap.take(),
    })
}

/// Loading system fonts is the slow part; do it once per process.
fn svg_options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut opt = usvg::Options::default();
        opt.fontdb_mut().load_system_fonts();
        opt
    })
}

fn options(p: &Palette) -> RenderOptions {
    let hex = |c: Rgb| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
    let muted = hex(mix(p.fg, p.panel, 0.45));
    let tint = hex(mix(p.bg, p.accent, 0.18));
    let (bg, fg, panel, accent) = (hex(p.bg), hex(p.fg), hex(p.panel), hex(p.accent));

    let mut t = if luma(p.bg) < 128.0 {
        Theme::dark()
    } else {
        Theme::modern()
    };
    t.font_family = FONT_FAMILY.to_owned();
    t.font_size = FONT_SIZE;
    t.background.clone_from(&panel);
    t.primary_color.clone_from(&bg);
    t.primary_text_color.clone_from(&fg);
    t.text_color.clone_from(&fg);
    t.primary_border_color.clone_from(&accent);
    t.line_color.clone_from(&muted);
    t.secondary_color.clone_from(&tint);
    t.tertiary_color.clone_from(&bg);
    t.edge_label_background.clone_from(&panel);
    t.cluster_background = hex(mix(p.panel, p.accent, 0.08));
    t.cluster_border.clone_from(&muted);
    t.sequence_actor_fill.clone_from(&bg);
    t.sequence_actor_border.clone_from(&accent);
    t.sequence_actor_line.clone_from(&muted);
    t.sequence_note_fill.clone_from(&tint);
    t.sequence_note_border.clone_from(&muted);
    t.sequence_activation_fill.clone_from(&tint);
    t.sequence_activation_border.clone_from(&accent);
    t.pie_title_text_color.clone_from(&fg);
    t.pie_legend_text_color.clone_from(&fg);
    t.pie_stroke_color = panel;

    let mut options = RenderOptions::modern();
    options.theme = t;
    options
}

/// Edge labels get a backdrop only when they touch their edge, outlined in
/// the node border color, so one label is framed and the next isn't. The
/// outline can't be themed separately, so it is dropped here: backdrops
/// (`<rect … data-label-kind=…>`) keep their fill and lose the stroke.
fn hide_label_borders(svg: &str) -> String {
    const OPACITY: &str = "stroke-opacity=\"";
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find("<rect ") {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find('>').map_or(rest.len(), |i| i + 1);
        let (tag, tail) = rest.split_at(end);
        rest = tail;
        let value = tag
            .find(OPACITY)
            .map(|i| i + OPACITY.len())
            .and_then(|v| Some(v..v + tag[v..].find('"')?));
        match value {
            Some(value) if tag.contains("data-label-kind=") => {
                out.push_str(&tag[..value.start]);
                out.push('0');
                out.push_str(&tag[value.end..]);
            }
            _ => out.push_str(tag),
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PALETTE: Palette = Palette {
        bg: (0x28, 0x2c, 0x34),
        fg: (0xdc, 0xdf, 0xe4),
        panel: (0x35, 0x39, 0x40),
        accent: (0x61, 0xaf, 0xef),
    };

    #[test]
    fn render_should_fill_exact_cell_box() {
        let r = render("graph LR\n A --> B", &PALETTE, (10, 20), (80, 20), 1.0, 2.0);
        let dims = r.map(|r| (r.width, r.height, r.cols * 10, r.rows * 20));
        let ok = dims.is_ok_and(|(w, h, cw, ch)| w == cw && h == ch);
        assert!(ok);
    }

    #[test]
    fn render_should_shrink_to_max_cells() {
        let src = "graph LR\n A --> B --> C --> D --> E --> F --> G --> H";
        let r = render(src, &PALETTE, (10, 20), (30, 20), 1.0, 2.0);
        assert!(r.is_ok_and(|r| r.cols <= 30));
    }

    #[test]
    fn render_should_fill_view_width_at_zoom_one() {
        let src = "graph LR\n A --> B --> C --> D --> E --> F";
        let r = render(src, &PALETTE, (10, 20), (60, 40), 1.0, 2.0);
        assert!(r.is_ok_and(|r| r.cols == 60));
    }

    #[test]
    fn render_should_overflow_view_when_zoomed_in() {
        let src = "graph LR\n A --> B --> C --> D --> E --> F";
        let r = render(src, &PALETTE, (10, 20), (60, 40), 2.0, 2.0);
        assert!(r.is_ok_and(|r| r.cols > 60));
    }

    #[test]
    fn render_should_report_parse_errors() {
        assert!(
            render(
                "this is not mermaid",
                &PALETTE,
                (10, 20),
                (80, 20),
                1.0,
                2.0
            )
            .is_err()
        );
    }

    #[test]
    fn hide_label_borders_should_only_touch_label_backdrops() {
        let svg = concat!(
            r#"<rect x="0" stroke-opacity="1"/>"#,
            r#"<rect data-label-kind="center" fill-opacity="0.90" stroke-opacity="0.30"/>"#,
        );
        assert_eq!(
            hide_label_borders(svg),
            concat!(
                r#"<rect x="0" stroke-opacity="1"/>"#,
                r#"<rect data-label-kind="center" fill-opacity="0.90" stroke-opacity="0"/>"#,
            )
        );
    }
}
