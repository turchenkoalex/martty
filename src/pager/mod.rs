//! A small `less`-like pager on the alternate screen.
//!
//! The document is a list of finished lines (images are Unicode placeholder
//! text, see `kitty.rs`), so scrolling is just redrawing a window of lines.
//! Redraws are wrapped in synchronized output (DEC 2026) to avoid tearing.
//!
//! Views form a stack: the document, and on top of it optionally a single
//! diagram opened full screen (click on it, or `z`). `+`/`-` re-render
//! diagrams at another zoom; images wider than the screen are panned with
//! `h`/`l` by redrawing their placeholder rows from a later image column —
//! the image itself is not re-sent.
//!
//! Mouse reporting (SGR, DEC 1000/1006) gives clicks and the wheel; it also
//! captures plain drag selection — in Ghostty, Shift+drag still selects.

mod draw;
mod input;
mod keys;
#[cfg(test)]
mod tests;
mod view;

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::AsRawFd;

use crate::kitty;
use crate::render::Document;
use crate::term::{self, RawMode, Size};

use keys::read_keys;
use view::View;

const ENTER: &str = "\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[?1000h\x1b[?1006h";
const LEAVE: &str = "\x1b[?1006l\x1b[?1000l\x1b[?7h\x1b[?25h\x1b[?1049l";

/// What to lay out: the whole document or one diagram full screen.
pub enum Layout<'a> {
    Document,
    Diagram(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Continue,
    Quit,
    /// Re-render the current view (zoom changed).
    Relayout,
    /// Open this diagram source full screen.
    Open(String),
    /// Leave the full-screen diagram.
    Close,
}

/// Runs the pager until the user quits. `render` lays out the document or a
/// single diagram for a screen size and diagram zoom. `pending` holds keys
/// already read from the tty (during the capability probe), handled first.
pub fn run(
    title: &str,
    doc: Document,
    pending: &[u8],
    mut render: impl FnMut(Layout<'_>, Size, f32) -> Document,
) -> io::Result<()> {
    let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let _raw = RawMode::keys(tty.as_raw_fd())?;
    let mut pager = Pager::new(title, term::size(), doc);
    pager.enter()?;

    let mut keys = keys::parse_keys(pending);
    loop {
        for key in keys {
            if pager.apply(key, &mut render)? == Action::Quit {
                return Ok(());
            }
        }
        pager.redraw()?;
        keys = match read_keys(&mut tty)? {
            Some(keys) => keys,
            None => {
                let size = term::size();
                if (size.cols, size.rows) != (pager.size.cols, pager.size.rows) {
                    pager.size = size;
                    for depth in 0..pager.views.len() {
                        pager.relayout(depth, &mut render)?;
                    }
                }
                Vec::new()
            }
        };
    }
}

struct Pager<'a> {
    title: &'a str,
    size: Size,
    /// Never empty: the document, then optionally a full-screen diagram.
    views: Vec<View>,
    query: Option<String>,
    /// Search input being typed, when the `/` prompt is open.
    prompt: Option<String>,
    message: Option<String>,
    /// Set once the alternate screen is entered; restored on drop.
    active: bool,
}

impl<'a> Pager<'a> {
    fn new(title: &'a str, size: Size, doc: Document) -> Self {
        Self {
            title,
            size,
            views: vec![View::new(doc, None)],
            query: None,
            prompt: None,
            message: None,
            active: false,
        }
    }

    fn view(&self) -> &View {
        &self.views[self.views.len() - 1]
    }

    fn view_mut(&mut self) -> &mut View {
        let last = self.views.len() - 1;
        &mut self.views[last]
    }

    fn view_rows(&self) -> usize {
        self.size.rows.saturating_sub(1).max(1)
    }

    fn enter(&mut self) -> io::Result<()> {
        self.active = true;
        let mut out = ENTER.to_owned();
        out.push_str(&self.views[0].doc.preamble);
        write_out(&out)
    }

    fn redraw(&self) -> io::Result<()> {
        let mut frame = String::new();
        self.draw(&mut frame);
        write_out(&frame)
    }

    fn push(&mut self, view: View) -> io::Result<()> {
        write_out(&view.doc.preamble)?;
        self.views.push(view);
        Ok(())
    }

    fn pop(&mut self) -> io::Result<()> {
        if self.views.len() > 1
            && let Some(view) = self.views.pop()
        {
            write_out(&free_images(&view.doc))?;
        }
        Ok(())
    }

    /// Handles one key; returns `Action::Quit` when the pager should exit.
    fn apply(
        &mut self,
        key: keys::Key,
        render: &mut impl FnMut(Layout<'_>, Size, f32) -> Document,
    ) -> io::Result<Action> {
        let action = self.handle(key);
        match &action {
            Action::Continue | Action::Quit => {}
            Action::Relayout => self.relayout(self.views.len() - 1, render)?,
            Action::Open(source) => {
                let doc = render(Layout::Diagram(source), self.size, 1.0);
                self.push(View::new(doc, Some(source.clone())))?;
            }
            Action::Close => self.pop()?,
        }
        Ok(action)
    }

    /// Re-renders the view at `depth`, keeping its relative position.
    fn relayout(
        &mut self,
        depth: usize,
        render: &mut impl FnMut(Layout<'_>, Size, f32) -> Document,
    ) -> io::Result<()> {
        let size = self.size;
        let view = &mut self.views[depth];
        let layout = match &view.source {
            Some(src) => Layout::Diagram(src),
            None => Layout::Document,
        };
        let doc = render(layout, size, view.zoom);
        let mut out = free_images(&view.doc);
        out.push_str(&doc.preamble);
        write_out(&out)?;

        let rows = size.rows.saturating_sub(1).max(1);
        let anchor = view.top as f64 / view.total().max(1) as f64;
        let (zoom, pan, source) = (view.zoom, view.pan, view.source.take());
        *view = View::new(doc, source);
        view.zoom = zoom;
        let max_top = view.total().saturating_sub(rows);
        view.top = ((anchor * view.total() as f64) as usize).min(max_top);
        view.pan = pan.min(view.doc.max_pan());
        Ok(())
    }
}

impl Drop for Pager<'_> {
    /// Restores the terminal and frees uploaded images, even on errors.
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut out = String::new();
        for view in &self.views {
            out.push_str(&free_images(&view.doc));
        }
        out.push_str(LEAVE);
        let _ = write_out(&out);
    }
}

fn write_out(s: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(s.as_bytes())?;
    out.flush()
}

fn free_images(doc: &Document) -> String {
    let mut out = String::new();
    for &id in &doc.image_ids {
        kitty::write_delete(&mut out, id);
    }
    out
}
