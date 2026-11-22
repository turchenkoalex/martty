//! SVG through usvg/resvg, for SVG images and Mermaid diagrams.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use resvg::{tiny_skia, usvg};

use super::{Frame, Frames};
use crate::heading;

static FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");

/// System fonts plus the bundled JetBrains Mono. Loading system fonts is the
/// slow part, so it happens once per process and only when an SVG needs it.
pub fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            db.load_font_data(FONT_REGULAR.to_vec());
            db.load_font_data(heading::FONT_BYTES.to_vec());
            Arc::new(db)
        })
        .clone()
}

/// Parses an SVG file; relative `<image href>`s resolve against `dir`.
pub(super) fn parse(data: &[u8], dir: Option<&Path>) -> Option<usvg::Tree> {
    let options = usvg::Options {
        resources_dir: dir.map(Path::to_path_buf),
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    usvg::Tree::from_data(data, &options).ok()
}

/// Rasterizes `tree` to fit inside `target` pixels, keeping its aspect
/// ratio, on a transparent background.
pub(super) fn rasterize(tree: &usvg::Tree, target: (u32, u32)) -> Option<Frames> {
    let size = tree.size();
    let scale = (target.0 as f32 / size.width()).min(target.1 as f32 / size.height());
    let w = ((size.width() * scale).round() as u32).max(1);
    let h = ((size.height() * scale).round() as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut rgba = pixmap.take();
    unpremultiply(&mut rgba);
    Some(Frames {
        size: (w, h),
        frames: vec![Frame { rgba, delay_ms: 0 }],
        plays: Some(1),
    })
}

/// tiny-skia keeps premultiplied alpha; kitty's `f=32` expects straight.
pub fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = u8::try_from(u16::from(*c) * 255 / a).unwrap_or(u8::MAX);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterize_should_fit_target_keeping_aspect() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="20"><rect width="100" height="20" fill="red"/></svg>"#;
        let tree = parse(svg, None).unwrap_or_else(|| panic!("parses"));
        let f = rasterize(&tree, (50, 50)).unwrap_or_else(|| panic!("draws"));
        assert_eq!(f.size, (50, 10));
        assert_eq!(&f.frames[0].rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn unpremultiply_should_restore_color_of_half_covered_pixel() {
        let mut px = [100, 50, 0, 128];
        unpremultiply(&mut px);
        assert_eq!(px, [199, 99, 0, 128]);
    }
}
