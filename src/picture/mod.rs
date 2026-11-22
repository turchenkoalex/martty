//! Images in the formats martty shows. PNG goes to Ghostty as it is, since
//! Ghostty decodes it itself. JPEG, GIF and WebP are decoded here, SVG is
//! rasterized with resvg, and all of them are sent as RGBA scaled down to
//! the cell box they take. Animated GIF and WebP become kitty animations.

mod decode;
mod scale;
pub mod svg;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::kitty;

/// Ghostty decodes PNG itself; anything bigger is not worth decoding.
const MAX_BYTES: usize = 64 << 20;
/// Largest bitmap decoded, in pixels (256 MB as RGBA). The size comes from
/// the file's header, so a few bytes could otherwise ask for gigabytes.
const MAX_PIXELS: u64 = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
    Svg,
}

/// An image file whose format and size are known, not yet decoded.
pub struct Picture {
    data: Arc<Vec<u8>>,
    format: Format,
    /// Natural size: pixels for bitmaps, CSS pixels for SVG.
    size: (u32, u32),
    svg: Option<svg::Image>,
}

/// What an SVG's `<image href>` may load besides `data:` URLs.
enum Refs {
    /// Local files; relative paths resolve against the directory.
    Local(Option<PathBuf>),
    /// Nothing: the SVG came from the network and must not read local files.
    DataOnly,
}

/// Straight (non-premultiplied) RGBA frames of one size.
pub struct Frames {
    pub size: (u32, u32),
    pub frames: Vec<Frame>,
    /// How many times an animation plays; `None` is forever.
    pub plays: Option<u32>,
}

pub struct Frame {
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

impl Picture {
    /// Recognizes the format by content (SVG also by extension) and reads
    /// the size. `path` is where the file came from, for SVG references.
    pub fn new(data: Vec<u8>, path: &Path) -> Option<Self> {
        Self::with_refs(
            Arc::new(data),
            path,
            Refs::Local(path.parent().map(Path::to_path_buf)),
        )
    }

    /// An image downloaded from `name` (the URL's path): an SVG may embed
    /// only `data:` images, never local files. The bytes are shared with
    /// the session's download cache.
    pub fn new_remote(data: Arc<Vec<u8>>, name: &Path) -> Option<Self> {
        Self::with_refs(data, name, Refs::DataOnly)
    }

    /// Decoders and usvg parse untrusted bytes; one that panics means the
    /// image can't be shown.
    fn with_refs(data: Arc<Vec<u8>>, path: &Path, refs: Refs) -> Option<Self> {
        if data.len() > MAX_BYTES {
            return None;
        }
        crate::panics::catch(|| Self::recognize(data, path, refs))
    }

    fn recognize(data: Arc<Vec<u8>>, path: &Path, refs: Refs) -> Option<Self> {
        let format = sniff(&data, path)?;
        let svg = match format {
            Format::Svg => Some(svg::Image::new(&data, refs)?),
            _ => None,
        };
        let size = match format {
            Format::Png => kitty::png_size(&data)?,
            Format::Svg => svg.as_ref()?.size(),
            _ => decode::size(format, &data)?,
        };
        let pixels = u64::from(size.0) * u64::from(size.1);
        // SVG is rasterized at the size of its cell box, not its own.
        let fits = format == Format::Svg || pixels <= MAX_PIXELS;
        (size.0 > 0 && size.1 > 0 && fits).then_some(Self {
            data,
            format,
            size,
            svg,
        })
    }

    /// Natural size in device pixels. SVG is sized in CSS pixels, taken as
    /// `css_px` device pixels each.
    pub fn pixels(&self, css_px: f32) -> (u32, u32) {
        if self.format != Format::Svg {
            return self.size;
        }
        let px = |v: u32| ((v as f32 * css_px).round() as u32).max(1);
        (px(self.size.0), px(self.size.1))
    }

    /// Encodes the picture for a box of `px` device pixels and returns
    /// what to transmit. `None` when it can't be decoded.
    pub fn encode(&self, px: (u32, u32)) -> Option<Encoded<'_>> {
        match self.format {
            Format::Png => Some(Encoded::Png(&self.data)),
            Format::Svg => {
                let image = self.svg.as_ref()?;
                Some(Encoded::Frames(image.rasterize(&self.data, px)?))
            }
            format => {
                let frames = decode::frames(format, &self.data, px)?;
                Some(Encoded::Frames(frames))
            }
        }
    }
}

pub enum Encoded<'a> {
    Png(&'a [u8]),
    Frames(Frames),
}

