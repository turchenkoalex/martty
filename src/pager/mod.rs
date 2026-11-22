//! A small `less`-like pager on the alternate screen.
//!
//! The document is a list of finished lines (images are Unicode placeholder
//! text, see `kitty.rs`), so scrolling is just redrawing a window of lines.
//! Redraws are wrapped in synchronized output (DEC 2026) to avoid tearing.
//! Images are uploaded only as they come within a screen of the view.
//!
//! Views form a stack: the document, and on top of it optionally a single
//! diagram opened full screen (`z`). `+`/`-` re-render
//! images and diagrams at another zoom; ones wider than the screen are panned with
//! `h`/`l` by redrawing their placeholder rows from a later image column —
//! the image itself is not re-sent.
//!
//! With following on (`-w`, `F`), the file is checked for changes on every
//! wake-up and laid out again when it changes, keeping the place by content.
//!
//! Ghostty reports theme switches (DEC 2031); the pager then asks for the
//! colors again and lays every view out in them.
//!
//! Keys use the kitty keyboard protocol ("disambiguate", `CSI > 1 u`), so
//! Esc and Alt combinations are told apart. Its flags are kept per screen,
//! so they are pushed after entering the alternate screen and popped before
//! leaving it.
//!
//! The mouse is left to Ghostty, so selecting text and Cmd+clicking OSC 8
//! links work as anywhere else. Alternate scroll mode (DEC 1007) makes
//! Ghostty turn the wheel into arrow keys on the alternate screen.

mod copy;
mod draw;
mod input;
mod keys;
mod links;
mod overlay;
mod search;
#[cfg(test)]
mod tests;
mod theme;
mod view;

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::render::Document;
use crate::term::{self, RawMode, Size, Theme};
use crate::{kitty, progress};

use keys::read_keys;
use overlay::Overlay;
use view::{Kind, View};

const ENTER: &str = "\x1b[?1049h\x1b[>1u\x1b[?25l\x1b[?7l\x1b[?1007h\x1b[?2031h";
const LEAVE: &str = "\x1b[?2031l\x1b[?1007l\x1b[?7h\x1b[?25h\x1b[<u\x1b[?1049l";

/// What to lay out: the document, a Markdown file opened from a link, or
/// one diagram full screen.
pub enum Layout<'a> {
    Document,
    File(&'a Path),
    Diagram(&'a str),
}

/// How to lay a view out.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub size: Size,
    /// Image and diagram zoom; 1.0 fits the screen.
    pub zoom: f32,
    /// Code and tables at their natural width, panned instead of wrapped.
    pub overflow: bool,
    /// The terminal's colors, updated when its theme changes.
    pub theme: Theme,
}

/// What the pager shows and how it starts.
pub struct Options<'a> {
    pub title: &'a str,
    /// The file shown, for following its changes; `None` for stdin.
    pub file: Option<&'a Path>,
    /// Follow changes from the start (`-w`).
    pub follow: bool,
    /// The colors probed at start.
    pub theme: Theme,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Continue,
    Quit,
    /// Re-render the current view (zoom changed).
    Relayout,
    /// Re-render every view (wrapping of wide blocks changed).
    Reflow,
    /// Put this text on the clipboard.
    Copy(String),
    /// Open full screen this diagram of the current view (an index into
    /// its `diagrams`).
    Open(usize),
    /// Switch the full-screen diagram to the next (`true`) or previous one
    /// in the document.
    Switch(bool),
    /// Go back from a full-screen diagram or an opened file.
    Close,
    /// Open a link target: `#anchor`, a Markdown file, or anything else
    /// through the system.
    Follow(String),
}

