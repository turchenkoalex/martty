use super::keys::Key;
use super::overlay::Overlay;
use super::{Action, Pager};

pub(super) const ZOOM_STEP: f32 = 1.25;
pub(super) const ZOOM_RANGE: (f32, f32) = (0.25, 8.0);

impl Pager<'_> {
    /// Scrolls by `delta` lines, and further in the same direction while a
    /// heading drawn as an image would be cut by the top or bottom edge, so
    /// a two-row heading passes in one step.
    pub(super) fn scroll(&mut self, delta: isize, view_rows: usize) {
        let view = self.view_mut();
        let max_top = view.total().saturating_sub(view_rows);
        let mut top = view.top.saturating_add_signed(delta).min(max_top);
        if delta != 0 {
            // Each edge needs at most one extra line.
            for _ in 0..2 {
                if !view.cuts_heading(top, view_rows) {
                    break;
                }
                let next = if delta > 0 {
                    (top + 1).min(max_top)
                } else {
                    top.saturating_sub(1)
                };
                if next == top {
                    break;
                }
                top = next;
            }
        }
        view.top = top;
    }

    pub(super) fn zoom_by(&mut self, factor: f32) -> Action {
        let view = self.view_mut();
        let zoom = (view.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        if (zoom - view.zoom).abs() < f32::EPSILON {
            return Action::Continue;
        }
        view.zoom = zoom;
        Action::Relayout
    }

    pub(super) fn pan_by(&mut self, delta: i64) {
        let view = self.view_mut();
        let max = i64::from(view.doc.max_pan());
        view.pan = u32::try_from((i64::from(view.pan) + delta).clamp(0, max)).unwrap_or(0);
    }

    /// The first diagram whose rows are on screen.
    pub(super) fn visible_diagram(&self) -> Option<usize> {
        let view = self.view();
        (view.top..view.top + self.view_rows()).find_map(|i| view.diagram_at(i))
    }

    pub(super) fn handle(&mut self, key: Key) -> Action {
        self.message = None;
        if self.prompt.is_some() {
            self.handle_prompt(key);
            return Action::Continue;
        }
        if self.overlay.is_some() {
            return self.handle_overlay(&key);
        }
        if let Some(kind) = self.pending.take() {
            self.handle_mark(kind, &key);
            return Action::Continue;
        }
        let fullscreen = self.view().is_diagram();
        match key {
            Key::Char('q' | 'Q' | 'v' | ' ') | Key::Esc if fullscreen => Action::Close,
            Key::Left | Key::Right if fullscreen => Action::Switch(key == Key::Right),
            Key::Esc if self.view().selected.is_some() => {
                self.view_mut().selected = None;
                Action::Continue
            }
            Key::Esc if self.query.is_some() => {
                self.query = None;
                Action::Continue
            }
            Key::Backspace | Key::Char('H') if self.views.len() > 1 => Action::Close,
            Key::Enter if let Some(url) = self.selected_on_screen() => Action::Follow(url),
            Key::Char('q' | 'Q') | Key::Ctrl(3) => Action::Quit,
            Key::Char('+' | '=') => self.zoom_by(ZOOM_STEP),
            Key::Char('-' | '_') => self.zoom_by(1.0 / ZOOM_STEP),
            Key::Char('0') => {
                let zoom = self.view().zoom;
                self.zoom_by(1.0 / zoom)
            }
            Key::Char('v') if !fullscreen => match self.visible_diagram() {
                Some(index) => Action::Open(index),
                None => {
                    self.message = Some("No diagram on screen".into());
                    Action::Continue
                }
            },
            Key::Char('w') => {
                self.overflow = !self.overflow;
                self.message = Some(if self.overflow {
                    "Wide code and tables: pan with h/l".into()
                } else {
                    "Wide code and tables: wrapped".into()
                });
                Action::Reflow
            }
            Key::Char('y') => self.copy_code().map_or(Action::Continue, Action::Copy),
            Key::Char('c') => Action::Copy(self.copy_screen(false)),
            Key::Char('C') => Action::Copy(self.copy_screen(true)),
            key => {
                self.handle_move(&key);
                Action::Continue
            }
        }
    }

    fn handle_prompt(&mut self, key: Key) {
        let Some(input) = &mut self.prompt else {
            return;
        };
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
    }

    /// The letter after `m` (set a mark) or `'` (go to it).
    fn handle_mark(&mut self, kind: char, key: &Key) {
        let &Key::Char(c) = key else {
            return;
        };
        let top = self.view().top;
        if kind == 'm' {
            if c.is_ascii_alphabetic() {
                self.view_mut().marks.insert(c, top);
                self.message = Some(format!("Mark {c} set"));
            } else {
                self.message = Some("Marks are letters a-z, A-Z".into());
            }
            return;
        }
        match self.view().marks.get(&c).copied() {
            Some(line) => self.jump(line),
            None => self.message = Some(format!("No mark {c}")),
        }
    }

    /// Scrolling, panning, search, links, marks and the other keys that
    /// only move around the view.
    fn handle_move(&mut self, key: &Key) {
        let rows = self.view_rows();
        let page = rows as isize;
        let step = i64::try_from(self.size.cols / 4).unwrap_or(8).max(8);
        match key {
            Key::Tab => self.move_link(true, rows),
            Key::BackTab => self.move_link(false, rows),
            Key::Char('h') | Key::Left => self.pan_by(-step),
            Key::Char('l') | Key::Right => self.pan_by(step),
            Key::Char('j' | 'e') | Key::Down | Key::Enter | Key::Ctrl(14) => self.scroll(1, rows),
            Key::Char('k') | Key::Up | Key::Ctrl(16) => self.scroll(-1, rows),
            // `z` pages down as in less.
            Key::Char(' ' | 'f' | 'z') | Key::PageDown | Key::Ctrl(6) => self.scroll(page, rows),
            Key::Char('b') | Key::PageUp | Key::Ctrl(2) => self.scroll(-page, rows),
            Key::Char('d') | Key::Ctrl(4) => self.scroll(page / 2, rows),
            Key::Char('u') | Key::Ctrl(21) => self.scroll(-page / 2, rows),
            Key::Char('g' | '<') | Key::Home => self.jump(0),
            Key::Char('G' | '>') | Key::End => self.jump(usize::MAX),
            Key::Char('[') => self.heading_step(false),
            Key::Char(']') => self.heading_step(true),
            Key::Char('t') => self.open_toc(),
            Key::Char('?') => self.overlay = Some(Overlay::Help),
            Key::Char(c @ ('m' | '\'')) => self.pending = Some(*c),
            Key::Char('/') => self.prompt = Some(String::new()),
            Key::Char('n') => self.find(true),
            Key::Char('N') => self.find(false),
            Key::Char('F') => self.toggle_follow(),
            _ => {}
        }
    }

    fn heading_step(&mut self, forward: bool) {
        let view = self.view();
        let mut lines = view.doc.headings.iter().map(|h| h.line);
        let found = if forward {
            lines.find(|&l| l > view.top)
        } else {
            lines.rev().find(|&l| l < view.top)
        };
        match found {
            Some(line) => self.jump(line),
            None => self.message = Some("No more headings".into()),
        }
    }

    fn toggle_follow(&mut self) {
        if !(0..self.views.len()).any(|d| self.path(d).is_some()) {
            self.message = Some("Nothing to follow: the text came from stdin".into());
            return;
        }
        self.follow = !self.follow;
        self.message = Some(if self.follow {
            "Following changes to the file".into()
        } else {
            "Stopped following the file".into()
        });
    }

    /// Selects the next (or previous) link, starting from the screen when
    /// nothing on it is selected, and scrolls it into view.
    pub(super) fn move_link(&mut self, forward: bool, rows: usize) {
        let view = self.view();
        let (top, bottom) = (view.top, view.top + rows);
        let current = self.selected_index_on_screen();
        let next = match current {
            Some(i) if forward => Some(i + 1).filter(|&n| n < view.links.len()),
            Some(i) => i.checked_sub(1),
            None if forward => view.links.iter().position(|l| l.first_line() >= top),
            None => view.links.iter().rposition(|l| l.first_line() < bottom),
        };
        let Some((next, link)) = next.and_then(|n| Some((n, view.links.get(n)?))) else {
            let none = view.links.is_empty();
            self.message = Some(if none { "No links" } else { "No more links" }.into());
            return;
        };
        let (first, last) = (link.first_line(), link.last_line());
        self.view_mut().selected = Some(next);
        if first < top {
            self.scroll(-isize::try_from(top - first).unwrap_or(isize::MAX), rows);
        } else if last >= bottom {
            self.scroll(
                isize::try_from(last + 1 - bottom).unwrap_or(isize::MAX),
                rows,
            );
        }
    }

    fn selected_index_on_screen(&self) -> Option<usize> {
        let view = self.view();
        let bottom = view.top + self.view_rows();
        let i = view.selected?;
        let link = view.links.get(i)?;
        (link.last_line() >= view.top && link.first_line() < bottom).then_some(i)
    }

    /// URL of the selected link, when it is on screen.
    pub(super) fn selected_on_screen(&self) -> Option<String> {
        let i = self.selected_index_on_screen()?;
        Some(self.view().links.get(i)?.url.clone())
    }
}
