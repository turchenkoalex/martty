//! The HTML that READMEs use in practice: line breaks, `<kbd>`, `<sub>` and
//! `<sup>`, `<details>`/`<summary>`, inline styling, links and images.
//! Other tags are dropped and their text kept; comments are hidden.

use super::ACCENT;
use super::Renderer;
use super::inline::{link_style, mark_style};
use super::span::Span;
use crate::style::{Style, Underline};

/// An inline tag that changed the style, so its end can restore it.
pub(super) struct OpenTag {
    name: String,
    /// `Renderer::styles` length before the tag.
    styles_len: usize,
    /// First of the tag's spans in `Renderer::inline`.
    inline_start: usize,
}

/// Block-level tags: each starts and ends a line.
const BLOCKS: &[&str] = &[
    "p",
    "div",
    "center",
    "details",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "table",
    "tr",
    "blockquote",
    "pre",
];

impl Renderer<'_> {
    /// `block` is set inside an HTML block, where newlines are just spaces.
    pub(super) fn html(&mut self, html: &str, block: bool) {
        let mut rest = html;
        loop {
            if self.in_html_comment {
                let Some(end) = rest.find("-->") else { return };
                rest = &rest[end + 3..];
                self.in_html_comment = false;
            }
            let Some(lt) = rest.find('<') else {
                self.html_text(rest, block);
                return;
            };
            self.html_text(&rest[..lt], block);
            rest = &rest[lt..];
            if let Some(after) = rest.strip_prefix("<!--") {
                self.in_html_comment = true;
                rest = after;
                continue;
            }
            let Some(gt) = rest.find('>') else {
                self.html_text(rest, block);
                return;
            };
            let tag = &rest[1..gt];
            rest = &rest[gt + 1..];
            self.html_tag(tag);
        }
    }

    /// Drops an HTML block that held only tags or comments, so it adds no
    /// blank line.
    pub(super) fn end_html_block(&mut self) {
        let empty = self.images.is_empty() && self.inline.iter().all(|s| s.text.trim().is_empty());
        if empty {
            self.inline.clear();
            return;
        }
        // Set aside while the blank line before the block goes out, images
        // too: flushed alone they would come before it, and their alt text
        // after them as a paragraph of its own.
        let spans = std::mem::take(&mut self.inline);
        let images = std::mem::take(&mut self.images);
        self.open_block();
        self.inline = spans;
        self.images = images;
        self.close_block();
    }

    /// Closes inline tags left open when their block ends.
    pub(super) fn reset_html(&mut self) {
        if let Some(first) = self.html_tags.first() {
            self.styles.truncate(first.styles_len.max(1));
        }
        self.html_tags.clear();
    }

    fn html_text(&mut self, text: &str, block: bool) {
        if text.is_empty() {
            return;
        }
        let mut text = decode_entities(text);
        if block {
            text = text.replace('\n', " ");
        }
        self.push_text(&text);
    }

    fn html_tag(&mut self, tag: &str) {
        let (closing, body) = match tag.strip_prefix('/') {
            Some(body) => (true, body),
            None => (false, tag),
        };
        if !body.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return;
        }
        let end = body
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(body.len());
        let name = body[..end].to_ascii_lowercase();
        let attrs = &body[end..];
        if closing {
            self.close_html(&name);
        } else {
            self.open_html(&name, attrs);
        }
    }

    fn open_html(&mut self, name: &str, attrs: &str) {
        let kbd_bg = self.cfg.code_bg;
        match name {
            "br" => self.push_text("\n"),
            "hr" => self.rule(),
            "summary" => {
                self.html_break();
                self.inline.push(Span::new("▾ ", Style::fg(ACCENT)));
                self.open_tag(name, |s| s.bold = true);
            }
            "b" | "strong" => self.open_tag(name, |s| s.bold = true),
            "i" | "em" | "cite" => self.open_tag(name, |s| s.italic = true),
            "s" | "del" | "strike" => self.open_tag(name, |s| s.strike = true),
            "u" | "ins" => self.open_tag(name, |s| s.underline = Underline::Single),
            "mark" => self.open_tag(name, mark_style),
            "sub" | "sup" => self.open_tag(name, |_| {}),
            "code" | "tt" => {
                let mut style = self.style();
                self.code_style(&mut style);
                self.open_tag(name, |s| *s = style);
            }
            // A key cap: the code background, or reverse video without one.
            "kbd" => {
                self.open_tag(name, |s| {
                    s.bold = true;
                    match kbd_bg {
                        Some(bg) => s.bg = Some(bg),
                        None => s.inverse = true,
                    }
                });
                self.push_text("\u{a0}");
            }
            "a" => match attr(attrs, "href") {
                Some(url) => {
                    let url = super::links::target(self.cfg, url);
                    self.open_tag(name, |s| link_style(s, &url));
                }
                None => self.open_tag(name, |_| {}),
            },
            "img" => {
                if let Some(src) = attr(attrs, "src") {
                    self.start_image(src);
                    self.push_text(&decode_entities(attr(attrs, "alt").unwrap_or("image")));
                    self.end_image_alt();
                    if self.styles.len() > 1 {
                        self.styles.pop();
                    }
                }
            }
            _ if BLOCKS.contains(&name) => {
                self.html_break();
                if name.starts_with('h') {
                    self.open_tag(name, |s| s.bold = true);
                }
            }
            _ => {}
        }
    }

    fn close_html(&mut self, name: &str) {
        let Some(i) = self.html_tags.iter().rposition(|t| t.name == name) else {
            if BLOCKS.contains(&name) {
                self.html_break();
            }
            return;
        };
        let tag = self.html_tags.drain(i..).next();
        let Some(tag) = tag else { return };
        match name {
            "kbd" => self.push_text("\u{a0}"),
            "sub" | "sup" => self.apply_script(tag.inline_start, name == "sup"),
            _ => {}
        }
        self.styles.truncate(tag.styles_len.max(1));
        if name == "summary" || BLOCKS.contains(&name) {
            self.html_break();
        }
    }

    fn open_tag(&mut self, name: &str, f: impl FnOnce(&mut Style)) {
        self.html_tags.push(OpenTag {
            name: name.to_owned(),
            styles_len: self.styles.len(),
            inline_start: self.inline.len(),
        });
        self.push_style(f);
    }

    /// Starts a new line, unless the current one is still empty.
    fn html_break(&mut self) {
        let at_line_start = self
            .inline
            .iter()
            .rev()
            .map(|s| s.text.trim_end_matches(' '))
            .find(|t| !t.is_empty())
            .is_none_or(|t| t.ends_with('\n'));
        if !at_line_start {
            self.push_text("\n");
        }
    }

    /// Turns the spans from `start` into Unicode super- or subscript
    /// characters, or dims them when some character has no such form.
    fn apply_script(&mut self, start: usize, sup: bool) {
        let Some(spans) = self.inline.get_mut(start..) else {
            return;
        };
        let converted: Option<Vec<String>> = spans
            .iter()
            .map(|s| s.text.chars().map(|c| script_char(c, sup)).collect())
            .collect();
        match converted {
            Some(texts) => {
                for (span, text) in spans.iter_mut().zip(texts) {
                    span.text = text;
                }
            }
            None => spans.iter_mut().for_each(|s| s.style.dim = true),
        }
    }
}

