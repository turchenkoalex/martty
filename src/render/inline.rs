use super::span::Span;
use super::{ACCENT, BULLETS, Renderer};
use crate::style::{Color, Style};

impl Renderer<'_> {
    pub(super) fn style(&self) -> Style {
        self.styles.last().cloned().unwrap_or_default()
    }

    pub(super) fn push_style(&mut self, f: impl FnOnce(&mut Style)) {
        let mut style = self.style();
        f(&mut style);
        self.styles.push(style);
    }

    pub(super) fn push_text(&mut self, text: &str) {
        let style = self.style();
        self.inline.push(Span::new(text, style));
    }

    pub(super) fn push_code(&mut self, text: &str) {
        let mut style = self.style();
        match self.cfg.code_bg {
            Some(bg) => {
                style.bg = Some(bg);
                // NBSP padding sticks to neighbours, so `code`, never wraps
                // or leaves a gap before the comma.
                self.inline
                    .push(Span::new(format!("\u{a0}{text}\u{a0}"), style));
            }
            None => {
                style.fg = Some(Color::Ansi(3));
                self.inline.push(Span::new(text, style));
            }
        }
    }

    pub(super) fn task_marker(&mut self, done: bool) {
        let (mark, style) = if done {
            ("☑", Style::fg(Color::Ansi(2)))
        } else {
            ("☐", Style::dim())
        };
        let unordered = matches!(self.lists.last(), Some(None));
        match self.containers.last_mut() {
            Some(item) if unordered => item.first = vec![Span::new(format!("{mark} "), style)],
            _ => self.inline.push(Span::new(format!("{mark} "), style)),
        }
    }

    pub(super) fn next_marker(&mut self) -> Span {
        let depth = self.lists.len();
        match self.lists.last_mut() {
            Some(Some(list)) => {
                let marker = format!("{:>w$}. ", list.next, w = list.width);
                list.next += 1;
                Span::new(marker, Style::fg(ACCENT))
            }
            _ => {
                let bullet = BULLETS[depth.saturating_sub(1) % BULLETS.len()];
                Span::new(format!("{bullet} "), Style::fg(ACCENT))
            }
        }
    }
}
