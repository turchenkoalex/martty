//! Passes over the whole document before it is laid out: what to fetch or
//! lay out in parallel ahead of time, and what layout must know in advance.

use std::collections::VecDeque;

use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};

use super::html;

/// Starts preparing, in the background, what laying `markdown` out will
/// need, while the terminal is being probed: the grammars of its code
/// blocks compile on first use (about 1 ms each).
pub fn warm_up(markdown: &str) {
    let langs: Vec<String> = markdown
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            l.strip_prefix("```").or_else(|| l.strip_prefix("~~~"))
        })
        .filter_map(|info| info.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    if !langs.is_empty() {
        std::thread::spawn(move || super::highlight::warm_up(&langs));
    }
}

/// Every image a document references, from Markdown and from `<img>` tags.
pub(super) fn image_urls<'a>(events: &'a [Event<'_>]) -> Vec<&'a str> {
    let mut urls = Vec::new();
    for event in events {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => urls.push(dest_url.as_ref()),
            Event::Html(html) | Event::InlineHtml(html) => urls.extend(html::img_sources(html)),
            _ => {}
        }
    }
    urls
}

/// The source of every ```` ```mermaid ```` block.
pub(super) fn mermaid_sources<'a>(events: &'a [Event<'_>]) -> Vec<&'a str> {
    let mut sources = Vec::new();
    let mut in_mermaid = false;
    for event in events {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                in_mermaid = info.split_whitespace().next() == Some("mermaid");
            }
            // Merged by `TextMergeStream`, so a block is one text event.
            Event::Text(text) if in_mermaid => sources.push(text.as_ref()),
            Event::End(TagEnd::CodeBlock) => in_mermaid = false,
            _ => {}
        }
    }
    sources
}

/// Item count of every ordered list, in the order the lists start, so
/// markers can be padded to the widest number before it is reached.
pub(super) fn ordered_list_lengths(events: &[Event<'_>]) -> VecDeque<u64> {
    let mut lengths = VecDeque::new();
    let mut open: Vec<Option<usize>> = Vec::new();
    for event in events {
        match event {
            Event::Start(Tag::List(Some(_))) => {
                open.push(Some(lengths.len()));
                lengths.push_back(0);
            }
            Event::Start(Tag::List(None)) => open.push(None),
            Event::End(TagEnd::List(_)) => {
                open.pop();
            }
            Event::Start(Tag::Item) => {
                if let Some(n) = open
                    .last()
                    .copied()
                    .flatten()
                    .and_then(|i| lengths.get_mut(i))
                {
                    *n += 1;
                }
            }
            _ => {}
        }
    }
    lengths
}
