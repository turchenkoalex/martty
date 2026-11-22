use std::ops::Range;
use std::path::{Path, PathBuf};

use pulldown_cmark::HeadingLevel;

use super::span::Span;
use super::upload::{Job, Upload};
use super::wrap::wrap;
use super::{ImageRow, Renderer};
use crate::cache;
use crate::kitty::{self, Transmit};
use crate::mermaid;
use crate::picture::{self, Picture};
use crate::style::{Color, Style};

/// An image sized for the screen, its upload not made yet.
pub(super) struct Sized {
    pub(super) cols: u32,
    pub(super) rows: u32,
    pub(super) job: Job,
}

pub(super) struct PendingImage {
    pub(super) url: String,
    /// Alt-text spans in `Renderer::inline`.
    pub(super) alt: Range<usize>,
}

impl Renderer<'_> {
    /// The image at `url`, read and recognized, with its cache key; `None`
    /// when images are off or it can't be shown.
    pub(super) fn load_image(&self, url: &str) -> Option<(Picture, u64)> {
        self.cfg.images.as_ref()?;
        load(&self.cfg.base_dir, url)
    }

    /// The cell box of an image in the text's width, and the job that
    /// draws it into that box.
    pub(super) fn size_image(&self, (picture, source): (Picture, u64)) -> Option<Sized> {
        let ctx = self.cfg.images.as_ref()?;
        let max_cols = u32::try_from(self.avail()).unwrap_or(u32::MAX);
        let px = picture.pixels(css_px(ctx.cell));
        let (cols, rows) = kitty::fit(px, ctx.cell, max_cols, ctx.max_rows, ctx.zoom);
        let target = (cols * ctx.cell.0, rows * ctx.cell.1);
        let key = cache::key(("image", source, target, cols, rows));
        let job =
            Box::new(move || cache::transmit(key, || picture.encode(target)?.transmit(cols, rows)));
        Some(Sized { cols, rows, job })
    }

    /// Emits an image's placeholder rows as regular lines, after any
    /// container prefixes and `indent` cells of space. `job` makes its
    /// upload, once the pager is about to show it.
    pub(super) fn place(&mut self, cols: u32, rows: u32, indent: usize, job: Job) -> u32 {
        self.place_row(vec![Sized { cols, rows, job }], indent)[0]
    }

    /// Places `images` side by side, a cell apart, like the inline images
    /// of a paragraph (a row of badges). Each is its own run of placeholder
    /// cells, which carries its row and column; one shorter than the row
    /// leaves blank cells below it. Only a lone image can be wider than the
    /// line and be panned. Returns the images' ids.
    pub(super) fn place_row(&mut self, images: Vec<Sized>, indent: usize) -> Vec<u32> {
        let ids: Vec<u32> = images.iter().map(|_| self.next_image_id()).collect();
        let avail = u32::try_from(self.avail().saturating_sub(indent)).unwrap_or(u32::MAX);
        let lone = images.len() == 1;
        let height = images.iter().map(|i| i.rows).max().unwrap_or(0);
        let first = self.line_no;
        for row in 0..height.min(kitty::MAX_ROWS) {
            let start = self.out.len();
            self.write_prefix();
            self.out.extend(std::iter::repeat_n(' ', indent));
            let prefix_len = self.out.len() - start;
            for (n, (image, &id)) in images.iter().zip(&ids).enumerate() {
                if n > 0 {
                    self.pen
                        .write(&mut self.out, &Span::new(" ", Style::default()), None);
                }
                let visible = image.cols.min(avail);
                let cells = if row < image.rows {
                    Span::new(kitty::placeholder_row(row, 0, visible), image_style(id))
                } else {
                    Span::new(" ".repeat(visible as usize), Style::default())
                };
                self.pen.write(&mut self.out, &cells, None);
                if lone {
                    let info = ImageRow {
                        id,
                        row,
                        cols: image.cols,
                        visible,
                        prefix_len,
                    };
                    self.image_rows.push((self.line_no, info));
                }
            }
            self.end_line();
        }
        for (image, id) in images.into_iter().zip(&ids) {
            let rows = image.rows.min(kitty::MAX_ROWS) as usize;
            self.uploads.push(Upload {
                id: *id,
                lines: first..first + rows,
                job: image.job,
            });
        }
        ids
    }

    /// Allocates the next image id and transmits the image into the
    /// preamble, for images inside a line of text.
    pub(super) fn upload(&mut self, transmit: &Transmit) -> u32 {
        let id = self.next_image_id();
        transmit.write(&mut self.preamble, id);
        id
    }

    fn next_image_id(&mut self) -> u32 {
        let n = u32::try_from(self.image_ids.len()).unwrap_or(0);
        let id = (self.cfg.first_image_id.wrapping_add(n) & 0x00FF_FFFF).max(1);
        self.image_ids.push(id);
        id
    }

    /// Lays a mermaid block out as an image. `Ok(false)` when diagrams are
    /// off; on a parse error its first line, for the block shown as code.
    pub(super) fn diagram(&mut self, source: &str) -> Result<bool, String> {
        let cfg = self.cfg;
        let Some(d) = &cfg.diagrams else {
            return Ok(false);
        };
        let view = (u32::try_from(self.avail()).unwrap_or(u32::MAX), d.max_rows);
        match mermaid::layout(source, &d.palette, d.cell, view, d.zoom, d.max_upscale) {
            Ok(diagram) => {
                let spare = usize::try_from(view.0.saturating_sub(diagram.cols)).unwrap_or(0);
                let indent = if d.center { spare / 2 } else { 0 };
                let key = cache::key((
                    "diagram",
                    source,
                    &d.palette,
                    d.cell,
                    view,
                    d.zoom.to_bits(),
                    d.max_upscale.to_bits(),
                ));
                let (cols, rows) = (diagram.cols, diagram.rows);
                let job = Box::new(move || cache::transmit(key, || diagram.transmit()));
                let id = self.place(cols, rows, indent, job);
                self.diagrams.push((id, source.to_owned()));
                Ok(true)
            }
            Err(e) => Err(e.lines().next().unwrap_or_default().to_owned()),
        }
    }

    /// Draws H1/H2 as bitmaps two rows tall. Returns `false` (having written
    /// nothing) when styled text should be used instead.
    pub(super) fn image_heading(&mut self, level: HeadingLevel, spans: &[Span]) -> bool {
        const ROWS: u32 = 2;
        let cfg = self.cfg;
        let Some(h) = &cfg.headings else {
            return false;
        };
        let color = h.color;
        let scale = match level {
            HeadingLevel::H1 => 2.0,
            HeadingLevel::H2 => 1.5,
            HeadingLevel::H3 => 1.25,
            _ => return false,
        };
        let text: String = spans.iter().map(|s| s.text.as_str()).collect();
        if !self.containers.is_empty() || !h.font.supports(&text) {
            return false;
        }

        let avail = self.avail();
        let per_line = ((avail as f32 / scale).floor() as usize).max(1);
        let mut lines = Vec::new();
        for line in wrap(spans, per_line) {
            let line: String = line.iter().map(|s| s.text.as_str()).collect();
            let line = line.trim_end().to_owned();
            match h.font.cols(&line, h.cell, scale) {
                Some(cols) => lines.push((line, cols)),
                None => return false,
            }
        }
        let max_cols = u32::try_from(avail).unwrap_or(u32::MAX);
        let (font, cell) = (h.font, h.cell);
        for (line, natural) in lines {
            let cols = natural.min(max_cols);
            let key = cache::key(("heading", &line, cell, scale.to_bits(), color, cols));
            let job = Box::new(move || {
                cache::transmit(key, || {
                    let b = font.render(&line, cell, scale, ROWS, color)?;
                    Some(Transmit::rgba(&b.rgba, (b.width, b.height), cols, ROWS))
                })
            });
            self.place(cols, ROWS, 0, job);
        }
        true
    }
}

