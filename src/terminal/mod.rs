//! What differs between the terminals martty runs in. Ghostty and Rex share
//! a terminal core, so drawing and the capability probe (`term.rs`) are the
//! same in both; the font the user chose and how a program reports what it
//! is doing are not. Which one martty runs in is decided once, from
//! `TERM_PROGRAM`; anything other than Rex is treated as Ghostty.

mod ghostty;
mod rex;

use std::sync::OnceLock;

/// What martty is doing, for the terminal's activity indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status<'a> {
    /// Busy for a noticeable while: laying out, downloading. `what` is one
    /// line for people, such as `Downloading README.md`; `progress` is the
    /// share done in percent, when known.
    Working { what: &'a str, progress: Option<u8> },
    /// The work is over; martty shows the result or waits for keys.
    Idle,
    /// Something new to look at, such as a followed file that changed.
    Done(&'a str),
    /// martty stopped on this error.
    Failed(&'a str),
}

pub trait Terminal: Sync {
    /// The family of the font the user set in the terminal, for headings
    /// drawn as images; `None` keeps the bundled font.
    fn font_family(&self) -> Option<String>;

    /// The escape sequence that reports `status`, or nothing when the
    /// terminal has no use for it.
    fn status(&self, status: Status<'_>) -> String;
}

pub fn current() -> &'static dyn Terminal {
    static REX: OnceLock<bool> = OnceLock::new();
    if *REX.get_or_init(|| std::env::var_os("TERM_PROGRAM").is_some_and(|p| p == "rex")) {
        &rex::Rex
    } else {
        &ghostty::Ghostty
    }
}

/// `msg` with every URL cut to scheme, host and path: no credentials, query
/// or fragment, where tokens travel. Terminals keep status messages, and
/// Rex may pass them on, while the screen and stderr show them only once.
pub(super) fn redact_urls(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(at) = rest.find("://") {
        out.push_str(&rest[..at + 3]);
        let after = &rest[at + 3..];
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let url = &after[..end];
        // A URL at the end of a phrase, as in `URL: error`.
        let trail = url.len() - url.trim_end_matches([':', ',', ')']).len();
        let url = &url[..url.len() - trail];
        let host_end = url.find('/').unwrap_or(url.len());
        let start = url[..host_end].rfind('@').map_or(0, |i| i + 1);
        let kept = &url[start..];
        let kept = &kept[..kept.find(['?', '#']).unwrap_or(kept.len())];
        out.push_str(kept);
        out.push_str(&after[end - trail..end]);
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_urls_should_drop_credentials_query_and_fragment() {
        assert_eq!(
            redact_urls("Can't download https://u:p@e.com/a.md?token=x#y: 404"),
            "Can't download https://e.com/a.md: 404"
        );
        assert_eq!(redact_urls("see http://e.com/a b"), "see http://e.com/a b");
        assert_eq!(redact_urls("no url here"), "no url here");
        assert_eq!(redact_urls("a://"), "a://");
    }
}
