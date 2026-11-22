//! Kitty graphics protocol: just enough to show images in a cell box.
//!
//! Images are shown through Unicode placeholders (`U=1`): the image is
//! transmitted once, and its cells are ordinary text (U+10EEEE plus row and
//! column diacritics, image id in the foreground color). So images scroll,
//! clip and get redrawn exactly like text — which the pager relies on.
//!
//! PNG goes over the wire as-is (`f=100`): Ghostty decodes it itself, so we
//! only need the header for the aspect ratio and a base64 encoder.

use std::fmt::Write as _;

use crate::diacritics::DIACRITICS;

/// Placeholders can address this many rows (one diacritic per row).
pub const MAX_ROWS: u32 = DIACRITICS.len() as u32;
/// Furthest column a placeholder row can start at (horizontal panning).
pub const MAX_COL: u32 = DIACRITICS.len() as u32 - 1;
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Max payload per escape, per the protocol spec.
const CHUNK: usize = 4096;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Pixel size from the IHDR chunk, or `None` if this isn't a PNG.
pub fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    if !data.starts_with(PNG_MAGIC) || data.get(12..16) != Some(b"IHDR") {
        return None;
    }
    let be = |r: std::ops::Range<usize>| Some(u32::from_be_bytes(data.get(r)?.try_into().ok()?));
    let (w, h) = (be(16..20)?, be(20..24)?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Cell box for an image: natural size, shrunk to fit `max_cols` × `max_rows`
/// while keeping the aspect ratio, then scaled by `zoom`. Zoomed in, it may
/// overflow the width by up to [`MAX_COL`] columns, for the pager to pan.
pub fn fit(
    px: (u32, u32),
    cell: (u32, u32),
    max_cols: u32,
    max_rows: u32,
    zoom: f32,
) -> (u32, u32) {
    let (pw, ph) = (f64::from(px.0.max(1)), f64::from(px.1.max(1)));
    let (cw, ch) = (f64::from(cell.0.max(1)), f64::from(cell.1.max(1)));
    let fitted = 1.0_f64
        .min(f64::from(max_cols) * cw / pw)
        .min(f64::from(max_rows) * ch / ph);
    let col_cap = max_cols.max(1).saturating_add(MAX_COL);
    let scale = (fitted * f64::from(zoom))
        .min(f64::from(col_cap) * cw / pw)
        .min(f64::from(MAX_ROWS) * ch / ph);
    let cols = (pw * scale / cw).ceil().clamp(1.0, f64::from(col_cap));
    let rows = (ph * scale / ch).ceil().clamp(1.0, f64::from(MAX_ROWS));
    // Values are clamped to u32 bounds above.
    (cols as u32, rows as u32)
}

/// Transmits a PNG as image `id` with a virtual placement of `cols` × `rows`
/// cells. Replies are suppressed (`q=2`) so nothing leaks into the shell.
pub fn write_png(out: &mut String, id: u32, png: &[u8], cols: u32, rows: u32) {
    let keys = format!("a=T,U=1,i={id},f=100,t=d,q=2,c={cols},r={rows}");
    write_chunked(out, &keys, png);
}

/// Like [`write_png`] for raw straight-alpha RGBA, zlib-compressed on the
/// wire (`o=z`): heading bitmaps are mostly transparent and shrink ~20×.
pub fn write_rgba(out: &mut String, id: u32, rgba: &[u8], size: (u32, u32), cols: u32, rows: u32) {
    let (w, h) = size;
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(rgba, 6);
    let keys = format!("a=T,U=1,i={id},f=32,o=z,s={w},v={h},t=d,q=2,c={cols},r={rows}");
    write_chunked(out, &keys, &packed);
}

/// Adds a frame to animation `id` (kitty `a=f`): straight RGBA covering the
/// whole image, shown for `gap_ms`.
pub fn write_frame(out: &mut String, id: u32, rgba: &[u8], size: (u32, u32), gap_ms: u32) {
    let (w, h) = size;
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(rgba, 6);
    let keys = format!("a=f,i={id},f=32,o=z,s={w},v={h},z={gap_ms},q=2");
    write_chunked(out, &keys, &packed);
}

/// Starts animation `id` (kitty `a=a`): sets the first frame's gap and runs
/// it `plays` times, or forever when `None`.
pub fn write_animation_start(out: &mut String, id: u32, first_gap_ms: u32, plays: Option<u32>) {
    // `v` is the loop count plus one; 1 loops forever.
    let loops = plays.map_or(1, |n| n.saturating_add(1));
    let _ = write!(
        out,
        "\x1b_Ga=a,i={id},r=1,z={first_gap_ms},q=2\x1b\\\x1b_Ga=a,i={id},s=3,v={loops},q=2\x1b\\"
    );
}

fn write_chunked(out: &mut String, keys: &str, data: &[u8]) {
    let payload = base64(data);
    let mut chunks = payload.as_bytes().chunks(CHUNK).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        out.push_str("\x1b_G");
        if first {
            let _ = write!(out, "{keys},");
            first = false;
        }
        let _ = write!(out, "m={more};");
        // base64 is pure ASCII.
        out.extend(chunk.iter().map(|&b| char::from(b)));
        out.push_str("\x1b\\");
    }
}

