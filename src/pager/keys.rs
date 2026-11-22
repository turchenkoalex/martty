use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;

const POLL_MS: i32 = 200;

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
    Backspace,
    Esc,
    /// SGR mouse event; `col`/`row` are 1-based.
    Mouse {
        button: u16,
        col: u16,
        row: u16,
        press: bool,
    },
}

/// Waits up to `POLL_MS` for input. `None` on timeout.
pub(super) fn read_keys(tty: &mut File) -> io::Result<Option<Vec<Key>>> {
    let mut pfd = libc::pollfd {
        fd: tty.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: a single valid pollfd.
    if unsafe { libc::poll(&mut pfd, 1, POLL_MS) } <= 0 {
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
                    match (fin, params) {
                        (Some(f @ (b'M' | b'm')), [b'<', rest @ ..]) => match sgr_mouse(rest) {
                            Some((button, col, row)) => Key::Mouse {
                                button,
                                col,
                                row,
                                press: f == b'M',
                            },
                            None => continue,
                        },
                        (Some(b'A'), _) => Key::Up,
                        (Some(b'B'), _) => Key::Down,
                        (Some(b'C'), _) => Key::Right,
                        (Some(b'D'), _) => Key::Left,
                        (Some(b'H'), _) => Key::Home,
                        (Some(b'F'), _) => Key::End,
                        (Some(b'~'), b"5") => Key::PageUp,
                        (Some(b'~'), b"6") => Key::PageDown,
                        (Some(b'~'), b"1" | b"7") => Key::Home,
                        (Some(b'~'), b"4" | b"8") => Key::End,
                        _ => continue,
                    }
                }
                _ => Key::Esc,
            },
            b'\r' | b'\n' => Key::Enter,
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

/// `button;col;row` from an SGR mouse report.
pub(super) fn sgr_mouse(params: &[u8]) -> Option<(u16, u16, u16)> {
    let s = std::str::from_utf8(params).ok()?;
    let mut it = s.split(';').map(str::parse::<u16>);
    Some((it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?))
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
    fn parse_keys_should_read_utf8_chars() {
        let keys = parse_keys("q/ж".as_bytes());
        assert_eq!(keys, [Key::Char('q'), Key::Char('/'), Key::Char('ж')]);
    }

    #[test]
    fn parse_keys_should_read_sgr_mouse_press() {
        let keys = parse_keys(b"\x1b[<0;12;5M");
        let expected = Key::Mouse {
            button: 0,
            col: 12,
            row: 5,
            press: true,
        };
        assert_eq!(keys, [expected]);
    }
}
