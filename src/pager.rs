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

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;

use unicode_width::UnicodeWidthChar;

use crate::kitty;
use crate::render::Document;
use crate::term::{self, RawMode, Size};

const ENTER: &str = "\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[?1000h\x1b[?1006h";
const LEAVE: &str = "\x1b[?1006l\x1b[?1000l\x1b[?7h\x1b[?25h\x1b[?1049l";
const POLL_MS: i32 = 200;
const ZOOM_STEP: f32 = 1.25;
const ZOOM_RANGE: (f32, f32) = (0.25, 8.0);
const WHEEL_LINES: isize = 3;

/// What to lay out: the whole document or one diagram full screen.
pub enum Layout<'a> {
    Document,
    Diagram(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Char(char),
    Ctrl(u8),
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Backspace,
    Esc,
    /// SGR mouse event; `col`/`row` are 1-based.
    Mouse {
        button: u16,
        col: u16,
        row: u16,
        press: bool,
    },
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
/// single diagram for a screen size and diagram zoom.
pub fn run(
    title: &str,
    doc: Document,
    mut render: impl FnMut(Layout<'_>, Size, f32) -> Document,
) -> io::Result<()> {
    let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let _raw = RawMode::keys(tty.as_raw_fd())?;
    let mut pager = Pager::new(title, term::size(), doc);
    pager.enter()?;

    loop {
        pager.redraw()?;
        let Some(keys) = read_keys(&mut tty)? else {
            let size = term::size();
            if (size.cols, size.rows) != (pager.size.cols, pager.size.rows) {
                pager.size = size;
                for depth in 0..pager.views.len() {
                    pager.relayout(depth, &mut render)?;
                }
            }
            continue;
        };
        for key in keys {
            match pager.handle(key) {
                Action::Continue => {}
                Action::Quit => return Ok(()),
                Action::Relayout => pager.relayout(pager.views.len() - 1, &mut render)?,
                Action::Open(source) => {
                    let doc = render(Layout::Diagram(&source), pager.size, 1.0);
                    pager.push(View::new(doc, Some(source)))?;
                }
                Action::Close => pager.pop()?,
            }
        }
    }
}

/// One screenful of content with its own scroll, pan and zoom.
struct View {
    doc: Document,
    /// Text of each line for searching (images contribute nothing).
    plain: Vec<String>,
    top: usize,
    /// Horizontal shift of overflowing images, in cells.
    pan: u32,
    /// Diagram zoom; 1.0 fits the screen.
    zoom: f32,
    /// Set for a full-screen diagram.
    source: Option<String>,
}

impl View {
    fn new(doc: Document, source: Option<String>) -> Self {
        let plain = doc.lines.iter().map(|l| plain_text(&l.text)).collect();
        Self {
            doc,
            plain,
            top: 0,
            pan: 0,
            zoom: 1.0,
            source,
        }
    }

    fn total(&self) -> usize {
        self.doc.lines.len()
    }

    /// Diagram source of the image on line `index`, if it is a diagram.
    fn diagram_at(&self, index: usize) -> Option<&str> {
        let id = self.doc.lines.get(index)?.image?.id;
        self.doc
            .diagrams
            .iter()
            .find(|(d, _)| *d == id)
            .map(|(_, src)| src.as_str())
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

    // ── terminal I/O ─────────────────────────────────────────────────────

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

    // ── input ────────────────────────────────────────────────────────────

    fn scroll(&mut self, delta: isize, view_rows: usize) {
        let view = self.view_mut();
        let max_top = view.total().saturating_sub(view_rows);
        view.top = view.top.saturating_add_signed(delta).min(max_top);
    }

    fn zoom_by(&mut self, factor: f32) -> Action {
        let view = self.view_mut();
        let zoom = (view.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        if (zoom - view.zoom).abs() < f32::EPSILON {
            return Action::Continue;
        }
        view.zoom = zoom;
        Action::Relayout
    }

    fn pan_by(&mut self, delta: i64) {
        let view = self.view_mut();
        let max = i64::from(view.doc.max_pan());
        view.pan = u32::try_from((i64::from(view.pan) + delta).clamp(0, max)).unwrap_or(0);
    }

    /// The first diagram whose rows are on screen.
    fn visible_diagram(&self) -> Option<String> {
        let view = self.view();
        (view.top..view.top + self.view_rows())
            .find_map(|i| view.diagram_at(i))
            .map(str::to_owned)
    }

    fn handle(&mut self, key: Key) -> Action {
        self.message = None;
        if let Some(input) = &mut self.prompt {
            match key {
                Key::Enter => {
                    let query = std::mem::take(input);
                    self.prompt = None;
                    if !query.is_empty() {
                        self.query = Some(query);
                    }
                    self.find(true);
                }
                Key::Esc | Key::Ctrl(3) => self.prompt = None,
                Key::Backspace => {
                    if input.pop().is_none() {
                        self.prompt = None;
                    }
                }
                Key::Char(c) => input.push(c),
                _ => {}
            }
            return Action::Continue;
        }

        let rows = self.view_rows();
        let page = rows as isize;
        let step = i64::try_from(self.size.cols / 4).unwrap_or(8).max(8);
        let fullscreen = self.views.len() > 1;
        match key {
            Key::Char('q' | 'Q') | Key::Esc if fullscreen => return Action::Close,
            Key::Char('q' | 'Q') | Key::Ctrl(3) => return Action::Quit,
            Key::Char('+' | '=') => return self.zoom_by(ZOOM_STEP),
            Key::Char('-' | '_') => return self.zoom_by(1.0 / ZOOM_STEP),
            Key::Char('0') => {
                let zoom = self.view().zoom;
                return self.zoom_by(1.0 / zoom);
            }
            Key::Char('z') if !fullscreen => {
                return match self.visible_diagram() {
                    Some(src) => Action::Open(src),
                    None => {
                        self.message = Some("No diagram on screen".into());
                        Action::Continue
                    }
                };
            }
            Key::Mouse { .. } => return self.mouse(&key, rows),
            Key::Char('h') | Key::Left => self.pan_by(-step),
            Key::Char('l') | Key::Right => self.pan_by(step),
            Key::Char('j' | 'e') | Key::Down | Key::Enter | Key::Ctrl(14) => self.scroll(1, rows),
            Key::Char('k' | 'y') | Key::Up | Key::Ctrl(16) => self.scroll(-1, rows),
            Key::Char(' ' | 'f') | Key::PageDown | Key::Ctrl(6) => self.scroll(page, rows),
            Key::Char('b') | Key::PageUp | Key::Ctrl(2) => self.scroll(-page, rows),
            Key::Char('d') | Key::Ctrl(4) => self.scroll(page / 2, rows),
            Key::Char('u') | Key::Ctrl(21) => self.scroll(-page / 2, rows),
            Key::Char('g' | '<') | Key::Home => self.view_mut().top = 0,
            Key::Char('G' | '>') | Key::End => self.scroll(isize::MAX, rows),
            Key::Char('/') => self.prompt = Some(String::new()),
            Key::Char('n') => self.find(true),
            Key::Char('N') => self.find(false),
            _ => {}
        }
        Action::Continue
    }

    fn mouse(&mut self, key: &Key, rows: usize) -> Action {
        let &Key::Mouse {
            button, row, press, ..
        } = key
        else {
            return Action::Continue;
        };
        // Bits 2–4 are Shift/Meta/Ctrl, bit 5 is motion: ignore modifiers.
        match button & !0b1_1100 {
            64 => self.scroll(-WHEEL_LINES, rows),
            65 => self.scroll(WHEEL_LINES, rows),
            66 => self.pan_by(-8),
            67 => self.pan_by(8),
            0 if press && self.views.len() > 1 => return Action::Close,
            0 if press => {
                let (_, top_margin) = self.margins();
                let line = (usize::from(row).saturating_sub(1 + top_margin)) + self.view().top;
                if let Some(src) = self.view().diagram_at(line) {
                    return Action::Open(src.to_owned());
                }
            }
            _ => {}
        }
        Action::Continue
    }

    /// Jumps so the next (or previous) matching line is at the top.
    /// Smart case: the query is case-sensitive only if it has uppercase.
    fn find(&mut self, forward: bool) {
        let Some(query) = self.query.clone() else {
            return;
        };
        let sensitive = query.chars().any(char::is_uppercase);
        let needle = if sensitive {
            query.clone()
        } else {
            query.to_lowercase()
        };
        let view = self.view();
        let hit = |i: &usize| {
            let line = &view.plain[*i];
            if sensitive {
                line.contains(&needle)
            } else {
                line.to_lowercase().contains(&needle)
            }
        };
        let found = if forward {
            (view.top + 1..view.total()).find(hit)
        } else {
            (0..view.top).rev().find(hit)
        };
        match found {
            Some(i) => {
                let rows = self.view_rows();
                self.view_mut().top = 0;
                self.scroll(isize::try_from(i).unwrap_or(isize::MAX), rows);
            }
            None => self.message = Some(format!("Pattern not found: {query}")),
        }
    }

    // ── drawing ──────────────────────────────────────────────────────────

    /// Left and top margins that centre a full-screen diagram.
    fn margins(&self) -> (usize, usize) {
        let view = self.view();
        if view.source.is_none() {
            return (0, 0);
        }
        let width = view
            .doc
            .lines
            .iter()
            .filter_map(|l| l.image.map(|i| i.visible as usize))
            .max()
            .unwrap_or(0);
        (
            self.size.cols.saturating_sub(width) / 2,
            self.view_rows().saturating_sub(view.total()) / 2,
        )
    }

    fn draw(&self, out: &mut String) {
        let view = self.view();
        let (left, top_margin) = self.margins();
        out.push_str("\x1b[?2026h");
        for row in 0..self.view_rows() {
            let _ = write!(out, "\x1b[{};1H\x1b[0m", row + 1);
            let line = row
                .checked_sub(top_margin)
                .and_then(|r| view.doc.lines.get(view.top + r));
            if let Some(line) = line {
                let _ = write!(out, "{:left$}", "");
                out.push_str(&line.panned(view.pan));
            }
            out.push_str("\x1b[0m\x1b[K");
        }

        let _ = write!(
            out,
            "\x1b[{};1H\x1b[0;7m{}\x1b[0m\x1b[K",
            self.size.rows.max(1),
            truncate(&self.status(), self.size.cols)
        );
        out.push_str(if self.prompt.is_some() {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });
        out.push_str("\x1b[?2026l");
    }

    fn status(&self) -> String {
        if let Some(input) = &self.prompt {
            return format!("/{input}");
        }
        if let Some(msg) = &self.message {
            return msg.clone();
        }
        let view = self.view();
        let mut extra = String::new();
        if (view.zoom - 1.0).abs() > f32::EPSILON {
            let _ = write!(extra, "  zoom {:.0}%", view.zoom * 100.0);
        }
        if view.doc.max_pan() > 0 {
            let _ = write!(extra, "  ←{}/{}→", view.pan, view.doc.max_pan());
        }
        if view.source.is_some() {
            return format!(
                " {} › diagram{extra}  · q/Esc/click back · +/- zoom · hjkl move",
                self.title
            );
        }
        let total = view.total();
        let last = (view.top + self.view_rows()).min(total);
        let pct = (last * 100).checked_div(total).unwrap_or(100);
        format!(
            " {}  {}-{}/{}  {pct}%{extra}  · q quit · / search · click/z diagram · +/- zoom",
            self.title,
            (view.top + 1).min(total),
            last,
            total
        )
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

/// Waits up to `POLL_MS` for input. `None` on timeout.
fn read_keys(tty: &mut File) -> io::Result<Option<Vec<Key>>> {
    let mut pfd = libc::pollfd {
        fd: tty.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: a single valid pollfd.
    if unsafe { libc::poll(&mut pfd, 1, POLL_MS) } <= 0 {
        return Ok(None);
    }
    let mut buf = [0u8; 1024];
    let n = tty.read(&mut buf)?;
    Ok(Some(parse_keys(&buf[..n])))
}

fn parse_keys(buf: &[u8]) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        i += 1;
        let key = match b {
            0x1b => match buf.get(i) {
                Some(b'[' | b'O') => {
                    let start = i + 1;
                    let len = buf[start..]
                        .iter()
                        .position(|c| (0x40..=0x7e).contains(c))
                        .unwrap_or(buf.len() - start);
                    let params = &buf[start..start + len];
                    let fin = buf.get(start + len).copied();
                    i = (start + len + 1).min(buf.len());
                    match (fin, params) {
                        (Some(f @ (b'M' | b'm')), [b'<', rest @ ..]) => match sgr_mouse(rest) {
                            Some((button, col, row)) => Key::Mouse {
                                button,
                                col,
                                row,
                                press: f == b'M',
                            },
                            None => continue,
                        },
                        (Some(b'A'), _) => Key::Up,
                        (Some(b'B'), _) => Key::Down,
                        (Some(b'C'), _) => Key::Right,
                        (Some(b'D'), _) => Key::Left,
                        (Some(b'H'), _) => Key::Home,
                        (Some(b'F'), _) => Key::End,
                        (Some(b'~'), b"5") => Key::PageUp,
                        (Some(b'~'), b"6") => Key::PageDown,
                        (Some(b'~'), b"1" | b"7") => Key::Home,
                        (Some(b'~'), b"4" | b"8") => Key::End,
                        _ => continue,
                    }
                }
                _ => Key::Esc,
            },
            b'\r' | b'\n' => Key::Enter,
            0x7f | 0x08 => Key::Backspace,
            0..0x20 => Key::Ctrl(b),
            _ => {
                let len = match b {
                    0xf0.. => 4,
                    0xe0.. => 3,
                    0xc0.. => 2,
                    _ => 1,
                };
                let end = (i - 1 + len).min(buf.len());
                let ch = std::str::from_utf8(&buf[i - 1..end])
                    .ok()
                    .and_then(|s| s.chars().next());
                i = end;
                match ch {
                    Some(c) => Key::Char(c),
                    None => continue,
                }
            }
        };
        keys.push(key);
    }
    keys
}

/// `button;col;row` from an SGR mouse report.
fn sgr_mouse(params: &[u8]) -> Option<(u16, u16, u16)> {
    let s = std::str::from_utf8(params).ok()?;
    let mut it = s.split(';').map(str::parse::<u16>);
    Some((it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?))
}

/// Visible text of a rendered line: escape sequences removed, image
/// placeholder lines reduced to nothing.
pub fn plain_text(line: &str) -> String {
    if line.contains('\u{10EEEE}') {
        return String::new();
    }
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']' | '_') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn truncate(s: &str, cols: usize) -> String {
    let mut used = 0;
    s.chars()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= cols
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{ImageRow, Line};

    fn text_doc(lines: &[&str]) -> Document {
        Document {
            preamble: String::new(),
            lines: lines
                .iter()
                .map(|t| Line {
                    text: (*t).to_owned(),
                    image: None,
                })
                .collect(),
            image_ids: Vec::new(),
            diagrams: Vec::new(),
        }
    }

    fn pager_with(doc: Document, rows: usize) -> Pager<'static> {
        let size = Size {
            cols: 80,
            rows,
            cell: None,
        };
        Pager::new("t", size, doc)
    }

    fn pager(lines: usize, rows: usize) -> Pager<'static> {
        let text: Vec<String> = (0..lines).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = text.iter().map(String::as_str).collect();
        pager_with(text_doc(&refs), rows)
    }

    /// A document whose line 5 is the only row of diagram image 9.
    fn diagram_doc() -> Document {
        let mut doc = text_doc(&["a"; 20]);
        doc.lines[5].image = Some(ImageRow {
            id: 9,
            row: 0,
            cols: 10,
            visible: 10,
            prefix_len: 0,
        });
        doc.diagrams.push((9, "graph LR\n A --> B".into()));
        doc
    }

    #[test]
    fn parse_keys_should_read_arrows_and_pages() {
        let keys = parse_keys(b"\x1b[A\x1bOB\x1b[5~\x1b[6~");
        assert_eq!(keys, [Key::Up, Key::Down, Key::PageUp, Key::PageDown]);
    }

    #[test]
    fn parse_keys_should_read_utf8_chars() {
        let keys = parse_keys("q/ж".as_bytes());
        assert_eq!(keys, [Key::Char('q'), Key::Char('/'), Key::Char('ж')]);
    }

    #[test]
    fn parse_keys_should_read_sgr_mouse_press() {
        let keys = parse_keys(b"\x1b[<0;12;5M");
        let expected = Key::Mouse {
            button: 0,
            col: 12,
            row: 5,
            press: true,
        };
        assert_eq!(keys, [expected]);
    }

    #[test]
    fn scroll_should_stop_at_last_page() {
        let mut p = pager(100, 11);
        p.handle(Key::End);
        assert_eq!(p.view().top, 90);
    }

    #[test]
    fn scroll_should_not_go_above_top() {
        let mut p = pager(100, 11);
        p.handle(Key::PageUp);
        assert_eq!(p.view().top, 0);
    }

    #[test]
    fn wheel_should_scroll_three_lines() {
        let mut p = pager(100, 11);
        p.handle(parse_keys(b"\x1b[<65;1;1M").remove(0));
        assert_eq!(p.view().top, 3);
    }

    #[test]
    fn search_should_jump_to_match_smart_case() {
        let mut doc = text_doc(&["x"; 100]);
        doc.lines[42].text = "Hello World".into();
        let mut p = pager_with(doc, 11);
        for key in parse_keys(b"/world\r") {
            p.handle(key);
        }
        assert_eq!(p.view().top, 42);
    }

    #[test]
    fn quit_should_stop_pager() {
        assert_eq!(pager(1, 10).handle(Key::Char('q')), Action::Quit);
    }

    #[test]
    fn zoom_in_should_request_relayout() {
        assert_eq!(pager(1, 10).handle(Key::Char('+')), Action::Relayout);
    }

    #[test]
    fn click_on_diagram_should_open_it() {
        let mut p = pager_with(diagram_doc(), 11);
        let action = p.handle(parse_keys(b"\x1b[<0;3;6M").remove(0));
        assert_eq!(action, Action::Open("graph LR\n A --> B".into()));
    }

    #[test]
    fn click_on_text_should_do_nothing() {
        let mut p = pager_with(diagram_doc(), 11);
        let action = p.handle(parse_keys(b"\x1b[<0;3;2M").remove(0));
        assert_eq!(action, Action::Continue);
    }

    #[test]
    fn z_should_open_visible_diagram() {
        let mut p = pager_with(diagram_doc(), 11);
        assert!(matches!(p.handle(Key::Char('z')), Action::Open(_)));
    }

    #[test]
    fn esc_should_close_fullscreen_diagram() {
        let mut p = pager_with(diagram_doc(), 11);
        p.views
            .push(View::new(text_doc(&["d"]), Some("src".into())));
        assert_eq!(p.handle(Key::Esc), Action::Close);
    }

    #[test]
    fn pan_should_stop_at_max_pan() {
        let mut doc = diagram_doc();
        if let Some(img) = &mut doc.lines[5].image {
            img.cols = 30;
        }
        let mut p = pager_with(doc, 11);
        for _ in 0..10 {
            p.handle(Key::Right);
        }
        assert_eq!(p.view().pan, 20);
    }

    #[test]
    fn plain_text_should_strip_sgr_and_hyperlinks() {
        let line = "\x1b[0;1mbold\x1b[0m \x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\";
        assert_eq!(plain_text(line), "bold link");
    }
}
