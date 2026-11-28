//! A Markdown document given on the command line as an http(s) URL.
//!
//! Unlike images in a document, the URL comes from the user, so it isn't
//! fenced in as `remote.rs` fences those: plain http and hosts inside the
//! network are fine. It is fetched once per run, with no disk cache, and is
//! independent of `--remote`.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use ureq::config::AutoHeaderValue;

/// Larger than any Markdown file people read in a terminal.
const MAX_BYTES: u64 = 16 << 20;
const TIMEOUT: Duration = Duration::from_secs(30);

/// The URL to download for `url`: a file page on GitHub
/// (`github.com/OWNER/REPO/blob/REF/PATH`) becomes the file itself on
/// `raw.githubusercontent.com`; any other URL stays as it is.
pub fn raw_url(url: &str) -> String {
    let Some((_, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut parts = rest.splitn(5, '/');
    let (Some(host), Some(owner), Some(repo), Some("blob" | "raw"), Some(path)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return url.to_owned();
    };
    if !matches!(
        host.to_ascii_lowercase().as_str(),
        "github.com" | "www.github.com"
    ) {
        return url.to_owned();
    }
    format!("https://raw.githubusercontent.com/{owner}/{repo}/{path}")
}

/// The file name in `url`'s path, for the pager's status line.
pub fn title(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    crate::render::percent_decode(name)
}

/// Downloads the document, with invalid UTF-8 shown as `�`.
pub fn fetch(url: &str) -> io::Result<String> {
    let failed = |e: ureq::Error| io::Error::other(format!("{url}: {e}"));
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(AutoHeaderValue::Provided(Arc::new("martty".to_owned())))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent.get(url).call().map_err(failed)?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok());
    if content_type.is_some_and(is_page) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{url}: an HTML page, not a Markdown file"),
        ));
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .map_err(failed)?;
    Ok(crate::lossy(bytes))
}

/// A web page rather than a file: rendered as Markdown it would be a wall
/// of tags.
fn is_page(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_url_should_turn_github_file_pages_into_raw_files() {
        assert_eq!(
            raw_url("https://github.com/vgv/kolbasa/blob/main/docs/Spring%20Boot.md"),
            "https://raw.githubusercontent.com/vgv/kolbasa/main/docs/Spring%20Boot.md"
        );
        assert_eq!(
            raw_url("https://github.com/o/r/blob/v1.0/README.md?plain=1#L10"),
            "https://raw.githubusercontent.com/o/r/v1.0/README.md"
        );
    }

    #[test]
    fn raw_url_should_keep_other_urls() {
        for url in [
            "https://raw.githubusercontent.com/o/r/refs/heads/main/a.md",
            "https://github.com/o/r",
            "https://github.com/o/r/tree/main/docs",
            "https://example.com/o/r/blob/main/a.md",
        ] {
            assert_eq!(raw_url(url), url);
        }
    }

    #[test]
    fn title_should_be_the_decoded_file_name() {
        assert_eq!(
            title("https://raw.githubusercontent.com/o/r/main/docs/Spring%20Boot.md"),
            "Spring Boot.md"
        );
        assert_eq!(title("https://example.com/notes/?x=1"), "notes");
    }

    #[test]
    fn is_page_should_spot_html() {
        assert!(is_page("text/html; charset=utf-8"));
        assert!(!is_page("text/plain; charset=utf-8"));
        assert!(!is_page("text/markdown"));
    }
}