/// Runs the pager until the user quits. `render` lays out the document, a
/// linked file or a single diagram. `pending` holds keys already read from
/// the tty (during the capability probe), handled first.
pub fn run(
    options: &Options<'_>,
    doc: Document,
    pending: &[u8],
    mut render: impl FnMut(Layout<'_>, Params) -> Document,
) -> io::Result<()> {
    let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let _raw = RawMode::keys(tty.as_raw_fd())?;
    let mut pager = Pager::new(options.title, term::size(), doc);
    pager.file = options.file.map(Path::to_path_buf);
    pager.follow = options.follow && pager.file.is_some();
    pager.theme = options.theme;
    pager.views[0].modified = pager.file.as_deref().and_then(modified);
    pager.enter()?;

    let mut keys = keys::parse_keys(pending);
    // Frames are drawn only after input or a resize, not on every wake-up.
    let mut dirty = true;
    loop {
        // Keys typed while the colors were asked for again, handled next.
        let mut later = Vec::new();
        for key in keys {
            dirty = true;
            if key == keys::Key::ThemeChanged {
                later.extend(pager.retheme(&mut tty, &mut render)?);
            } else if pager.apply(key, &mut render)? == Action::Quit {
                return Ok(());
            }
        }
        if !later.is_empty() {
            keys = later;
            continue;
        }
        if pager.reload_changed(&mut render)? {
            dirty = true;
        }
        if dirty {
            pager.redraw()?;
            dirty = false;
        }
        keys = match read_keys(&mut tty)? {
            Some(keys) => keys,
            None => {
                let size = term::size();
                if (size.cols, size.rows) != (pager.size.cols, pager.size.rows) {
                    pager.size = size;
                    for depth in 0..pager.views.len() {
                        pager.relayout(depth, &mut render)?;
                    }
                    dirty = true;
                }
                Vec::new()
            }
        };
    }
}

struct Pager<'a> {
    title: &'a str,
    size: Size,
    /// Never empty: the document, then files opened from links and a
    /// full-screen diagram on top of it.
    views: Vec<View>,
    query: Option<String>,
    /// Search input being typed, when the `/` prompt is open.
    prompt: Option<String>,
    message: Option<String>,
    overlay: Option<Overlay>,
    /// `m` or `'` typed, waiting for the mark's letter.
    pending: Option<char>,
    /// Lay code and tables out unwrapped, for panning (`w`).
    overflow: bool,
    /// Reload views whose file changed (`-w`, `F`).
    follow: bool,
    /// The document's file; `None` when it came from stdin.
    file: Option<PathBuf>,
    /// The terminal's colors, which views are laid out in.
    theme: Theme,
    /// Set once the alternate screen is entered; restored on drop.
    active: bool,
}

