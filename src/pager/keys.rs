use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::time::Duration;

use crate::term::wait_readable;

/// How often the pager wakes up without input, to notice a resize.
const POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Key {
    Char(char),
    Ctrl(u8),
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Tab,
    /// Shift-Tab (`CSI Z`).
    BackTab,
    Backspace,
    Esc,
    /// Ghostty switched between its light and dark theme (DEC 2031
    /// notification `CSI ? 997 ; 1|2 n`): not a key, but it arrives with them.
    ThemeChanged,
}

/// Waits up to `POLL` for input. `None` on timeout.
pub(super) fn read_keys(tty: &mut File) -> io::Result<Option<Vec<Key>>> {
    if !wait_readable(tty.as_raw_fd(), POLL) {
        return Ok(None);
    }
    let mut buf = [0u8; 1024];
    let n = tty.read(&mut buf)?;
    Ok(Some(parse_keys(&buf[..n])))
}

pub(super) fn parse_keys(buf: &[u8]) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        i += 1;
        let key = match b {
            0x1b => match buf.get(i) {
                Some(b'[' | b'O') => {
                    let start = i + 1;
                    let len = buf[start..]
                        .iter()
                        .position(|c| (0x40..=0x7e).contains(c))
                        .unwrap_or(buf.len() - start);
                    let params = &buf[start..start + len];
                    let fin = buf.get(start + len).copied();
                    i = (start + len + 1).min(buf.len());
                    let Some(key) = csi_key(fin, params) else {
                        continue;
                    };
                    key
                }
                _ => Key::Esc,
            },
            b'\r' | b'\n' => Key::Enter,
            b'\t' => Key::Tab,
            0x7f | 0x08 => Key::Backspace,
            0..0x20 => Key::Ctrl(b),
            _ => {
                let len = match b {
                    0xf0.. => 4,
                    0xe0.. => 3,
                    0xc0.. => 2,
                    _ => 1,
                };
                let end = (i - 1 + len).min(buf.len());
                let ch = std::str::from_utf8(&buf[i - 1..end])
                    .ok()
                    .and_then(|s| s.chars().next());
                i = end;
                match ch {
                    Some(c) => Key::Char(c),
                    None => continue,
                }
            }
        };
        keys.push(key);
    }
    keys
}

/// The key a CSI (or SS3) sequence stands for. Modifiers after `;` are
/// dropped from legacy keys (`CSI 1;5A` is Up), so Ctrl-arrows act as arrows.
fn csi_key(fin: Option<u8>, params: &[u8]) -> Option<Key> {
    let first = params.split(|&b| b == b';').next().unwrap_or_default();
    Some(match (fin?, first) {
        (b'A', _) => Key::Up,
        (b'B', _) => Key::Down,
        (b'C', _) => Key::Right,
        (b'D', _) => Key::Left,
        (b'H', _) => Key::Home,
        (b'F', _) => Key::End,
        (b'Z', _) => Key::BackTab,
        (b'~', b"5") => Key::PageUp,
        (b'~', b"6") => Key::PageDown,
        (b'~', b"1" | b"7") => Key::Home,
        (b'~', b"4" | b"8") => Key::End,
        (b'n', b"?997") => Key::ThemeChanged,
        (b'u', _) => return kitty_key(params),
        _ => return None,
    })
}

/// A key in the kitty keyboard protocol, `CSI code[:alternates] ;
/// modifiers[:event] u`. With "disambiguate" (flag 1) the pager gets these
/// only for Esc and for keys with Ctrl or Alt held (or modified Enter, Tab,
/// Backspace); plain text still comes as text. Ctrl-letters map to the C0
/// byte the legacy encoding sends; Alt combinations are dropped, where
/// legacy input reads Alt-x as Esc followed by `x`.
fn kitty_key(params: &[u8]) -> Option<Key> {
    let mut fields = params.split(|&b| b == b';');
    let number = |field: Option<&[u8]>| -> Option<u32> {
        let digits = field?.split(|&b| b == b':').next()?;
        std::str::from_utf8(digits).ok()?.parse().ok()
    };
    let code = number(fields.next())?;
    // Modifiers are sent as 1 + bits: shift 1, alt 2, ctrl 4, super 8, and
    // Caps Lock 64 and Num Lock 128, which don't change the key here.
    let mods = number(fields.next()).unwrap_or(1).saturating_sub(1) & !(64 | 128);
    let (shift, alt, ctrl) = (mods & 1 != 0, mods & 2 != 0, mods & 4 != 0);
    Some(match code {
        27 => Key::Esc,
        13 => Key::Enter,
        9 if shift => Key::BackTab,
        9 => Key::Tab,
        127 | 8 => Key::Backspace,
        _ if alt || mods & !(1 | 4) != 0 => return None,
        _ if ctrl => Key::Ctrl(u8::try_from(code).ok().filter(u8::is_ascii_alphabetic)? & 0x1f),
        _ => Key::Char(char::from_u32(code)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keys_should_read_arrows_and_pages() {
        let keys = parse_keys(b"\x1b[A\x1bOB\x1b[5~\x1b[6~");
        assert_eq!(keys, [Key::Up, Key::Down, Key::PageUp, Key::PageDown]);
    }

    #[test]
    fn parse_keys_should_read_tab_and_shift_tab() {
        assert_eq!(parse_keys(b"\t\x1b[Z"), [Key::Tab, Key::BackTab]);
    }

    #[test]
    fn parse_keys_should_read_kitty_keys() {
        let keys = parse_keys(b"\x1b[27u\x1b[99;5u\x1b[9;2u\x1b[13;1:1u");
        assert_eq!(keys, [Key::Esc, Key::Ctrl(3), Key::BackTab, Key::Enter]);
    }

    #[test]
    fn parse_keys_should_drop_alt_combinations() {
        assert_eq!(parse_keys(b"\x1b[120;3uq"), [Key::Char('q')]);
    }

    #[test]
    fn parse_keys_should_read_modified_legacy_keys() {
        assert_eq!(parse_keys(b"\x1b[1;5A\x1b[5;5~"), [Key::Up, Key::PageUp]);
    }

    #[test]
    fn parse_keys_should_report_theme_change() {
        let keys = parse_keys(b"\x1b[?997;1n\x1b[?997;2nj");
        assert_eq!(keys, [Key::ThemeChanged, Key::ThemeChanged, Key::Char('j')]);
    }

    #[test]
    fn parse_keys_should_read_utf8_chars() {
        let keys = parse_keys("q/ж".as_bytes());
        assert_eq!(keys, [Key::Char('q'), Key::Char('/'), Key::Char('ж')]);
    }
}
