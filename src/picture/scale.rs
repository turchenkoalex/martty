//! Area-average downscaling: each target pixel is the mean of the source
//! pixels it covers, weighted by alpha, so photos don't alias and
//! transparent edges don't darken.

/// Shrinks straight RGBA `size` to fit inside `target`, keeping the aspect
/// ratio. Smaller images come back unchanged; the terminal scales them up.
pub(super) fn downscale(
    rgba: Vec<u8>,
    size: (u32, u32),
    target: (u32, u32),
) -> (Vec<u8>, (u32, u32)) {
    let (sw, sh) = (size.0.max(1), size.1.max(1));
    let ratio = (f64::from(target.0) / f64::from(sw)).min(f64::from(target.1) / f64::from(sh));
    if ratio >= 1.0 || rgba.len() < (sw as usize) * (sh as usize) * 4 {
        return (rgba, size);
    }
    let tw = ((f64::from(sw) * ratio).round() as u32).max(1);
    let th = ((f64::from(sh) * ratio).round() as u32).max(1);
    let (sw, sh) = (sw as usize, sh as usize);
    let (tw_px, th_px) = (tw as usize, th as usize);
    // Source columns and rows each target column and row covers.
    let spans = |s: usize, t: usize| -> Vec<(usize, usize)> {
        (0..t)
            .map(|i| {
                let a = i * s / t;
                let b = ((i + 1) * s / t).max(a + 1).min(s);
                (a, b)
            })
            .collect()
    };
    let (xs, ys) = (spans(sw, tw_px), spans(sh, th_px));
    let mut out = vec![0u8; tw_px * th_px * 4];
    for (ty, &(y0, y1)) in ys.iter().enumerate() {
        for (tx, &(x0, x1)) in xs.iter().enumerate() {
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                for px in rgba[(y * sw + x0) * 4..(y * sw + x1) * 4]
                    .as_chunks::<4>()
                    .0
                {
                    let a = u64::from(px[3]);
                    sum[0] += u64::from(px[0]) * a;
                    sum[1] += u64::from(px[1]) * a;
                    sum[2] += u64::from(px[2]) * a;
                    sum[3] += a;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let o = (ty * tw_px + tx) * 4;
            for c in 0..3 {
                if let Some(v) = sum[c].checked_div(sum[3]) {
                    out[o + c] = v.min(255) as u8;
                }
            }
            out[o + 3] = (sum[3] / n).min(255) as u8;
        }
    }
    (out, (tw, th))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_should_keep_small_images() {
        let (out, size) = downscale(vec![1, 2, 3, 4], (1, 1), (10, 10));
        assert_eq!((out, size), (vec![1, 2, 3, 4], (1, 1)));
    }

    #[test]
    fn downscale_should_average_covered_pixels() {
        let rgba = [[200, 0, 0, 255], [0, 0, 200, 255]].concat();
        let (out, size) = downscale(rgba, (2, 1), (1, 1));
        assert_eq!((out, size), (vec![100, 0, 100, 255], (1, 1)));
    }

    #[test]
    fn downscale_should_ignore_color_of_transparent_pixels() {
        let rgba = [[255, 255, 255, 255], [0, 0, 0, 0]].concat();
        let (out, _) = downscale(rgba, (2, 1), (1, 1));
        assert_eq!(out, vec![255, 255, 255, 127]);
    }

    #[test]
    fn downscale_should_keep_aspect_ratio() {
        let (_, size) = downscale(vec![0; 400 * 100 * 4], (400, 100), (100, 100));
        assert_eq!(size, (100, 25));
    }
}
