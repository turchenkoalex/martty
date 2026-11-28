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
