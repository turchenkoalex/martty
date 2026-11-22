//! Terminal size and capability probing.
//!
//! One round-trip to the terminal answers everything we care about:
//! - kitty text sizing protocol (OSC 66): print a `w=2` space and check
//!   via cursor position report whether the cursor moved by two cells;
//! - foreground/background colors (OSC 10/11) to derive panel colors;
//! - palette colors 1–6 (OSC 4) so image headings and diagrams match the theme;
//! - kitty graphics support (`a=q` query) and cell size in pixels (CSI 16 t);
//! - DA1 as a sentinel, since every terminal answers it.

use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

pub type Rgb = (u8, u8, u8);

/// Perceived brightness, 0–255.
pub fn luma(c: Rgb) -> f32 {
    0.299 * f32::from(c.0) + 0.587 * f32::from(c.1) + 0.114 * f32::from(c.2)
}

/// Linear blend from `a` (t = 0) to `b` (t = 1).
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let ch = |x: u8, y: u8| {
        let v = f32::from(x) + (f32::from(y) - f32::from(x)) * t;
        v.round().clamp(0.0, 255.0) as u8
    };
    (ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub text_sizing: bool,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub graphics: bool,
    /// Cell size in pixels, `(width, height)`.
    pub cell: Option<(u32, u32)>,
    /// Base ANSI palette (only the entries we query are filled).
    pub palette: [Option<Rgb>; 8],
}

#[derive(Debug, Clone, Copy)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
    /// Cell size in pixels, when the kernel knows it.
    pub cell: Option<(u32, u32)>,
}

const QUERY: &[u8] = b"\r\x1b]66;w=2; \x07\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\
    \x1b]4;1;?\x1b\\\x1b]4;2;?\x1b\\\x1b]4;3;?\x1b\\\x1b]4;4;?\x1b\\\x1b]4;5;?\x1b\\\x1b]4;6;?\x1b\\\
    \x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c";
const TIMEOUT: Duration = Duration::from_millis(500);

pub fn size() -> Size {
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: TIOCGWINSZ writes into the zeroed winsize we own.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0;
        if ok && ws.ws_col > 0 && ws.ws_row > 0 {
            let cell = (ws.ws_xpixel > 0 && ws.ws_ypixel > 0).then(|| {
                (
                    u32::from(ws.ws_xpixel / ws.ws_col),
                    u32::from(ws.ws_ypixel / ws.ws_row),
                )
            });
            return Size {
                cols: usize::from(ws.ws_col),
                rows: usize::from(ws.ws_row),
                cell,
            };
        }
    }
    let cols = std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(80);
    Size {
        cols,
        rows: 24,
        cell: None,
    }
}

pub fn probe() -> Caps {
    if !io::stdout().is_terminal() {
        return Caps::default();
    }
    let mut caps = query().map(|r| parse_response(&r)).unwrap_or_default();
    if let Ok(v) = std::env::var("MARTTY_TEXT_SIZING") {
        caps.text_sizing = v == "1";
    }
    if let Ok(v) = std::env::var("MARTTY_IMAGES") {
        caps.graphics = v == "1";
    }
    caps
}

fn query() -> io::Result<Vec<u8>> {
    let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    // Declared after `tty`, so it is dropped (terminal restored) first.
    let _raw = RawMode::enter(tty.as_raw_fd())?;
    tty.write_all(QUERY)?;
    tty.flush()?;
    let response = read_until_da1(&mut tty)?;
    tty.write_all(b"\r\x1b[2K")?;
    Ok(response)
}