/// Placeholder cells name their image by the foreground color.
pub(super) fn image_style(id: u32) -> Style {
    let (r, g, b) = kitty::id_color(id);
    Style::fg(Color::Rgb(r, g, b))
}

/// Device pixels per CSS pixel, for SVG images: CSS's default 16px font
/// is taken to be the terminal's font, which is a cell height / 1.25.
fn css_px(cell: (u32, u32)) -> f32 {
    cell.1 as f32 / 1.25 / 16.0
}

/// Reads and recognizes the image at `url`: a local path or an http(s) URL.
/// Also returns what tells this image from others for the cache: the URL,
/// or for a file its path, size and modification time.
fn load(base: &Path, url: &str) -> Option<(Picture, u64)> {
    if crate::remote::is_remote(url) {
        let data = crate::remote::fetch(url)?;
        let name = url.split(['?', '#']).next().unwrap_or(url);
        return Some((Picture::new_remote(data, Path::new(name))?, cache::key(url)));
    }
    let path = resolve(base, url)?;
    let meta = std::fs::metadata(&path).ok()?;
    let source = cache::key((&path, meta.len(), meta.modified().ok()));
    Some((Picture::new(picture::read_file(&path)?, &path)?, source))
}

/// Local path for an image URL; remote URLs give `None`.
pub(super) fn resolve(base: &Path, url: &str) -> Option<PathBuf> {
    let url = url.strip_prefix("file://").unwrap_or(url);
    if url.contains("://") || url.starts_with("data:") {
        return None;
    }
    let path = PathBuf::from(super::links::percent_decode(url));
    Some(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_should_reject_remote_urls() {
        assert_eq!(resolve(Path::new("/x"), "https://e.com/a.png"), None);
    }
}
