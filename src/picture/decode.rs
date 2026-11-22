//! JPEG, GIF and WebP to RGBA frames, each scaled down to the target box.

use std::io::Cursor;

use super::scale::downscale;
use super::{Format, Frame, Frames};

/// Decoded frames kept per image, after scaling; an animation over this is
/// cut short rather than exhausting memory.
const MAX_ANIMATION_BYTES: usize = 64 << 20;
/// Browsers show GIF frames with a delay under 20 ms for 100 ms.
const MIN_GIF_DELAY_MS: u32 = 20;
const DEFAULT_GIF_DELAY_MS: u32 = 100;

pub(super) fn size(format: Format, data: &[u8]) -> Option<(u32, u32)> {
    match format {
        Format::Jpeg => {
            let mut decoder = jpeg_decoder(data);
            decoder.decode_headers().ok()?;
            let info = decoder.info()?;
            Some((u32::from(info.width), u32::from(info.height)))
        }
        // The logical screen size, right after the signature.
        Format::Gif => {
            let le = |i: usize| {
                Some(u32::from(u16::from_le_bytes([
                    *data.get(i)?,
                    *data.get(i + 1)?,
                ])))
            };
            Some((le(6)?, le(8)?))
        }
        Format::Webp => image_webp::WebPDecoder::new(Cursor::new(data))
            .ok()
            .map(|d| d.dimensions()),
        Format::Png | Format::Svg => None,
    }
}

/// Frames no larger than `target` (device pixels), keeping the aspect ratio.
pub(super) fn frames(format: Format, data: &[u8], target: (u32, u32)) -> Option<Frames> {
    match format {
        Format::Jpeg => jpeg(data, target),
        Format::Gif => gif(data, target),
        Format::Webp => webp(data, target),
        Format::Png | Format::Svg => None,
    }
}

fn jpeg_decoder(
    data: &[u8],
) -> zune_jpeg::JpegDecoder<zune_jpeg::zune_core::bytestream::ZCursor<&[u8]>> {
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(1 << 15)
        .set_max_height(1 << 15);
    zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options)
}

fn jpeg(data: &[u8], target: (u32, u32)) -> Option<Frames> {
    let mut decoder = jpeg_decoder(data);
    let rgba = decoder.decode().ok()?;
    let info = decoder.info()?;
    let size = (u32::from(info.width), u32::from(info.height));
    Some(still(rgba, size, target))
}

fn still(rgba: Vec<u8>, size: (u32, u32), target: (u32, u32)) -> Frames {
    let (rgba, size) = downscale(rgba, size, target);
    Frames {
        size,
        frames: vec![Frame { rgba, delay_ms: 0 }],
        plays: Some(1),
    }
}

/// Composes GIF frames onto the logical screen, applying each frame's
/// disposal, so every frame sent is a full picture.
fn gif(data: &[u8], target: (u32, u32)) -> Option<Frames> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(Cursor::new(data)).ok()?;
    let (w, h) = (usize::from(decoder.width()), usize::from(decoder.height()));
    let plays = match decoder.repeat() {
        gif::Repeat::Infinite => None,
        gif::Repeat::Finite(n) => Some(u32::from(n) + 1),
    };
    let mut canvas = vec![0u8; w.checked_mul(h)?.checked_mul(4)?];
    let mut out = Frames {
        size: (0, 0),
        frames: Vec::new(),
        plays,
    };
    let mut kept = 0;
    while let Ok(Some(frame)) = decoder.read_next_frame() {
        let saved = (frame.dispose == gif::DisposalMethod::Previous).then(|| canvas.clone());
        let rect = (
            usize::from(frame.left),
            usize::from(frame.top),
            usize::from(frame.width),
            usize::from(frame.height),
        );
        blit(&mut canvas, w, h, &frame.buffer, rect);
        let delay = u32::from(frame.delay) * 10;
        let delay_ms = if delay < MIN_GIF_DELAY_MS {
            DEFAULT_GIF_DELAY_MS
        } else {
            delay
        };
        let (rgba, size) = downscale(canvas.clone(), (w as u32, h as u32), target);
        kept += rgba.len();
        out.size = size;
        out.frames.push(Frame { rgba, delay_ms });
        match (frame.dispose, saved) {
            (gif::DisposalMethod::Background, _) => clear(&mut canvas, w, h, rect),
            (gif::DisposalMethod::Previous, Some(saved)) => canvas = saved,
            _ => {}
        }
        if kept > MAX_ANIMATION_BYTES {
            break;
        }
    }
    (!out.frames.is_empty()).then_some(out)
}

