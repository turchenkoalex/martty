//! Link targets and heading anchors.

use std::collections::HashMap;
use std::path::Path;

/// The URL a link points to, as OSC 8 should carry it: URLs with a scheme
/// and `#fragment`s as they are, relative paths as absolute `file://` URLs,
/// since Ghostty and the pager can't know what they were relative to.
pub(super) fn resolve(base: &Path, url: &str) -> String {
    if url.is_empty() || url.starts_with('#') || has_scheme(url) {
        return url.to_owned();
    }
    let (path, fragment) = match url.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (url, None),
    };
    let path = path.replace("%20", " ");
    let joined = base.join(&path);
    let absolute = std::path::absolute(&joined).unwrap_or(joined);
    let mut out = format!("file://{}", absolute.display()).replace(' ', "%20");
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
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
    fn anchors_should_follow_github_rules() {
        let mut anchors = Anchors::default();
        assert_eq!(anchors.next("Tables & Lists!"), "tables--lists");
        assert_eq!(anchors.next("Заголовок 2"), "заголовок-2");
        assert_eq!(anchors.next("Tables & Lists"), "tables--lists-1");
    }
}
