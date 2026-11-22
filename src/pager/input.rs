use super::keys::Key;
use super::{Action, Pager};

pub(super) const ZOOM_STEP: f32 = 1.25;
pub(super) const ZOOM_RANGE: (f32, f32) = (0.25, 8.0);

impl Pager<'_> {
    pub(super) fn scroll(&mut self, delta: isize, view_rows: usize) {
        let view = self.view_mut();
        let max_top = view.total().saturating_sub(view_rows);
        view.top = view.top.saturating_add_signed(delta).min(max_top);
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
    pub(super) fn visible_diagram(&self) -> Option<String> {
        let view = self.view();
        (view.top..view.top + self.view_rows())
            .find_map(|i| view.diagram_at(i))
            .map(str::to_owned)
    }

    pub(super) fn handle(&mut self, key: Key) -> Action {
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
        let fullscreen = self.view().is_diagram();
        match key {
            Key::Char('q' | 'Q') | Key::Esc if fullscreen => return Action::Close,
            Key::Esc if self.view().selected.is_some() => self.view_mut().selected = None,
            Key::Backspace | Key::Char('H') if self.views.len() > 1 => return Action::Close,
            Key::Tab => self.move_link(true, rows),
            Key::BackTab => self.move_link(false, rows),
            Key::Enter if let Some(url) = self.selected_on_screen() => return Action::Follow(url),
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

    /// Jumps so the next (or previous) matching line is at the top.
    /// Smart case: the query is case-sensitive only if it has uppercase.
    pub(super) fn find(&mut self, forward: bool) {
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
}