/// Draws a GIF frame's RGBA rectangle onto the canvas; transparent pixels
/// leave what is below.
fn blit(
    canvas: &mut [u8],
    w: usize,
    h: usize,
    src: &[u8],
    (x0, y0, fw, fh): (usize, usize, usize, usize),
) {
    for y in 0..fh.min(h.saturating_sub(y0)) {
        for x in 0..fw.min(w.saturating_sub(x0)) {
            let s = (y * fw + x) * 4;
            let Some(px) = src.get(s..s + 4) else {
                return;
            };
            if px[3] > 0 {
                let d = ((y0 + y) * w + x0 + x) * 4;
                canvas[d..d + 4].copy_from_slice(px);
            }
        }
    }
}

fn clear(canvas: &mut [u8], w: usize, h: usize, (x0, y0, fw, fh): (usize, usize, usize, usize)) {
    for y in y0..(y0 + fh).min(h) {
        let start = (y * w + x0.min(w)) * 4;
        let end = (y * w + (x0 + fw).min(w)) * 4;
        canvas[start..end].fill(0);
    }
}

fn webp(data: &[u8], target: (u32, u32)) -> Option<Frames> {
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(data)).ok()?;
    let size = decoder.dimensions();
    let alpha = decoder.has_alpha();
    let mut buf = vec![0u8; decoder.output_buffer_size()?];
    let to_rgba = |buf: &[u8]| -> Vec<u8> {
        if alpha {
            return buf.to_vec();
        }
        buf.as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    };
    if !decoder.is_animated() {
        decoder.read_image(&mut buf).ok()?;
        return Some(still(to_rgba(&buf), size, target));
    }
    let plays = match decoder.loop_count() {
        image_webp::LoopCount::Forever => None,
        image_webp::LoopCount::Times(n) => Some(u32::from(n.get())),
    };
    let mut out = Frames {
        size: (0, 0),
        frames: Vec::new(),
        plays,
    };
    let mut kept = 0;
    while let Ok(delay_ms) = decoder.read_frame(&mut buf) {
        let (rgba, scaled) = downscale(to_rgba(&buf), size, target);
        kept += rgba.len();
        out.size = scaled;
        out.frames.push(Frame {
            rgba,
            delay_ms: delay_ms.max(MIN_GIF_DELAY_MS),
        });
        if kept > MAX_ANIMATION_BYTES {
            break;
        }
    }
    (!out.frames.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_gif(frames: &[(u16, u16, [u8; 4], gif::DisposalMethod)]) -> Vec<u8> {
        let mut data = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut data, 2, 2, &[]).unwrap_or_else(|e| panic!("{e}"));
            let _ = enc.set_repeat(gif::Repeat::Infinite);
            for &(x, y, color, dispose) in frames {
                let mut px = color.to_vec();
                let mut frame = gif::Frame::from_rgba(1, 1, &mut px);
                frame.left = x;
                frame.top = y;
                frame.delay = 5;
                frame.dispose = dispose;
                enc.write_frame(&frame).unwrap_or_else(|e| panic!("{e}"));
            }
        }
        data
    }

    #[test]
    fn size_should_read_gif_screen() {
        let data = encode_gif(&[(0, 0, [255, 0, 0, 255], gif::DisposalMethod::Keep)]);
        assert_eq!(size(Format::Gif, &data), Some((2, 2)));
    }

    #[test]
    fn gif_should_compose_frames_on_the_canvas() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let keep = gif::DisposalMethod::Keep;
        let data = encode_gif(&[(0, 0, red, keep), (1, 1, blue, keep)]);
        let f = gif(&data, (100, 100)).unwrap_or_else(|| panic!("decodes"));
        assert_eq!((f.size, f.frames.len(), f.plays), ((2, 2), 2, None));
        let second = &f.frames[1].rgba;
        assert_eq!(&second[0..4], &red, "first frame kept under the second");
        assert_eq!(&second[12..16], &blue);
        assert_eq!(f.frames[1].delay_ms, 50);
    }

    #[test]
    fn gif_should_clear_background_disposal() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let data = encode_gif(&[
            (0, 0, red, gif::DisposalMethod::Background),
            (1, 1, blue, gif::DisposalMethod::Keep),
        ]);
        let f = gif(&data, (100, 100)).unwrap_or_else(|| panic!("decodes"));
        assert_eq!(f.frames[1].rgba[3], 0, "red was disposed of");
    }
}