/// Frees image `id` and its data in the terminal.
pub fn write_delete(out: &mut String, id: u32) {
    let _ = write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
}

/// Text for `cols` placeholder cells of image row `row`, starting at image
/// column `col`. Only the first cell carries diacritics; the terminal infers
/// the following columns. The caller must color it with [`id_color`].
pub fn placeholder_row(row: u32, col: u32, cols: u32) -> String {
    let last = DIACRITICS.len() - 1;
    let mut s = String::with_capacity(cols as usize * 4 + 8);
    s.push(PLACEHOLDER);
    s.push(DIACRITICS[(row as usize).min(last)]);
    s.push(DIACRITICS[(col as usize).min(last)]);
    for _ in 1..cols {
        s.push(PLACEHOLDER);
    }
    s
}

/// The 24-bit foreground color that encodes image `id` in placeholders.
pub fn id_color(id: u32) -> (u8, u8, u8) {
    let [_, r, g, b] = id.to_be_bytes();
    (r, g, b)
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for group in data.chunks(3) {
        let b = [
            group[0],
            *group.get(1).unwrap_or(&0),
            *group.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= group.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut v = PNG_MAGIC.to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v
    }

    #[test]
    fn base64_should_match_rfc4648_vectors() {
        let got: Vec<String> = ["", "f", "fo", "foo", "foob"]
            .iter()
            .map(|s| base64(s.as_bytes()))
            .collect();
        assert_eq!(got, ["", "Zg==", "Zm8=", "Zm9v", "Zm9vYg=="]);
    }

    #[test]
    fn png_size_should_read_ihdr() {
        assert_eq!(png_size(&png_header(640, 480)), Some((640, 480)));
    }

    #[test]
    fn png_size_should_reject_jpeg() {
        assert_eq!(png_size(b"\xff\xd8\xff\xe0 not a png at all......"), None);
    }

    #[test]
    fn fit_should_keep_small_images_at_natural_size() {
        assert_eq!(fit((100, 40), (10, 20), 80, 30, 1.0), (10, 2));
    }

    #[test]
    fn fit_should_shrink_wide_images_to_width() {
        assert_eq!(fit((2000, 1000), (10, 20), 50, 100, 1.0), (50, 13));
    }

    #[test]
    fn fit_should_shrink_tall_images_to_height() {
        assert_eq!(fit((1000, 2000), (10, 20), 80, 10, 1.0), (10, 10));
    }

    #[test]
    fn fit_should_overflow_width_when_zoomed_in() {
        assert_eq!(fit((2000, 1000), (10, 20), 50, 100, 2.0), (100, 25));
    }

    #[test]
    fn fit_should_shrink_when_zoomed_out() {
        assert_eq!(fit((100, 40), (10, 20), 80, 30, 0.5), (5, 1));
    }

    #[test]
    fn placeholder_row_should_mark_only_first_cell() {
        assert_eq!(
            placeholder_row(2, 0, 3),
            "\u{10EEEE}\u{030E}\u{0305}\u{10EEEE}\u{10EEEE}"
        );
    }

    #[test]
    fn placeholder_row_should_encode_start_column() {
        assert!(placeholder_row(0, 1, 1).ends_with('\u{030D}'));
    }

    #[test]
    fn id_color_should_encode_low_24_bits() {
        assert_eq!(id_color(0x0012_3456), (0x12, 0x34, 0x56));
    }

    #[test]
    fn write_png_should_split_payload_into_chunks() {
        let mut out = String::new();
        write_png(&mut out, 7, &vec![0u8; 4000], 4, 2);
        assert_eq!(out.matches("\x1b_G").count(), 2);
    }
}
