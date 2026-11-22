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

pub(super) fn write_span(out: &mut String, span: &Span, scale: Option<u8>) {
    span.style.write_sgr(out);
    if let Some(url) = &span.style.link {
        let _ = write!(out, "\x1b]8;;{url}\x1b\\");
    }
    match scale {
        Some(s) => {
            let _ = write!(out, "\x1b]66;s={s};{}\x07", span.text);
        }
        None => out.push_str(&span.text),
    }
    if span.style.link.is_some() {
        out.push_str("\x1b]8;;\x1b\\");
    }
}
