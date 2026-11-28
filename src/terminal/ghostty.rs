//! Ghostty: the font from its config, activity as its progress indicator
//! (OSC 9;4).

use std::process::{Command, Stdio};

use super::{Status, Terminal};

pub struct Ghostty;

impl Terminal for Ghostty {
    /// From `ghostty +show-config`, the config as Ghostty resolved it
    /// (included files, defaults). Ghostty puts its binary's directory in
    /// `GHOSTTY_BIN_DIR`; outside Ghostty `ghostty` may be on PATH.
    fn font_family(&self) -> Option<String> {
        let bin = std::env::var_os("GHOSTTY_BIN_DIR").map_or_else(
            || "ghostty".into(),
            |dir| std::path::Path::new(&dir).join("ghostty"),
        );
        let output = Command::new(bin)
            .arg("+show-config")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        font_family(&String::from_utf8_lossy(&output.stdout))
    }

    fn status(&self, status: Status<'_>) -> String {
        match status {
            // OSC 9;4 state 3: indeterminate.
            Status::Working(_) => "\x1b]9;4;3\x1b\\".into(),
            // OSC 9;4 state 0: remove the indicator.
            Status::Idle => "\x1b]9;4;0\x1b\\".into(),
            // The error state would stay on the tab over the shell prompt,
            // with no message; stderr already says what went wrong.
            Status::Failed(_) => String::new(),
        }
    }
}

/// `font-family-bold` if set, else `font-family`; each may repeat for
/// fallbacks, and the first non-empty one is the primary font.
fn font_family(config: &str) -> Option<String> {
    let first = |key: &str| {
        config.lines().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            let v = v.trim().trim_matches('"');
            (k.trim() == key && !v.is_empty()).then(|| v.to_owned())
        })
    };
    first("font-family-bold").or_else(|| first("font-family"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_family_should_prefer_bold_family() {
        let config = "font-family = \nfont-family = Iosevka\nfont-family-bold = \"Fira Code\"\n";
        assert_eq!(font_family(config).as_deref(), Some("Fira Code"));
    }

    #[test]
    fn font_family_should_take_first_non_empty_entry() {
        let config =
            "font-family = \nfont-family = Iosevka\nfont-family = Symbols\nfont-family-bold = \n";
        assert_eq!(font_family(config).as_deref(), Some("Iosevka"));
    }

    #[test]
    fn font_family_should_be_none_when_unset() {
        assert_eq!(font_family("font-family = \nfont-size = 13\n"), None);
    }

    #[test]
    fn status_should_set_and_remove_progress() {
        assert_eq!(Ghostty.status(Status::Working("x")), "\x1b]9;4;3\x1b\\");
        assert_eq!(Ghostty.status(Status::Idle), "\x1b]9;4;0\x1b\\");
        assert_eq!(Ghostty.status(Status::Failed("x")), "");
    }
}