impl Encoded<'_> {
    /// The upload of the picture with a virtual placement of `cols` × `rows`
    /// cells; more than one frame also starts the animation.
    pub fn transmit(&self, cols: u32, rows: u32) -> Option<kitty::Transmit> {
        let f = match self {
            Encoded::Png(data) => return Some(kitty::Transmit::png(data, cols, rows)),
            Encoded::Frames(f) => f,
        };
        let (first, rest) = f.frames.split_first()?;
        let mut t = kitty::Transmit::rgba(&first.rgba, f.size, cols, rows);
        if !rest.is_empty() {
            for frame in rest {
                t.frame(&frame.rgba, f.size, frame.delay_ms);
            }
            t.start_animation(first.delay_ms, f.plays);
        }
        Some(t)
    }
}

/// A local image file, read only when it is a regular file of at most
/// `MAX_BYTES`: never a device (`/dev/zero`) or a FIFO, which would hang.
pub fn read_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let limit = u64::try_from(MAX_BYTES).ok()?;
    if !meta.is_file() || meta.len() > limit {
        return None;
    }
    let mut data = Vec::new();
    file.take(limit + 1).read_to_end(&mut data).ok()?;
    (data.len() <= MAX_BYTES).then_some(data)
}

fn sniff(data: &[u8], path: &Path) -> Option<Format> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Format::Png);
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return Some(Format::Jpeg);
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        return Some(Format::Webp);
    }
    let text = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let start = text
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(0);
    let svg_ext = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    (svg_ext || text[start..].starts_with(b"<svg") || text[start..].starts_with(b"<?xml"))
        .then_some(Format::Svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_should_recognize_formats_by_content() {
        let none = Path::new("x");
        assert_eq!(sniff(b"\xff\xd8\xff\xe0", none), Some(Format::Jpeg));
        assert_eq!(sniff(b"GIF89a....", none), Some(Format::Gif));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ", none), Some(Format::Webp));
        assert_eq!(sniff(b"  <svg xmlns=''/>", none), Some(Format::Svg));
        assert_eq!(sniff(b"hello", none), None);
    }

    #[test]
    fn new_should_refuse_bitmaps_too_large_to_decode() {
        let header = b"GIF89a\xff\xff\xff\xff\0\0\0".to_vec();
        assert!(Picture::new(header, Path::new("huge.gif")).is_none());
    }

    #[test]
    fn read_file_should_refuse_devices_and_directories() {
        assert!(read_file(Path::new("/dev/zero")).is_none());
        assert!(read_file(Path::new(env!("CARGO_MANIFEST_DIR"))).is_none());
        let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo.gif");
        assert!(read_file(&demo).is_some());
    }

    fn asset(name: &str) -> (Picture, (u32, u32)) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(name);
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let picture = Picture::new(data, &path).unwrap_or_else(|| panic!("{name}: not recognized"));
        let size = picture.pixels(2.0);
        (picture, size)
    }

    fn frames(name: &str, target: (u32, u32)) -> Frames {
        let (picture, _) = asset(name);
        match picture.encode(target) {
            Some(Encoded::Frames(f)) => f,
            _ => panic!("{name}: should decode to frames"),
        }
    }

    #[test]
    fn demo_assets_should_decode_within_target() {
        for name in ["demo.jpg", "demo.gif", "demo.webp", "demo.svg"] {
            let f = frames(name, (40, 40));
            assert!(f.size.0 <= 40 && f.size.1 <= 40, "{name}: {:?}", f.size);
            assert_eq!(f.frames.len(), 1, "{name}");
            let len = (f.size.0 * f.size.1 * 4) as usize;
            assert_eq!(f.frames[0].rgba.len(), len, "{name}");
        }
    }

    #[test]
    fn animated_gif_should_keep_all_frames() {
        let f = frames("demo-animated.gif", (40, 40));
        assert!(f.frames.len() > 1);
        assert!(
            f.frames
                .iter()
                .all(|fr| fr.rgba.len() == f.frames[0].rgba.len())
        );
    }

    #[test]
    fn animation_should_be_sent_as_frames_and_started() {
        let (picture, _) = asset("demo-animated.gif");
        let mut out = String::new();
        if let Some(t) = picture.encode((40, 40)).and_then(|e| e.transmit(4, 2)) {
            t.write(&mut out, 7);
        }
        assert!(out.contains("\x1b_Gi=7,a=T,U=1,f=32"));
        assert!(out.contains("\x1b_Gi=7,a=f,"));
        assert!(out.contains("\x1b_Gi=7,a=a,s=3,"));
    }

    #[test]
    fn png_should_be_sent_as_is() {
        let (picture, size) = asset("demo.png");
        assert!(size.0 > 0);
        assert!(matches!(picture.encode((10, 10)), Some(Encoded::Png(_))));
    }

    #[test]
    fn svg_should_be_sized_in_css_pixels() {
        let (picture, size) = asset("demo.svg");
        assert_eq!(size, (picture.size.0 * 2, picture.size.1 * 2));
    }

    #[test]
    fn sniff_should_take_svg_extension() {
        assert_eq!(
            sniff(b"<!-- c --><svg/>", Path::new("a.SVG")),
            Some(Format::Svg)
        );
    }
}
