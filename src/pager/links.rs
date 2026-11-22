//! Links in rendered lines, found by their OSC 8 sequences, for moving
//! between them with Tab and opening them with Enter.

use std::ffi::OsStr;
use std::ops::Range;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::render::Line;

const OSC8: &str = "\x1b]8;;";

/// One link as the reader sees it. A link wrapped onto the next line is
/// written as one OSC 8 run per line; those runs form a single link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Link {
    pub(super) url: String,
    /// Line index and byte range of the link text within that line.
    pub(super) parts: Vec<(usize, Range<usize>)>,
}

impl Link {
    pub(super) fn first_line(&self) -> usize {
        self.parts.first().map_or(0, |p| p.0)
    }

    pub(super) fn last_line(&self) -> usize {
        self.parts.last().map_or(0, |p| p.0)
    }
}

pub(super) fn find(lines: &[Line]) -> Vec<Link> {
    let mut links: Vec<Link> = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        for (i, (url, range)) in runs(&line.text).into_iter().enumerate() {
            let continues = i == 0
                && links.last().is_some_and(|l| {
                    let wraps = l
                        .parts
                        .last()
                        .is_some_and(|p| ends_line(&lines[p.0].text, &p.1));
                    l.url == url && l.last_line() + 1 == n && wraps
                });
            match links.last_mut() {
                Some(link) if continues => link.parts.push((n, range)),
                _ => links.push(Link {
                    url,
                    parts: vec![(n, range)],
                }),
            }
        }
    }
    links
}

/// OSC 8 runs of a line: target and the byte range of the text inside.
fn runs(text: &str) -> Vec<(String, Range<usize>)> {
    let mut out = Vec::new();
    let mut open: Option<(String, usize)> = None;
    let mut at = 0;
    while let Some(found) = text[at..].find(OSC8) {
        let start = at + found;
        let params = start + OSC8.len();
        let Some((url_len, term_len)) = terminator(&text[params..]) else {
            break;
        };
        let url = &text[params..params + url_len];
        let after = params + url_len + term_len;
        if let Some((target, from)) = open.take() {
            out.push((target, from..start));
        }
        if !url.is_empty() {
            open = Some((url.to_owned(), after));
        }
        at = after;
    }
    if let Some((target, from)) = open {
        out.push((target, from..text.len()));
    }
    out
}

/// Length of the OSC parameter up to its ST (`ESC \`) or BEL, and of that
/// terminator.
fn terminator(s: &str) -> Option<(usize, usize)> {
    let st = s.find("\x1b\\").map(|i| (i, 2));
    let bel = s.find('\x07').map(|i| (i, 1));
    match (st, bel) {
        (Some(a), Some(b)) => Some(if a.0 < b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// Whether only escape sequences follow `range` on its line: the link runs
/// to the line's end and may continue on the next one.
fn ends_line(text: &str, range: &Range<usize>) -> bool {
    super::view::plain_text(&text[range.end..])
        .trim()
        .is_empty()
}

/// `text` with the bytes in `range` shown in reverse video. SGR inside the
/// range may reset attributes, so reverse is set again after each one.
pub(super) fn highlight(text: &str, range: &Range<usize>) -> String {
    let inner = &text[range.clone()];
    let mut out = String::with_capacity(text.len() + 16);
    out.push_str(&text[..range.start]);
    out.push_str("\x1b[7m");
    let mut rest = inner;
    while let Some(i) = rest.find("\x1b[") {
        let seq = &rest[i + 2..];
        let len = seq
            .find(|c: char| ('\x40'..='\x7e').contains(&c))
            .map_or(seq.len(), |e| e + 1);
        out.push_str(&rest[..i + 2 + len]);
        if seq[..len].ends_with('m') {
            out.push_str("\x1b[7m");
        }
        rest = &rest[i + 2 + len..];
    }
    out.push_str(rest);
    out.push_str("\x1b[27m");
    out.push_str(&text[range.end..]);
    out
}

/// `text` with every range in `ranges` reversed, as `highlight` does;
/// a range overlapping one already marked is skipped.
pub(super) fn highlight_all(text: &str, mut ranges: Vec<Range<usize>>) -> String {
    ranges.sort_by_key(|r| std::cmp::Reverse(r.start));
    let mut out = text.to_owned();
    let mut limit = usize::MAX;
    // From the last range back, so earlier byte offsets stay valid.
    for range in ranges {
        if range.end > limit || range.end > out.len() {
            continue;
        }
        limit = range.start;
        out = highlight(&out, &range);
    }
    out
}

pub(super) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// Hands a URL or file to the system (`open` on macOS, `xdg-open`
/// elsewhere) and returns a status message.
pub(super) fn open_external(target: &OsStr) -> String {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let spawned = Command::new(opener)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // Reaped in the background, so the pager never waits on it.
            std::thread::spawn(move || child.wait());
            format!("Opened {}", target.to_string_lossy())
        }
        Err(e) => format!("{opener}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line {
        Line::new(text)
    }

    fn link(url: &str, text: &str) -> String {
        format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
    }

    #[test]
    fn find_should_list_links_in_order() {
        let lines = [
            line(&format!("see {} and {}", link("a", "A"), link("b", "B"))),
            line(&format!("then {}", link("c", "C"))),
        ];
        let urls: Vec<String> = find(&lines).into_iter().map(|l| l.url).collect();
        assert_eq!(urls, ["a", "b", "c"]);
    }

    #[test]
    fn find_should_join_a_link_wrapped_onto_the_next_line() {
        let lines = [
            line(&format!("see {}\x1b[0m", link("a", "long"))),
            line(&format!("{} rest", link("a", "link"))),
        ];
        let found = find(&lines);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].first_line(), found[0].last_line()), (0, 1));
    }

    #[test]
    fn find_should_keep_separate_links_to_the_same_url() {
        let lines = [
            line(&format!("{} end", link("a", "one"))),
            line(&link("a", "two")),
        ];
        assert_eq!(find(&lines).len(), 2);
    }

    #[test]
    fn highlight_should_keep_reverse_through_sgr_resets() {
        let text = format!("x {}", link("u", "a\x1b[0mb"));
        let found = find(&[line(&text)]);
        let out = highlight(&text, &found[0].parts[0].1);
        assert!(out.contains("\x1b[7ma\x1b[0m\x1b[7mb\x1b[27m"), "{out:?}");
    }
}
