use std::mem;
use std::rc::Rc;

use super::emoji::replace_shortcodes;
use super::images::PendingImage;
use super::span::Span;
use super::{ACCENT, BULLETS, LINK, Renderer};
use crate::style::{Color, Style, Underline};

pub(super) fn link_style(style: &mut Style, url: &str) {
    style.link = Some(Rc::from(url));
    style.fg = Some(LINK);
    style.underline = Underline::Single;
    style.underline_color = Some(LINK);
}

/// Highlighted text, as `<mark>` or `==text==`.
pub(super) fn mark_style(style: &mut Style) {
    style.bg = Some(Color::Ansi(3));
    style.fg = Some(Color::Ansi(0));
}

impl Renderer<'_> {
    pub(super) fn style(&self) -> Style {
        self.styles.last().cloned().unwrap_or_default()
    }

    pub(super) fn push_style(&mut self, f: impl FnOnce(&mut Style)) {
        let mut style = self.style();
        f(&mut style);
        self.styles.push(style);
    }

    /// Text from the document: emoji shortcodes are replaced and `==text==`
    /// is highlighted. Only pairs within one text event count, and the
    /// marked text can't start or end with a space, so `a == b` stays as is.
    pub(super) fn push_prose(&mut self, text: &str) {
        let text = replace_shortcodes(text);
        let mut parts = text.split("==");
        let mut plain = parts.next().unwrap_or_default().to_owned();
        let rest: Vec<&str> = parts.collect();
        for pair in rest.chunks(2) {
            match pair {
                [inner, after] if !inner.is_empty() && inner.trim() == *inner => {
                    self.push_text(&mem::take(&mut plain));
                    let mut style = self.style();
                    mark_style(&mut style);
                    self.inline.push(Span::new(*inner, style));
                    plain.push_str(after);
                }
                _ => {
                    for part in pair {
                        plain.push_str("==");
                        plain.push_str(part);
                    }
                }
            }
        }
        self.push_text(&plain);
    }

    pub(super) fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let style = self.style();
        self.inline.push(Span::new(text, style));
    }

    /// Opens an image: its alt text follows, shown when the image can't be.
    pub(super) fn start_image(&mut self, url: &str) {
        let start = self.inline.len();
        self.images.push(PendingImage {
            url: url.to_owned(),
            alt: start..start,
        });
        self.push_style(|s| {
            s.link = Some(Rc::from(url));
            s.fg = Some(Color::Ansi(5));
            s.italic = true;
        });
        self.push_text("🖼 ");
    }

    pub(super) fn end_image_alt(&mut self) {
        let end = self.inline.len();
        if let Some(image) = self.images.last_mut() {
            image.alt.end = end;
        }
    }

    pub(super) fn code_style(&self, style: &mut Style) {
        match self.cfg.code_bg {
            Some(bg) => style.bg = Some(bg),
            None => style.fg = Some(Color::Ansi(3)),
        }
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
