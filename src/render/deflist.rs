//! pulldown-cmark starts a definition at any line beginning with `:`, even
//! without a space after it, so a paragraph wrapped before `:emoji:` or
//! `::path` turns into a definition list. Such colons are escaped (`\:`) and
//! the text parsed again, so they stay in their paragraph.

use std::borrow::Cow;

use pulldown_cmark::{Event, Options, Parser, Tag};

/// `markdown` with every definition marker that lacks a following space
/// escaped. Borrowed when there are none, which is the usual case.
pub(super) fn escape_false_markers(markdown: &str, options: Options) -> Cow<'_, str> {
    if !options.contains(Options::ENABLE_DEFINITION_LIST) {
        return Cow::Borrowed(markdown);
    }
    let markers: Vec<usize> = Parser::new_ext(markdown, options)
        .into_offset_iter()
        .filter(|(event, _)| matches!(event, Event::Start(Tag::DefinitionListDefinition)))
        .map(|(_, range)| range.start)
        .filter(|&at| {
            markdown
                .get(at..)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| !rest.starts_with(char::is_whitespace))
        })
        .collect();
    if markers.is_empty() {
        return Cow::Borrowed(markdown);
    }
    let mut out = String::with_capacity(markdown.len() + markers.len());
    let mut last = 0;
    for at in markers {
        out.push_str(&markdown[last..at]);
        out.push('\\');
        last = at;
    }
    out.push_str(&markdown[last..]);
    Cow::Owned(out)
}
