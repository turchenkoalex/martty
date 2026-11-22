use std::fmt::Write as _;

use unicode_width::UnicodeWidthStr;

use crate::style::Style;

#[derive(Clone, Debug)]
pub(super) struct Span {
    pub(super) text: String,
    pub(super) style: Style,
}

impl Span {
    pub(super) fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub(super) fn width(&self) -> usize {
        self.text.width()
    }
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