impl<'a> Pager<'a> {
    fn new(title: &'a str, size: Size, doc: Document) -> Self {
        Self {
            title,
            size,
            views: vec![View::new(doc, Kind::Document)],
            query: None,
            prompt: None,
            message: None,
            overlay: None,
            pending: None,
            overflow: false,
            follow: false,
            file: None,
            theme: Theme::default(),
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

    /// Draws the screen, first uploading the images on it and on a screen
    /// above and below, so scrolling to them finds them ready.
    fn redraw(&mut self) -> io::Result<()> {
        let rows = self.view_rows();
        let view = self.view_mut();
        let near = view.top.saturating_sub(rows)..view.top.saturating_add(2 * rows);
        let mut frame = if view.doc.has_uploads(near.clone()) {
            let _busy = progress::Busy::start();
            view.doc.take_uploads(near)
        } else {
            String::new()
        };
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
        render: &mut impl FnMut(Layout<'_>, Params) -> Document,
    ) -> io::Result<Action> {
        let action = self.handle(key);
        match &action {
            Action::Continue | Action::Quit => {}
            Action::Relayout => self.relayout(self.views.len() - 1, render)?,
            Action::Reflow => {
                for depth in 0..self.views.len() {
                    self.relayout(depth, render)?;
                }
            }
            Action::Copy(text) => write_out(&copy::osc52(text))?,
            Action::Open(index) => self.open_diagram(*index, render)?,
            Action::Switch(forward) => self.switch_diagram(*forward, render)?,
            Action::Close => self.pop()?,
            Action::Follow(url) => self.follow(url, render)?,
        }
        self.refresh_hits();
        Ok(action)
    }

    /// Opens diagram `index` of the current view full screen, scrolling
    /// the view to it first, so going back lands on the diagram last shown.
    fn open_diagram(
        &mut self,
        index: usize,
        render: &mut impl FnMut(Layout<'_>, Params) -> Document,
    ) -> io::Result<()> {
        let rows = self.view_rows();
        let view = self.view_mut();
        let Some((id, source)) = view.doc.diagrams.get(index).cloned() else {
            return Ok(());
        };
        let line = view
            .doc
            .lines
            .iter()
            .position(|l| l.image.is_some_and(|i| i.id == id));
        if let Some(line) = line.filter(|&l| l < view.top || l >= view.top + rows) {
            view.top = line.min(view.total().saturating_sub(rows));
        }
        let doc = render(Layout::Diagram(&source), self.params(1.0));
        let mut diagram = View::new(doc, Kind::Diagram(source));
        diagram.diagram = Some(index);
        self.push(diagram)
    }

    /// Replaces the full-screen diagram with its neighbour in the view
    /// below. Diagrams are rendered from their source when shown, so this
    /// needs nothing rasterized in the document.
    fn switch_diagram(
        &mut self,
        forward: bool,
        render: &mut impl FnMut(Layout<'_>, Params) -> Document,
    ) -> io::Result<()> {
        let depth = self.views.len();
        let (Some(current), Some(parent)) = (self.view().diagram, depth.checked_sub(2)) else {
            return Ok(());
        };
        let count = self.views[parent].doc.diagrams.len();
        let next = if forward {
            Some(current + 1).filter(|&n| n < count)
        } else {
            current.checked_sub(1)
        };
        let Some(next) = next else {
            let end = if forward { "last" } else { "first" };
            self.message = Some(format!("This is the {end} diagram"));
            return Ok(());
        };
        self.pop()?;
        self.open_diagram(next, render)
    }

    fn params(&self, zoom: f32) -> Params {
        Params {
            size: self.size,
            zoom,
            overflow: self.overflow,
            theme: self.theme,
        }
    }

    /// The file the view at `depth` shows, if any.
    fn path(&self, depth: usize) -> Option<&Path> {
        match &self.views[depth].kind {
            Kind::Document => self.file.as_deref(),
            Kind::File(path) => Some(path),
            Kind::Diagram(_) => None,
        }
    }

    /// Lays out again the views whose file changed, when following.
    /// Returns whether any did.
    fn reload_changed(
        &mut self,
        render: &mut impl FnMut(Layout<'_>, Params) -> Document,
    ) -> io::Result<bool> {
        if !self.follow {
            return Ok(false);
        }
        let mut changed = false;
        for depth in 0..self.views.len() {
            let time = self.path(depth).and_then(modified);
            if time.is_some() && time != self.views[depth].modified {
                self.relayout(depth, render)?;
                changed = true;
            }
        }
        if changed {
            self.refresh_hits();
        }
        Ok(changed)
    }

    /// Scrolls `line` to the top, remembering where the jump started (`''`).
    fn jump(&mut self, line: usize) {
        let rows = self.view_rows();
        let view = self.view_mut();
        view.marks.insert('\'', view.top);
        view.top = line.min(view.total().saturating_sub(rows));
    }

    fn follow(
        &mut self,
        url: &str,
        render: &mut impl FnMut(Layout<'_>, Params) -> Document,
    ) -> io::Result<()> {
        if let Some(anchor) = url.strip_prefix('#') {
            self.jump_to(anchor);
            return Ok(());
        }
        let Some(local) = url.strip_prefix("file://") else {
            self.message = Some(links::open_external(url.as_ref()));
            return Ok(());
        };
        let (path, fragment) = match local.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (local, None),
        };
        let path = PathBuf::from(path.replace("%20", " "));
        if !links::is_markdown(&path) {
            self.message = Some(links::open_external(path.as_os_str()));
            return Ok(());
        }
        if !path.is_file() {
            self.message = Some(format!("No such file: {}", path.display()));
            return Ok(());
        }
        let doc = render(Layout::File(&path), self.params(1.0));
        let mut view = View::new(doc, Kind::File(path.clone()));
        view.modified = modified(&path);
        self.push(view)?;
        if let Some(anchor) = fragment {
            self.jump_to(anchor);
        }
        Ok(())
    }

    /// Scrolls the heading with this anchor to the top.
    fn jump_to(&mut self, anchor: &str) {
        match self.view().anchor_line(anchor) {
            Some(line) => self.jump(line),
            None => self.message = Some(format!("No heading #{anchor}")),
        }
    }

    /// Re-renders the view at `depth`, keeping the place by content.
    fn relayout(
        &mut self,
        depth: usize,
        render: &mut impl FnMut(Layout<'_>, Params) -> Document,
    ) -> io::Result<()> {
        let rows = self.view_rows();
        let params = self.params(self.views[depth].zoom);
        let path = self.path(depth).map(Path::to_path_buf);
        let old = &self.views[depth];
        let doc = match &old.kind {
            Kind::Document => render(Layout::Document, params),
            Kind::File(path) => render(Layout::File(path), params),
            Kind::Diagram(src) => render(Layout::Diagram(src), params),
        };
        let mut out = free_images(&old.doc);
        out.push_str(&doc.preamble);
        write_out(&out)?;

        let mut view = View::new(doc, old.kind.clone());
        view.zoom = old.zoom;
        view.diagram = old.diagram;
        view.top = view
            .follow_line(old, old.top)
            .min(view.total().saturating_sub(rows));
        view.pan = old.pan.min(view.doc.max_pan());
        view.marks = old
            .marks
            .iter()
            .map(|(&c, &line)| (c, view.follow_line(old, line)))
            .collect();
        view.current_hit = old.current_hit.map(|line| view.follow_line(old, line));
        view.modified = path.as_deref().and_then(modified);
        self.views[depth] = view;
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

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
