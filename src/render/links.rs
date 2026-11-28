//! Link targets and heading anchors.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use super::Config;

/// [`resolve`] against the document's URL when it was downloaded, else
/// against its directory.
pub(super) fn target(cfg: &Config, url: &str) -> String {
    resolve(&cfg.base_dir, &absolute(cfg.base_url.as_deref(), url))
}

/// `url` made absolute against `base`, the URL of a downloaded document;
/// as it is without one, or when it has a scheme or is a `#fragment`.
pub(super) fn absolute<'a>(base: Option<&str>, url: &'a str) -> Cow<'a, str> {
    match base {
        Some(base) if !url.is_empty() && !url.starts_with('#') && !has_scheme(url) => {
            Cow::Owned(encode(&join(base, url), |c| c.is_control() || c == ' '))
        }
        _ => Cow::Borrowed(url),
    }
}

/// RFC 3986 reference resolution, for the references a Markdown file has:
/// `//host/…`, `/path`, and paths relative to the base's directory.
fn join(base: &str, url: &str) -> String {
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    if let Some(network) = url.strip_prefix("//") {
        return format!("{scheme}://{network}");
    }
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, path) = rest.split_at(host_end);
    let joined = if url.starts_with('/') {
        url.to_owned()
    } else {
        let path = path.split(['?', '#']).next().unwrap_or_default();
        let dir = path.rfind('/').map_or("", |i| &path[..i]);
        format!("{dir}/{url}")
    };
    format!("{scheme}://{host}{}", remove_dots(&joined))
}

/// `.` and `..` segments of an absolute path applied; `..` stops at the root.
fn remove_dots(path: &str) -> String {
    let (path, tail) = path
        .find(['?', '#'])
        .map_or((path, ""), |i| path.split_at(i));
    let segments: Vec<&str> = path.split('/').collect();
    let mut out: Vec<&str> = Vec::with_capacity(segments.len());
    for segment in &segments {
        match *segment {
            "." => {}
            ".." => {
                // The first segment is the empty one before the leading `/`.
                if out.len() > 1 {
                    out.pop();
                }
            }
            s => out.push(s),
        }
    }
    let mut joined = out.join("/");
    if matches!(segments.last(), Some(&("." | ".."))) {
        joined.push('/');
    }
    joined.push_str(tail);
    joined
}

/// The URL a link points to, as OSC 8 should carry it: URLs with a scheme
/// and `#fragment`s as they are, relative paths as absolute `file://` URLs,
/// since Ghostty and the pager can't know what they were relative to.
///
/// Control characters are percent-encoded wherever they are: the URL goes
/// into an OSC 8 sequence, which one of them would end early.
pub(super) fn resolve(base: &Path, url: &str) -> String {
    if url.is_empty() || url.starts_with('#') || has_scheme(url) {
        return encode(url, |c| c.is_control());
    }
    let (path, fragment) = match url.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (url, None),
    };
    let joined = base.join(percent_decode(path));
    let absolute = std::path::absolute(&joined).unwrap_or(joined);
    let mut out = "file://".to_owned();
    out.push_str(&encode(&absolute.to_string_lossy(), |c| {
        c.is_control() || matches!(c, ' ' | '%' | '#' | '?')
    }));
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(&encode(fragment, |c| c.is_control()));
    }
    out
}

/// `text` with the characters `escape` picks written as `%XX` per UTF-8 byte.
fn encode(text: &str, escape: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if escape(c) {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                let _ = write!(out, "%{b:02X}");
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `%XX` escapes decoded; malformed ones and invalid UTF-8 are kept as text.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `scheme:` per RFC 3986: a letter, then letters, digits, `+`, `-`, `.`.
fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// GitHub-style heading anchors: lowercase, punctuation dropped, spaces as
/// `-`, and `-1`, `-2`… for repeats.
#[derive(Default)]
pub(super) struct Anchors(HashMap<String, usize>);

impl Anchors {
    pub(super) fn next(&mut self, heading: &str) -> String {
        let slug: String = heading
            .trim()
            .to_lowercase()
            .chars()
            .filter_map(|c| match c {
                ' ' => Some('-'),
                '-' | '_' => Some(c),
                c if c.is_alphanumeric() => Some(c),
                _ => None,
            })
            .collect();
        let seen = self.0.entry(slug.clone()).or_insert(0);
        let anchor = if *seen == 0 {
            slug
        } else {
            format!("{slug}-{seen}")
        };
        *seen += 1;
        anchor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_should_keep_urls_and_fragments() {
        let base = Path::new("/docs");
        assert_eq!(resolve(base, "https://e.com/a"), "https://e.com/a");
        assert_eq!(resolve(base, "mailto:me@e.com"), "mailto:me@e.com");
        assert_eq!(resolve(base, "#tables"), "#tables");
    }

    #[test]
    fn resolve_should_make_relative_paths_absolute() {
        let base = Path::new("/docs");
        assert_eq!(resolve(base, "README.md"), "file:///docs/README.md");
        assert_eq!(resolve(base, "a b.md#x"), "file:///docs/a%20b.md#x");
    }

    #[test]
    fn resolve_should_round_trip_escaped_paths() {
        let url = resolve(Path::new("/docs"), "100%25%20done.md");
        assert_eq!(url, "file:///docs/100%25%20done.md");
        assert_eq!(percent_decode(&url), "file:///docs/100% done.md");
    }

    #[test]
    fn resolve_should_encode_control_characters() {
        assert_eq!(
            resolve(Path::new("/"), "https://a\x1b]0;x\x07"),
            "https://a%1B]0;x%07"
        );
    }

    #[test]
    fn absolute_should_resolve_against_the_document_url() {
        let base = Some("https://raw.example.com/o/r/main/docs/Spring%20Boot.md?x=1");
        let cases = [
            (
                "img/a.png",
                "https://raw.example.com/o/r/main/docs/img/a.png",
            ),
            (
                "../README.md#use",
                "https://raw.example.com/o/r/main/README.md#use",
            ),
            ("./a b.md", "https://raw.example.com/o/r/main/docs/a%20b.md"),
            ("/top.png", "https://raw.example.com/top.png"),
            ("//cdn.example.com/x.svg", "https://cdn.example.com/x.svg"),
            ("../../../../../up.md", "https://raw.example.com/up.md"),
            ("https://e.com/a", "https://e.com/a"),
            ("#tables", "#tables"),
        ];
        for (url, want) in cases {
            assert_eq!(absolute(base, url), want, "{url}");
        }
        assert_eq!(absolute(None, "img/a.png"), "img/a.png");
    }

    #[test]
    fn percent_decode_should_keep_malformed_escapes() {
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%zz%41"), "%zzA");
    }

    #[test]
    fn anchors_should_follow_github_rules() {
        let mut anchors = Anchors::default();
        assert_eq!(anchors.next("Tables & Lists!"), "tables--lists");
        assert_eq!(anchors.next("Заголовок 2"), "заголовок-2");
        assert_eq!(anchors.next("Tables & Lists"), "tables--lists-1");
    }
}
