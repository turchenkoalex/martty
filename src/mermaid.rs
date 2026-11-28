//! Mermaid diagrams: merman lays them out as SVG the way Mermaid does,
//! resvg rasterizes that at the terminal's own text size into a box of whole
//! cells, and kitty graphics shows it. No browser involved.
//!
//! Laying a diagram out (merman, then usvg) tells its size in cells;
//! rasterizing waits until the pager needs the pixels. Laid out trees are
//! kept for the session, so a zoom or a resize only rasterizes again.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use resvg::{tiny_skia, usvg};

use crate::cache::{self, Lru};
use crate::kitty::{self, Transmit};
use crate::panics;
use crate::picture::svg;
use crate::progress::{self, Busy};
use crate::term::Rgb;
use merman_svg::to_svg;

/// Terminal colors the diagram theme is derived from.
#[derive(Debug, Clone, Copy, Hash)]
pub struct Palette {
    pub bg: Rgb,
    pub fg: Rgb,
    /// Panel behind the diagram (same as code blocks).
    pub panel: Rgb,
    pub accent: Rgb,
    /// ANSI colors 1–6 (red, green, yellow, blue, magenta, cyan), for pie slices.
    pub ansi: [Rgb; 6],
}

const FONT_SIZE: f32 = 14.0;
/// Terminal cells are roughly this much taller than the font size.
const CELL_TO_FONT: f32 = 1.25;
/// Bound on the raster (RGBA), whatever the zoom.
const MAX_PIXELS: f32 = 16_000_000.0;
/// Laid out diagrams kept for the session.
const TREES: usize = 64;

type Tree = Result<Arc<usvg::Tree>, String>;

/// A diagram sized for the screen, not drawn yet.
pub struct Diagram {
    pub cols: u32,
    pub rows: u32,
    tree: Arc<usvg::Tree>,
    cell: (u32, u32),
    /// Pixels per SVG unit.
    scale: f32,
    panel: Rgb,
}

/// Lays out `source`. At `zoom` 1 it is as large as fits `view` (cols,
/// rows) with text at most `max_upscale` × the terminal's; other zooms
/// scale from there and may overflow the view by up to
/// [`kitty::MAX_COL`] columns, for the pager to pan.
pub fn layout(
    source: &str,
    palette: &Palette,
    cell: (u32, u32),
    view: (u32, u32),
    zoom: f32,
    max_upscale: f32,
) -> Result<Diagram, String> {
    let tree = tree(source, palette)?;
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
    let cols = (((sw * scale) / cw).ceil() as u32).clamp(1, max_cols);
    let rows = (((sh * scale) / ch).ceil() as u32).clamp(1, kitty::MAX_ROWS);
    Ok(Diagram {
        cols,
        rows,
        tree,
        cell,
        scale,
        panel: palette.panel,
    })
}

impl Diagram {
    /// The diagram on its panel, filling exactly its cells: fully
    /// opaque RGBA, so premultiplied and straight alpha coincide.
    pub fn rasterize(&self) -> Option<(Vec<u8>, (u32, u32))> {
        let (width, height) = (self.cols * self.cell.0, self.rows * self.cell.1);
        let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
        let (r, g, b) = self.panel;
        pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        let size = self.tree.size();
        let (w, h) = (size.width() * self.scale, size.height() * self.scale);
        // Centred inside the cell-aligned box, so the terminal never
        // rescales it.
        let dx = (width as f32 - w) / 2.0;
        let dy = (height as f32 - h) / 2.0;
        let transform = tiny_skia::Transform::from_row(self.scale, 0.0, 0.0, self.scale, dx, dy);
        resvg::render(&self.tree, transform, &mut pixmap.as_mut());
        Some((pixmap.take(), (width, height)))
    }

    pub fn transmit(&self) -> Option<Transmit> {
        let (rgba, size) = self.rasterize()?;
        Some(Transmit::rgba(&rgba, size, self.cols, self.rows))
    }
}

/// Lays out every diagram not laid out yet, in parallel, so the
/// document's layout then finds them ready.
pub fn prefetch(sources: &[&str], palette: &Palette) {
    let threads = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(sources.len());
    if threads <= 1 {
        return;
    }
    let busy = Busy::start(&format!("Drawing {} diagrams", sources.len()));
    let total = sources.len() as u64;
    let done = AtomicU64::new(0);
    let per_thread = sources.len().div_ceil(threads);
    thread::scope(|s| {
        for chunk in sources.chunks(per_thread) {
            let (busy, done) = (&busy, &done);
            s.spawn(move || {
                for source in chunk {
                    let _ = tree(source, palette);
                    busy.progress(progress::percent(
                        done.fetch_add(1, Ordering::Relaxed) + 1,
                        total,
                    ));
                }
            });
        }
    });
}

