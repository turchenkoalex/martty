//! Mermaid diagrams: merman lays them out as SVG the way Mermaid does,
//! resvg rasterizes that at the terminal's own text size into a box of whole
//! cells, and kitty graphics shows it. No browser involved.

use std::sync::OnceLock;

use resvg::{tiny_skia, usvg};

use crate::kitty;
use crate::picture::svg;
use crate::term::Rgb;

const FONT_SIZE: f32 = 14.0;
/// Terminal cells are roughly this much taller than the font size.
const CELL_TO_FONT: f32 = 1.25;
/// Bound on the raster (RGBA), whatever the zoom.
const MAX_PIXELS: f32 = 16_000_000.0;

/// Terminal colors the diagram theme is derived from.
// Only merman reads the colors; without it, diagrams are never drawn.
#[cfg_attr(not(feature = "mermaid"), expect(dead_code))]
pub struct Palette {
    pub bg: Rgb,
    pub fg: Rgb,
    /// Panel behind the diagram (same as code blocks).
    pub panel: Rgb,
    pub accent: Rgb,
    /// ANSI colors 1–6 (red, green, yellow, blue, magenta, cyan), for pie slices.
    pub ansi: [Rgb; 6],
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
    let svg = to_svg(source, palette)?;
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

/// Bundled JetBrains Mono, with system fonts as a fallback for glyphs it
/// lacks.
fn svg_options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| usvg::Options {
        fontdb: svg::fonts(),
        ..usvg::Options::default()
    })
}

#[cfg(not(feature = "mermaid"))]
fn to_svg(_source: &str, _palette: &Palette) -> Result<String, String> {
    Err("built without the mermaid feature".to_owned())
}

#[cfg(feature = "mermaid")]
use merman_svg::to_svg;

#[cfg(feature = "mermaid")]
mod merman_svg {
    use merman::svg::{
        DeterministicTextMeasurer, MeasurementProfileId, TextMeasurementPolicy,
        TextMeasurementProfile, TextMeasurementProfileIdentity, TextStyle,
    };
    use merman::{
        MermaidEngine, OperationControl, RenderOutput, RenderRequest, Renderer,
        SourcePresentationConfig, SvgEnvironment, SvgRequest,
    };
    use serde_json::{Map, Value, json};
    use unicode_width::UnicodeWidthStr;

    use super::{FONT_SIZE, Palette};
    use crate::term::{Rgb, luma, mix};

    /// Bundled, so the font resvg draws with is the one text was measured in.
    const FONT_FAMILY: &str = "\"JetBrains Mono\", monospace";
    /// JetBrains Mono advances every glyph by 600/1000 em.
    const ADVANCE_EM: f64 = 0.6;

    pub(super) fn to_svg(source: &str, palette: &Palette) -> Result<String, String> {
        let config = SourcePresentationConfig::from_value(site_config(palette));
        let renderer =
            Renderer::new().with_presentation_engine(MermaidEngine::new().with_site_config(config));
        let request = SvgRequest {
            environment: SvgEnvironment::deterministic()
                .with_text_measurement_policy(monospace_measurement()?),
            ..SvgRequest::default()
        };
        match renderer
            .render(RenderRequest::svg(source, OperationControl::new(), request))
            .map_err(|e| e.to_string())?
        {
            RenderOutput::Svg(Some(svg)) => Ok(svg.svg().to_owned()),
            _ => Err("not a Mermaid diagram".to_owned()),
        }
    }

    /// merman sizes boxes from its own font metrics by default; in a
    /// monospace font a label is exactly its cell count wide.
    fn monospace_measurement() -> Result<TextMeasurementPolicy, String> {
        let measurer = DeterministicTextMeasurer::default().with_width_callback(
            |text: &str, style: &TextStyle| {
                text.width() as f64 * ADVANCE_EM * style.font_size.max(1.0)
            },
        );
        let id = MeasurementProfileId::new("martty.jetbrains-mono").map_err(|e| e.to_string())?;
        let identity = TextMeasurementProfileIdentity::new(id, "advance-600".to_owned())
            .map_err(|e| e.to_string())?;
        Ok(TextMeasurementPolicy::uniform(TextMeasurementProfile::new(
            identity, measurer,
        )))
    }

