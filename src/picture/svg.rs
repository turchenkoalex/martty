//! SVG through usvg/resvg, for SVG images and Mermaid diagrams, and the
//! bundled fonts they and image headings use.

use std::sync::{Arc, OnceLock};

use resvg::{tiny_skia, usvg};

use super::{Frame, Frames, Refs};

static FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");
pub static FONT_BOLD: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf");
const BUNDLED_FAMILY: &str = "JetBrains Mono";

/// System fonts plus the bundled JetBrains Mono. Loading system fonts is the
/// slow part (~60 ms), so it happens once per process and only when needed.
pub fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            load_bundled(&mut db);
            Arc::new(db)
        })
        .clone()
}

/// Only the bundled JetBrains Mono, standing in for every generic family
/// too, so any `font-family` resolves to it.
pub fn bundled_fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            load_bundled(&mut db);
            db.set_serif_family(BUNDLED_FAMILY);
            db.set_sans_serif_family(BUNDLED_FAMILY);
            db.set_monospace_family(BUNDLED_FAMILY);
            db.set_cursive_family(BUNDLED_FAMILY);
            db.set_fantasy_family(BUNDLED_FAMILY);
            Arc::new(db)
        })
        .clone()
}

fn load_bundled(db: &mut usvg::fontdb::Database) {
    db.load_font_data(FONT_REGULAR.to_vec());
    db.load_font_data(FONT_BOLD.to_vec());
}

/// Whether the bundled JetBrains Mono has a glyph for every char of `text`.
pub fn bundled_covers(text: &str) -> bool {
    use fontdue::{Font, FontSettings};
    static FONT: OnceLock<Option<Font>> = OnceLock::new();
    let Some(font) = FONT
        .get_or_init(|| Font::from_bytes(FONT_REGULAR, FontSettings::default()).ok())
        .as_ref()
    else {
        return false;
    };
    text.chars()
        .all(|c| c.is_control() || font.lookup_glyph_index(c) != 0)
}

/// An SVG image. Its size doesn't depend on fonts, so it is read with the
/// bundled ones; one with text is read again with the system's (loaded in
/// the background meanwhile) only when it is drawn.
pub(super) struct Image {
    tree: usvg::Tree,
    text: bool,
    refs: Refs,
}

impl Image {
    pub(super) fn new(data: &[u8], refs: Refs) -> Option<Self> {
        let text = has_text(data);
        if text {
            std::thread::spawn(fonts);
        }
        let tree = parse(data, &refs, bundled_fonts())?;
        Some(Self { tree, text, refs })
    }

    /// Natural size in CSS pixels.
    pub(super) fn size(&self) -> (u32, u32) {
        let size = self.tree.size();
        (size.width().ceil() as u32, size.height().ceil() as u32)
    }

    /// `data` is the SVG this was made from.
    pub(super) fn rasterize(&self, data: &[u8], target: (u32, u32)) -> Option<Frames> {
        if self.text {
            return rasterize(&parse(data, &self.refs, fonts())?, target);
        }
        rasterize(&self.tree, target)
    }
}

fn parse(data: &[u8], refs: &Refs, fontdb: Arc<usvg::fontdb::Database>) -> Option<usvg::Tree> {
    let mut options = usvg::Options {
        fontdb,
        ..usvg::Options::default()
    };
    match refs {
        Refs::Local(dir) => options.resources_dir.clone_from(dir),
        Refs::DataOnly => options.image_href_resolver.resolve_string = Box::new(|_, _| None),
    }
    usvg::Tree::from_data(data, &options).ok()
}

/// Whether the SVG may draw text, which needs the fonts it names. A
/// compressed SVG (svgz) can't be told, so it may.
fn has_text(data: &[u8]) -> bool {
    data.starts_with(b"\x1f\x8b") || data.windows(5).any(|w| w == b"<text")
}

/// Rasterizes `tree` to fit inside `target` pixels, keeping its aspect
/// ratio, on a transparent background.
fn rasterize(tree: &usvg::Tree, target: (u32, u32)) -> Option<Frames> {
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
        let tree =
            parse(svg, &Refs::Local(None), bundled_fonts()).unwrap_or_else(|| panic!("parses"));
        let f = rasterize(&tree, (50, 50)).unwrap_or_else(|| panic!("draws"));
        assert_eq!(f.size, (50, 10));
        assert_eq!(&f.frames[0].rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn parse_should_not_load_local_files_for_remote_svg() {
        let dir = std::env::temp_dir().join(format!("martty-svg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let png = dir.join("local.png");
        let mut data = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        data.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        std::fs::write(&png, &data).ok();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="{}" width="10" height="10"/></svg>"#,
            png.display()
        );
        let images = |refs| {
            let tree =
                parse(svg.as_bytes(), &refs, bundled_fonts()).unwrap_or_else(|| panic!("parses"));
            tree.root().children().len()
        };
        let local = images(Refs::Local(None));
        let remote = images(Refs::DataOnly);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!((local, remote), (1, 0));
    }

    #[test]
    fn has_text_should_find_text_elements() {
        assert!(has_text(b"<svg><text>a</text></svg>"));
        assert!(!has_text(b"<svg><rect/></svg>"));
    }

    #[test]
    fn unpremultiply_should_restore_color_of_half_covered_pixel() {
        let mut px = [100, 50, 0, 128];
        unpremultiply(&mut px);
        assert_eq!(px, [199, 99, 0, 128]);
    }
}