fn read_until_da1(tty: &mut File) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + TIMEOUT;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 256];
    while !has_da1(&buf) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd {
            fd: tty.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: a single valid pollfd.
        if unsafe { libc::poll(&mut pfd, 1, ms) } <= 0 {
            break;
        }
        let n = tty.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

fn has_da1(buf: &[u8]) -> bool {
    find(buf, b"\x1b[?").is_some_and(|i| buf[i..].contains(&b'c'))
}

fn parse_response(resp: &[u8]) -> Caps {
    Caps {
        text_sizing: cursor_column(resp) == Some(3),
        fg: osc_color(resp, b"]10;rgb:"),
        bg: osc_color(resp, b"]11;rgb:"),
        graphics: find(resp, b"\x1b_Gi=31;OK").is_some(),
        cell: cell_pixels(resp),
        palette: std::array::from_fn(|i| osc_color(resp, format!("]4;{i};rgb:").as_bytes())),
    }
}

/// Cell size from a `CSI 6 ; height ; width t` reply.
fn cell_pixels(resp: &[u8]) -> Option<(u32, u32)> {
    let start = find(resp, b"\x1b[6;")? + 4;
    let rest = &resp[start..];
    let end = rest.iter().position(|b| *b == b't')?;
    let body = std::str::from_utf8(&rest[..end]).ok()?;
    let (h, w) = body.split_once(';')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Column from a CPR reply `ESC [ row ; col R`.
fn cursor_column(resp: &[u8]) -> Option<u32> {
    let mut rest = resp;
    while let Some(start) = find(rest, b"\x1b[") {
        rest = &rest[start + 2..];
        let end = rest
            .iter()
            .position(|b| !(b.is_ascii_digit() || *b == b';'))?;
        if rest[end] == b'R' {
            let body = std::str::from_utf8(&rest[..end]).ok()?;
            return body.split(';').nth(1)?.parse().ok();
        }
    }
    None
}

/// Parses `rgb:rrrr/gggg/bbbb` (1–4 hex digits per channel).
fn osc_color(resp: &[u8], prefix: &[u8]) -> Option<Rgb> {
    let start = find(resp, prefix)? + prefix.len();
    let rest = &resp[start..];
    let end = rest.iter().position(|b| *b == 0x1b || *b == 0x07)?;
    let body = std::str::from_utf8(&rest[..end]).ok()?;
    let mut channels = body.split('/').map(|hex| {
        let v = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len().min(4))) - 1;
        u8::try_from(v * 255 / max).ok()
    });
    Some((channels.next()??, channels.next()??, channels.next()??))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Disables canonical mode and echo for the lifetime of the guard.
pub struct RawMode {
    fd: RawFd,
    saved: libc::termios,
}

impl RawMode {
    fn enter(fd: RawFd) -> io::Result<Self> {
        Self::with(fd, |_| {})
    }

    /// Raw input for an interactive pager: also no signals (Ctrl-C arrives
    /// as a byte, so the terminal is always restored) and no flow control.
    pub fn keys(fd: RawFd) -> io::Result<Self> {
        Self::with(fd, |t| {
            t.c_lflag &= !(libc::ISIG | libc::IEXTEN);
            t.c_iflag &= !(libc::IXON | libc::ICRNL);
        })
    }

    fn with(fd: RawFd, tweak: impl FnOnce(&mut libc::termios)) -> io::Result<Self> {
        let mut term = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr fully initialises `term` on success.
        if unsafe { libc::tcgetattr(fd, term.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let saved = unsafe { term.assume_init() };
        let mut raw = saved;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        tweak(&mut raw);
        // SAFETY: `raw` is a valid termios copied from the current one.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: restoring the termios we read in `enter`.
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GHOSTTY_LIKE: &[u8] =
        b"\x1b[12;3R\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:2828/2c2c/3434\x1b\\\
          \x1b]4;5;rgb:c6c6/a0a0/f6f6\x1b\\\
          \x1b_Gi=31;OK\x1b\\\x1b[6;38;17t\x1b[?62;22c";

    #[test]
    fn parse_should_detect_text_sizing_when_cursor_moved_two_cells() {
        assert!(parse_response(GHOSTTY_LIKE).text_sizing);
    }

    #[test]
    fn parse_should_reject_text_sizing_when_cursor_stayed() {
        assert!(!parse_response(b"\x1b[12;1R\x1b[?62c").text_sizing);
    }

    #[test]
    fn parse_should_read_background_color() {
        assert_eq!(parse_response(GHOSTTY_LIKE).bg, Some((0x28, 0x2c, 0x34)));
    }

    #[test]
    fn parse_should_detect_kitty_graphics() {
        assert!(parse_response(GHOSTTY_LIKE).graphics);
    }

    #[test]
    fn parse_should_read_cell_size_as_width_height() {
        assert_eq!(parse_response(GHOSTTY_LIKE).cell, Some((17, 38)));
    }

    #[test]
    fn parse_should_read_palette_entry() {
        assert_eq!(
            parse_response(GHOSTTY_LIKE).palette[5],
            Some((0xc6, 0xa0, 0xf6))
        );
    }

    #[test]
    fn osc_color_should_scale_short_hex_channels() {
        assert_eq!(
            osc_color(b"]11;rgb:f/8/0\x07", b"]11;rgb:"),
            Some((255, 136, 0))
        );
    }
}
