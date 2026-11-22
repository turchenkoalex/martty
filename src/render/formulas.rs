//! `$…$` and `$$…$$` (and ```` ```math ```` blocks) as images, falling back
//! to code when graphics are off or latex-rust can't render the formula.

use super::Renderer;
use super::images::image_style;
use super::span::Span;
use crate::{kitty, math};

impl Renderer<'_> {
    /// An inline formula becomes one row of placeholder cells inside the
    /// paragraph, so it wraps with the text around it.
    pub(super) fn inline_math(&mut self, latex: &str) {
        let cfg = self.cfg;
        let raster = cfg
            .math
            .as_ref()
            .and_then(|m| math::inline(latex, m.color, m.cell))
            // A placeholder run split across lines would lose its diacritics.
            .filter(|r| usize::try_from(r.cols).is_ok_and(|c| c <= self.avail()));
        let Some(r) = raster else {
            self.push_code(latex.trim());
            return;
        };
        let id = self.upload(|out, id| {
            kitty::write_rgba(out, id, &r.rgba, (r.width, r.height), r.cols, r.rows);
        });
        let mut style = self.style();
        style.fg = image_style(id).fg;
        self.inline
            .push(Span::new(kitty::placeholder_row(0, 0, r.cols), style));
    }

    /// A display formula inside a paragraph splits it: the text before, the
    /// formula centred on its own rows, then the text after.
    pub(super) fn display_math(&mut self, latex: &str) {
        let Some(raster) = self.math_raster(latex) else {
            self.push_code(latex.trim());
            return;
        };
        self.flush_inline();
        self.place_formula(&raster);
    }

    /// A ```` ```math ```` block; `false` shows it as code instead.
    pub(super) fn math_block(&mut self, latex: &str) -> bool {
        let Some(raster) = self.math_raster(latex) else {
            return false;
        };
        self.place_formula(&raster);
        true
    }

    fn math_raster(&self, latex: &str) -> Option<math::Raster> {
        let m = self.cfg.math.as_ref()?;
        math::display(latex, m.color, m.cell)
    }

    fn place_formula(&mut self, r: &math::Raster) {
        let avail = u32::try_from(self.avail()).unwrap_or(u32::MAX);
        let indent = usize::try_from(avail.saturating_sub(r.cols) / 2).unwrap_or(0);
        self.place(r.cols, r.rows, indent, |out, id| {
            kitty::write_rgba(out, id, &r.rgba, (r.width, r.height), r.cols, r.rows);
        });
    }
}
