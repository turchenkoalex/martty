use std::borrow::Cow;
use std::fmt::Write as _;

use unicode_width::UnicodeWidthStr;

use crate::style::Style;

#[derive(Clone, Debug)]
pub(super) struct Span {
    pub(super) text: String,
    pub(super) style: Style,
}

impl Span {
    /// Control characters in `text` are shown as symbols (`␛`), so a
    /// document can't send the terminal escape sequences of its own: set
    /// the clipboard (OSC 52), retitle the window, fake a link. Newlines and
    /// tabs stay for wrapping to handle.
    pub(super) fn new(text: impl Into<String>, style: Style) -> Self {
        let text = text.into();
        let text = match visible_controls(&text, true) {
            Cow::Borrowed(_) => text,
            Cow::Owned(shown) => shown,
        };
        Self { text, style }
    }

    pub(super) fn width(&self) -> usize {
        self.text.width()
    }
}

/// `text` with control characters replaced by visible ones: C0 by their
/// Control Pictures (`␛` for ESC), DEL by `␡`, C1 by `�`. `keep_breaks`
/// leaves newlines and tabs alone.
pub fn visible_controls(text: &str, keep_breaks: bool) -> Cow<'_, str> {
    let hidden = |c: char| c.is_control() && !(keep_breaks && matches!(c, '\n' | '\t'));
    if !text.contains(hidden) {
        return Cow::Borrowed(text);
    }
    let shown = text
        .chars()
        .map(|c| match c {
            _ if !hidden(c) => c,
            '\0'..='\x1f' => char::from_u32(0x2400 + u32::from(c)).unwrap_or('\u{fffd}'),
            '\x7f' => '\u{2421}',
            _ => '\u{fffd}',
        })
        .collect();
    Cow::Owned(shown)
}

pub(super) fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// Writes spans of one line, sending only the SGR and OSC 8 changes between
/// them. Each line starts and ends with the default style, so the pager can
/// draw any line on its own.
#[derive(Default)]
pub(super) struct Pen {
    /// Style and link the terminal has right now.
    current: Style,
    /// `MARTTY_NO_COLOR`: colors are dropped, other attributes kept.
    no_color: bool,
}

impl Pen {
    pub(super) fn new(color: bool) -> Self {
        Self {
            current: Style::default(),
            no_color: !color,
        }
    }

    /// `scale` wraps the text in OSC 66 (text sizing).
    pub(super) fn write(&mut self, out: &mut String, span: &Span, scale: Option<u8>) {
        let style = if self.no_color {
            span.style.without_colors()
        } else {
            span.style.clone()
        };
        style.write_sgr_from(&self.current, out);
        if style.link != self.current.link {
            if self.current.link.is_some() {
                out.push_str("\x1b]8;;\x1b\\");
            }
            if let Some(url) = &style.link {
                let _ = write!(out, "\x1b]8;;{url}\x1b\\");
            }
        }
        match scale {
            Some(s) => {
                let _ = write!(out, "\x1b]66;s={s};{}\x07", span.text);
            }
            None => out.push_str(&span.text),
        }
        self.current = style;
    }

    /// Closes an open link and resets the style, if anything is active.
    pub(super) fn finish_line(&mut self, out: &mut String) {
        if self.current.link.is_some() {
            out.push_str("\x1b]8;;\x1b\\");
        }
        if self.current != Style::default() {
            out.push_str("\x1b[0m");
        }
        self.current = Style::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_should_show_control_characters() {
        let span = Span::new("a\x1b]52;c;eA==\x07b\x7f\u{9b}\n", Style::default());
        assert_eq!(span.text, "a\u{241b}]52;c;eA==\u{2407}b\u{2421}\u{fffd}\n");
    }

    #[test]
    fn visible_controls_should_borrow_plain_text() {
        assert!(matches!(visible_controls("plain", false), Cow::Borrowed(_)));
        assert_eq!(visible_controls("a\nb", false), "a\u{240a}b");
    }
}