/// merman's SVG for `source` in `palette`, parsed by usvg, from the
/// session's cache when it was laid out before.
fn tree(source: &str, palette: &Palette) -> Tree {
    static TREES_CACHE: OnceLock<Mutex<Lru<Tree>>> = OnceLock::new();
    let cache = TREES_CACHE.get_or_init(|| Mutex::new(Lru::new(TREES)));
    let key = cache::key((source, palette));
    if let Some(tree) = cache.lock().ok().and_then(|mut c| c.get(key)) {
        return tree;
    }
    let tree = panics::catch(|| {
        Some(to_svg(source, palette).and_then(|svg| {
            let options = usvg::Options {
                fontdb: fonts_for(source),
                ..usvg::Options::default()
            };
            usvg::Tree::from_str(&svg, &options)
                .map(Arc::new)
                .map_err(|e| e.to_string())
        }))
    })
    .unwrap_or_else(|| Err("the diagram renderer crashed on this diagram".to_owned()));
    if let Ok(mut c) = cache.lock() {
        c.insert(key, tree.clone(), 1);
    }
    tree
}

/// Text is measured in the bundled JetBrains Mono. Loading the system
/// fonts takes ~60 ms, so they are only loaded for glyphs it lacks.
fn fonts_for(source: &str) -> Arc<usvg::fontdb::Database> {
    if svg::bundled_covers(source) {
        svg::bundled_fonts()
    } else {
        svg::fonts()
    }
}

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

    use super::FONT_SIZE;
    use super::Palette;
    use crate::term::{Rgb, luma, mix};

    /// Bundled, so the font resvg draws with is the one text was measured in.
    const FONT_FAMILY: &str = "\"JetBrains Mono\", monospace";
    /// Where flowchart labels wrap, in SVG pixels at `FONT_SIZE`.
    const WRAPPING_WIDTH: f64 = 280.0;
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
            // Mermaid wraps labels at 200px, set explicitly: left unset,
            // merman wraps SVG labels at ~120px. Monospace text is wider than
            // Mermaid's proportional font, so 280px keeps about as many
            // words on a line and long identifiers whole.
            "flowchart": { "htmlLabels": false, "wrappingWidth": WRAPPING_WIDTH },
            "themeVariables": vars,
        })
    }
}

#[cfg(test)]
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
    fn rasterize_should_fill_exact_cell_box() {
        let d = layout("graph LR\n A --> B", &PALETTE, (10, 20), (80, 20), 1.0, 2.0);
        let ok = d.ok().is_some_and(|d| {
            d.rasterize().is_some_and(|(rgba, size)| {
                size == (d.cols * 10, d.rows * 20) && rgba.len() == (size.0 * size.1 * 4) as usize
            })
        });
        assert!(ok);
    }

    #[test]
    fn layout_should_shrink_to_max_cells() {
        let src = "graph LR\n A --> B --> C --> D --> E --> F --> G --> H";
        let d = layout(src, &PALETTE, (10, 20), (30, 20), 1.0, 2.0);
        assert!(d.is_ok_and(|d| d.cols <= 30));
    }

    #[test]
    fn layout_should_fill_view_width_at_zoom_one() {
        let src = "graph LR\n A --> B --> C --> D --> E --> F";
        let d = layout(src, &PALETTE, (10, 20), (60, 40), 1.0, 2.0);
        assert!(d.is_ok_and(|d| d.cols == 60));
    }

    #[test]
    fn layout_should_overflow_view_when_zoomed_in() {
        let src = "graph LR\n A --> B --> C --> D --> E --> F";
        let d = layout(src, &PALETTE, (10, 20), (60, 40), 2.0, 2.0);
        assert!(d.is_ok_and(|d| d.cols > 60));
    }

    #[test]
    fn layout_should_report_parse_errors() {
        let d = layout(
            "this is not mermaid",
            &PALETTE,
            (10, 20),
            (80, 20),
            1.0,
            2.0,
        );
        assert!(d.is_err());
    }

    #[test]
    fn layout_should_draw_every_diagram_type() {
        let sources = [
            "sankey-beta\nA,B,10\nA,C,5",
            "mindmap\n  root\n    a\n    b",
            "architecture-beta\n  service db(database)[DB]\n  service api(server)[API]\n  db:L -- R:api",
            "block-beta\n  a b c",
            "packet-beta\n  0-15: \"Source\"\n  16-31: \"Dest\"",
            "kanban\n  todo\n    task1",
            "radar-beta\n  axis a, b, c\n  curve x{1, 2, 3}",
            "treemap-beta\n\"A\"\n  \"B\": 10\n  \"C\": 20",
            "C4Context\n  Person(u, \"User\")",
            "---\nconfig:\n  layout: elk\n---\nflowchart LR\n  A --> B",
        ];
        for s in sources {
            let d = layout(s, &PALETTE, (10, 20), (80, 20), 1.0, 1.0);
            assert!(d.is_ok(), "{s}: {:?}", d.err());
        }
    }

    #[test]
    fn prefetch_should_leave_trees_for_layout() {
        let sources = [
            "graph LR\n P1 --> Q",
            "graph LR\n P2 --> Q",
            "graph TD\n P3 --> Q",
        ];
        prefetch(&sources, &PALETTE);
        assert!(
            sources
                .iter()
                .all(|s| layout(s, &PALETTE, (10, 20), (80, 20), 1.0, 1.0).is_ok())
        );
    }
}
