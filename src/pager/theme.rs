use std::fs::File;
use std::io;

use super::keys::{Key, parse_keys};
use super::{Layout, Pager, Params};
use crate::render::Document;
use crate::term;

impl Pager<'_> {
    /// Asks the terminal for its colors after a theme change and, when they
    /// differ, lays every view out again in them, re-sending its images:
    /// headings and diagrams are rasterized in the theme's colors. Returns
    /// the keys typed while waiting for the reply.
    pub(super) fn retheme(
        &mut self,
        tty: &mut File,
        render: &mut impl FnMut(Layout<'_>, Params) -> Document,
    ) -> io::Result<Vec<Key>> {
        let (theme, typed) = term::query_theme(tty)?;
        // No reply in time: keep the colors we have.
        if theme.bg.is_some() && theme != self.theme {
            self.theme = theme;
            for depth in 0..self.views.len() {
                self.relayout(depth, render)?;
            }
            self.refresh_hits();
        }
        Ok(parse_keys(&typed))
    }
}
