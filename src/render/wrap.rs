use std::mem;

use unicode_width::UnicodeWidthChar;

use super::span::{Span, spans_width};

pub(super) enum Token {
    /// May span several styles, e.g. `**bold**ness,`.
    Word(Vec<Span>),
    Space(Span),
    Newline,
}

pub(super) fn tokenize(spans: &[Span]) -> Vec<Token> {
    #[derive(PartialEq)]
    enum Kind {
        Newline,
        Space,
        Text,
    }
    let kind = |c: char| match c {
        '\n' => Kind::Newline,
        ' ' | '\t' => Kind::Space,
        _ => Kind::Text,
    };

    let mut tokens = Vec::new();
    let mut word = Vec::new();
    for span in spans {
        let mut rest = span.text.as_str();
        while let Some(c) = rest.chars().next() {
            let k = kind(c);
            let end = match k {
                Kind::Newline => 1,
                _ => rest.find(|ch| kind(ch) != k).unwrap_or(rest.len()),
            };
            let (piece, tail) = rest.split_at(end);
            rest = tail;
            if k == Kind::Text {
                word.push(Span::new(piece, span.style.clone()));
                continue;
            }
            if !word.is_empty() {
                tokens.push(Token::Word(mem::take(&mut word)));
            }
            tokens.push(match k {
                Kind::Newline => Token::Newline,
                _ => Token::Space(Span::new(" ", span.style.clone())),
            });
        }
    }
    if !word.is_empty() {
        tokens.push(Token::Word(word));
    }
    tokens
}

/// Greedy word wrap by display width; overlong words are split by chars.
pub(super) fn wrap(spans: &[Span], width: usize) -> Vec<Vec<Span>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line: Vec<Span> = Vec::new();
    let mut used = 0;
    let mut space: Option<Span> = None;

    for token in tokenize(spans) {
        match token {
            Token::Newline => {
                lines.push(mem::take(&mut line));
                used = 0;
                space = None;
            }
            Token::Space(s) => {
                if used > 0 {
                    space = Some(s);
                }
            }
            Token::Word(parts) => {
                let word_width = spans_width(&parts);
                let gap = usize::from(space.is_some());
                if used > 0 && used + gap + word_width > width {
                    lines.push(mem::take(&mut line));
                    used = 0;
                    space = None;
                }
                if let Some(s) = space.take() {
                    line.push(s);
                    used += 1;
                }
                for part in parts {
                    if used + part.width() <= width {
                        used += part.width();
                        line.push(part);
                        continue;
                    }
                    let mut chunk = String::new();
                    for ch in part.text.chars() {
                        let cw = ch.width().unwrap_or(0);
                        if used + cw > width && used > 0 {
                            if !chunk.is_empty() {
                                line.push(Span::new(mem::take(&mut chunk), part.style.clone()));
                            }
                            lines.push(mem::take(&mut line));
                            used = 0;
                        }
                        chunk.push(ch);
                        used += cw;
                    }
                    if !chunk.is_empty() {
                        line.push(Span::new(chunk, part.style));
                    }
                }
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

pub(super) fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw > width && used > 0 {
            out.push(String::new());
            used = 0;
        }
        if let Some(last) = out.last_mut() {
            last.push(ch);
        }
        used += cw;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;

    fn plain(lines: &[Vec<Span>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn wrap_should_break_at_word_boundaries() {
        let spans = [Span::new("aaa bbb ccc", Style::default())];
        assert_eq!(plain(&wrap(&spans, 7)), ["aaa bbb", "ccc"]);
    }

    #[test]
    fn wrap_should_keep_styled_word_parts_together() {
        let spans = [
            Span::new("xx ", Style::default()),
            Span::new("bold", Style::dim()),
            Span::new("ness", Style::default()),
        ];
        assert_eq!(plain(&wrap(&spans, 8)), ["xx", "boldness"]);
    }

    #[test]
    fn wrap_should_split_overlong_words() {
        let spans = [Span::new("abcdefgh", Style::default())];
        assert_eq!(plain(&wrap(&spans, 3)), ["abc", "def", "gh"]);
    }
}
