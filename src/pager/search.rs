//! `/` search: matches across the document, their highlight on screen and
//! the `3/17` counter.

use std::ops::Range;

use super::Pager;
use super::view::{View, visible_chars};

/// One match: line index and byte range in that line's ANSI text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Hit {
    pub(super) line: usize,
    pub(super) range: Range<usize>,
}

/// Matches of `query` in a rendered line, as byte ranges of its ANSI text.
/// Smart case: the query is case-sensitive only if it has uppercase.
pub(super) fn find_in_line(line: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let sensitive = query.chars().any(char::is_uppercase);
    let chars = visible_chars(line);
    // The text searched, and for each of its bytes the character it came
    // from: lowercasing may change a character's length.
    let mut hay = String::new();
    let mut from = Vec::new();
    for (i, (c, _)) in chars.iter().enumerate() {
        let before = hay.len();
        if sensitive {
            hay.push(*c);
        } else {
            hay.extend(c.to_lowercase());
        }
        from.resize(hay.len(), i);
        debug_assert!(hay.len() > before);
    }
    let needle = if sensitive {
        query.to_owned()
    } else {
        query.to_lowercase()
    };
    hay.match_indices(&needle)
        .filter_map(|(at, m)| {
            let first = &chars[*from.get(at)?].1;
            let last = &chars[*from.get(at + m.len() - 1)?].1;
            Some(first.start..last.end)
        })
        .collect()
}

fn find_all(view: &View, query: &str) -> Vec<Hit> {
    let doc = &view.doc;
    let mut hits: Vec<Hit> = doc
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.image.is_none())
        .flat_map(|(line, l)| {
            find_in_line(&l.text, query)
                .into_iter()
                .map(move |range| Hit { line, range })
        })
        .collect();
    // A heading drawn as an image has no text on its lines, so its matches
    // land on its first row with an empty range: the pager moves there, and
    // nothing is reversed, since swapping the colors of placeholder cells
    // would change the image id they carry.
    for heading in &doc.headings {
        if doc
            .lines
            .get(heading.line)
            .is_some_and(|l| l.image.is_some())
        {
            let count = find_in_line(&heading.text, query).len();
            hits.extend((0..count).map(|_| Hit {
                line: heading.line,
                range: 0..0,
            }));
        }
    }
    hits.sort_by_key(|h| h.line);
    hits
}

impl Pager<'_> {
    /// Finds the matches of the query in the current view again if it
    /// changed, or the view was laid out anew.
    pub(super) fn refresh_hits(&mut self) {
        let query = self.query.clone();
        let view = self.view_mut();
        match query {
            None => view.hits = None,
            Some(q) if view.hits.as_ref().is_none_or(|(old, _)| *old != q) => {
                // A view laid out anew keeps the match it was at.
                if view.hits.is_some() {
                    view.current_hit = None;
                }
                let hits = find_all(view, &q);
                view.hits = Some((q, hits));
            }
            Some(_) => {}
        }
    }

    /// Moves to the next (or previous) line with a match. Starts from the
    /// match moved to last while it is on screen, otherwise from the top
    /// line; a match off screen is scrolled to the top.
    pub(super) fn find(&mut self, forward: bool) {
        self.refresh_hits();
        let rows = self.view_rows();
        let view = self.view();
        let Some((query, hits)) = &view.hits else {
            return;
        };
        let on_screen = |l: usize| (view.top..view.top + rows).contains(&l);
        let base = view
            .current_hit
            .filter(|&l| on_screen(l))
            .unwrap_or(view.top);
        let mut lines = hits.iter().map(|h| h.line);
        let found = if forward {
            lines.find(|&l| l > base)
        } else {
            lines.rev().find(|&l| l < base)
        };
        match found {
            Some(line) => {
                let visible = on_screen(line);
                self.view_mut().current_hit = Some(line);
                if !visible {
                    self.jump(line);
                }
            }
            None if hits.is_empty() => self.message = Some(format!("Pattern not found: {query}")),
            None => self.message = Some(format!("No more matches: {query}")),
        }
    }

    /// `3/17`: the first match on the line moved to last, out of all.
    pub(super) fn hit_counter(&self) -> Option<String> {
        let view = self.view();
        let (query, hits) = view.hits.as_ref()?;
        let line = view.current_hit?;
        let n = hits.iter().position(|h| h.line == line)? + 1;
        Some(format!("/{query} {n}/{}", hits.len()))
    }

    /// Byte ranges to show reversed on line `index`: matches and the
    /// selected link. A match inside the selected link gives way to it, so
    /// the whole link is reversed and not just the matched word.
    pub(super) fn marked_ranges(&self, index: usize) -> Vec<Range<usize>> {
        let view = self.view();
        let selected = view.selected_range(index);
        let overlaps = |r: &Range<usize>| {
            selected
                .as_ref()
                .is_some_and(|s| r.start < s.end && s.start < r.end)
        };
        let mut ranges: Vec<Range<usize>> = view
            .hits
            .iter()
            .flat_map(|(_, hits)| hits)
            .filter(|h| h.line == index && !h.range.is_empty() && !overlaps(&h.range))
            .map(|h| h.range.clone())
            .collect();
        ranges.extend(selected);
        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_in_line_should_map_matches_to_ansi_bytes() {
        let line = "\x1b[1mHello\x1b[0m world";
        assert_eq!(find_in_line(line, "lo w"), vec![7..15]);
    }

    #[test]
    fn find_in_line_should_ignore_case_for_lowercase_query() {
        assert_eq!(find_in_line("Ab ab AB", "ab").len(), 3);
        assert_eq!(find_in_line("Ab ab AB", "AB").len(), 1);
    }

    #[test]
    fn find_in_line_should_handle_case_changing_length() {
        // 'İ' lowercases to two characters.
        assert_eq!(find_in_line("İx", "x"), vec![2..3]);
    }
}