fn script_char(c: char, sup: bool) -> Option<char> {
    const SUP: &str = "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ᵃᵇᶜᵈᵉᶠᵍʰⁱʲᵏˡᵐⁿᵒᵖ\0ʳˢᵗᵘᵛʷˣʸᶻ";
    const SUB: &str = "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎ₐ\0\0\0ₑ\0\0ₕᵢⱼₖₗₘₙₒₚ\0ᵣₛₜᵤᵥ\0ₓ\0\0";
    const FROM: &str = "0123456789+-=()abcdefghijklmnopqrstuvwxyz";
    if c == ' ' {
        return Some(c);
    }
    let i = FROM.chars().position(|f| f == c)?;
    let table = if sup { SUP } else { SUB };
    table.chars().nth(i).filter(|&t| t != '\0')
}

/// Value of an attribute, quoted or not.
/// `src` of every `<img>` tag in a piece of HTML.
pub(super) fn img_sources(html: &str) -> Vec<&str> {
    let lower = html.to_ascii_lowercase();
    lower
        .match_indices("<img")
        .filter_map(|(at, _)| {
            let tag = &html[at + 4..];
            attr(&tag[..tag.find('>').unwrap_or(tag.len())], "src")
        })
        .collect()
}

fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].split_whitespace().last().unwrap_or("");
        let after = rest[eq + 1..].trim_start();
        let (value, tail) = match after.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let body = &after[1..];
                let end = body.find(q)?;
                (&body[..end], &body[end + 1..])
            }
            _ => {
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                (after[..end].trim_end_matches('/'), &after[end..])
            }
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
        rest = tail;
    }
    None
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.find(';').filter(|&semi| semi <= 10).and_then(|semi| {
            let name = &rest[1..semi];
            let c = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => '\u{a0}',
                "copy" => '©',
                "mdash" => '—',
                "ndash" => '–',
                _ => {
                    let num = name.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_should_read_quoted_and_bare_values() {
        let attrs = r#" alt='a cat' width=200 src="cat.png""#;
        assert_eq!(attr(attrs, "src"), Some("cat.png"));
        assert_eq!(attr(attrs, "alt"), Some("a cat"));
        assert_eq!(attr(attrs, "width"), Some("200"));
        assert_eq!(attr(attrs, "href"), None);
    }

    #[test]
    fn img_sources_should_find_every_image() {
        let html = r#"<p><IMG src="a.png"> and <img alt=x src='b.svg'/></p>"#;
        assert_eq!(img_sources(html), ["a.png", "b.svg"]);
    }

    #[test]
    fn decode_entities_should_keep_unknown_ones() {
        assert_eq!(
            decode_entities("a &amp; b &#x41; &bogus; &"),
            "a & b A &bogus; &"
        );
    }

    #[test]
    fn script_char_should_map_digits_and_skip_missing_letters() {
        assert_eq!(script_char('2', true), Some('²'));
        assert_eq!(script_char('2', false), Some('₂'));
        assert_eq!(script_char('q', true), None);
        assert_eq!(script_char('b', false), None);
    }
}
