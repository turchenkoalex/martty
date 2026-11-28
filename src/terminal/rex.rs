//! Rex: the font from its macOS defaults, activity through the Program
//! Status Protocol (OSC 7501), which Rex shows on tabs and in the session
//! picker. Rex has no Ghostty config and doesn't show OSC 9;4.

use std::fmt::Write as _;
use std::process::{Command, Stdio};

use super::{Status, Terminal};
use crate::kitty;

/// Shown in Rex's lists, and well under the protocol's 2048-byte limit.
const MAX_MESSAGE_CHARS: usize = 200;

pub struct Rex;

impl Terminal for Rex {
    /// The font picked in Rex's settings; `None` (the bundled font) when
    /// the setting was never changed.
    fn font_family(&self) -> Option<String> {
        let output = Command::new("defaults")
            .args(["read", "com.superlogical.rex", "Workspace.terminalFontName"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let family = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (!family.is_empty()).then_some(family)
    }

    /// Reports go to the root record; each replaces it whole, so every one
    /// carries `app`. `working` is dropped by Rex when martty exits, while
    /// `done` and `error` stay, marking the tab until the user looks.
    fn status(&self, status: Status<'_>) -> String {
        let body = match status {
            Status::Working { what, progress } => {
                let mut body = format!("state=working:app=martty:msg={}", encode(what));
                if let Some(p) = progress {
                    let _ = write!(body, ":progress={}", p.min(100));
                }
                body
            }
            Status::Idle => "state=idle:app=martty".to_owned(),
            Status::Done(msg) => format!("state=done:app=martty:msg={}", encode(msg)),
            Status::Failed(msg) => format!("state=error:app=martty:msg={}", encode(msg)),
        };
        format!("\x1b]7501;{body}\x1b\\")
    }
}

/// Base64 of one line: the protocol refuses a whole report whose message
/// holds a control character, and file names and errors may.
fn encode(msg: &str) -> String {
    let line: String = msg
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_MESSAGE_CHARS)
        .collect();
    kitty::base64(line.trim().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_should_report_working_with_message() {
        let working = |progress| Status::Working {
            what: "Hi",
            progress,
        };
        assert_eq!(
            Rex.status(working(None)),
            "\x1b]7501;state=working:app=martty:msg=SGk=\x1b\\"
        );
        assert_eq!(
            Rex.status(working(Some(40))),
            "\x1b]7501;state=working:app=martty:msg=SGk=:progress=40\x1b\\"
        );
        assert_eq!(
            Rex.status(Status::Idle),
            "\x1b]7501;state=idle:app=martty\x1b\\"
        );
        assert_eq!(
            Rex.status(Status::Done("Hi")),
            "\x1b]7501;state=done:app=martty:msg=SGk=\x1b\\"
        );
    }

    #[test]
    fn status_should_keep_control_characters_out_of_the_message() {
        let report = Rex.status(Status::Failed("a\nb\x1b"));
        assert_eq!(
            report,
            format!(
                "\x1b]7501;state=error:app=martty:msg={}\x1b\\",
                kitty::base64(b"a b")
            )
        );
    }
}