    /// Mermaid's `base` theme with every color taken from the terminal
    /// palette. Labels are plain SVG text: resvg can't draw the HTML that
    /// Mermaid puts in `<foreignObject>` by default.
    fn site_config(p: &Palette) -> Value {
        let hex = |c: Rgb| format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2);
        let muted = hex(mix(p.fg, p.panel, 0.45));
        let tint = hex(mix(p.bg, p.accent, 0.18));
        let cluster = hex(mix(p.panel, p.accent, 0.08));
        let (bg, fg, panel, accent) = (hex(p.bg), hex(p.fg), hex(p.panel), hex(p.accent));
        let colors = [
            ("background", &panel),
            ("primaryColor", &bg),
            ("primaryTextColor", &fg),
            ("primaryBorderColor", &accent),
            ("secondaryColor", &tint),
            ("secondaryTextColor", &fg),
            ("secondaryBorderColor", &muted),
            ("tertiaryColor", &bg),
            ("tertiaryTextColor", &fg),
            ("tertiaryBorderColor", &muted),
            ("mainBkg", &bg),
            ("nodeBorder", &accent),
            ("nodeTextColor", &fg),
            ("textColor", &fg),
            ("titleColor", &fg),
            ("lineColor", &muted),
            ("defaultLinkColor", &muted),
            ("edgeLabelBackground", &panel),
            ("clusterBkg", &cluster),
            ("clusterBorder", &muted),
            ("noteBkgColor", &tint),
            ("noteTextColor", &fg),
            ("noteBorderColor", &muted),
            ("actorBkg", &bg),
            ("actorBorder", &accent),
            ("actorTextColor", &fg),
            ("actorLineColor", &muted),
            ("signalColor", &fg),
            ("signalTextColor", &fg),
            ("labelBoxBkgColor", &bg),
            ("labelBoxBorderColor", &accent),
            ("labelTextColor", &fg),
            ("loopTextColor", &fg),
            ("activationBkgColor", &tint),
            ("activationBorderColor", &accent),
            ("pieTitleTextColor", &fg),
            ("pieSectionTextColor", &bg),
            ("pieLegendTextColor", &fg),
            ("pieStrokeColor", &panel),
            ("pieOuterStrokeColor", &muted),
            ("attributeBackgroundColorOdd", &bg),
            ("attributeBackgroundColorEven", &panel),
        ];
        let mut vars: Map<String, Value> = colors
            .into_iter()
            .map(|(key, color)| (key.to_owned(), Value::from(color.as_str())))
            .collect();
        vars.insert("darkMode".to_owned(), Value::from(luma(p.bg) < 128.0));
        vars.insert("fontFamily".to_owned(), Value::from(FONT_FAMILY));
        vars.insert("fontSize".to_owned(), Value::from(format!("{FONT_SIZE}px")));
        for (i, &color) in p.ansi.iter().enumerate() {
            vars.insert(format!("pie{}", i + 1), Value::from(hex(color)));
            // Class, state and ER nodes take their fill from the cScale ramp.
            vars.insert(format!("cScale{i}"), Value::from(bg.as_str()));
            vars.insert(format!("cScalePeer{i}"), Value::from(accent.as_str()));
        }
        json!({
            "theme": "base",
            "htmlLabels": false,
            "flowchart": { "htmlLabels": false },
            "themeVariables": vars,
        })
    }
}

#[cfg(all(test, feature = "mermaid"))]
mod tests {
    use super::*;

    const PALETTE: Palette = Palette {
        bg: (0x28, 0x2c, 0x34),
        fg: (0xdc, 0xdf, 0xe4),
        panel: (0x35, 0x39, 0x40),
        accent: (0x61, 0xaf, 0xef),
        ansi: [
            (0xe0, 0x6c, 0x75),
            (0x98, 0xc3, 0x79),
            (0xe5, 0xc0, 0x7b),
            (0x61, 0xaf, 0xef),
            (0xc6, 0x78, 0xdd),
            (0x56, 0xb6, 0xc2),
        ],
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
}
