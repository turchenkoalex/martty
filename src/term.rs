//! Terminal size and capability probing.
//!
//! One round-trip to the terminal answers everything we care about:
//! - kitty text sizing protocol (OSC 66): print a `w=2` space and check
//!   via cursor position report whether the cursor moved by two cells;
//! - foreground/background colors (OSC 10/11) to derive panel colors;
//! - palette colors 1–6 (OSC 4) so image headings and diagrams match the theme
//!   (asked again by `query_theme` when the theme changes);
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

/// The terminal's colors, probed at start and again when Ghostty reports a
/// theme change (DEC 2031).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    /// Base ANSI palette (only the entries we query are filled).
    pub palette: [Option<Rgb>; 8],
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub text_sizing: bool,
    pub theme: Theme,
    pub graphics: bool,
    /// Cell size in pixels, `(width, height)`.
    pub cell: Option<(u32, u32)>,
    /// Keys pressed while the probe was reading the tty, for the pager to
    /// handle first instead of losing them.
    pub pending_input: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
    /// Cell size in pixels, when the kernel knows it.
    pub cell: Option<(u32, u32)>,
}

/// OSC 10/11 (foreground, background) and OSC 4 for palette entries 1–6.
macro_rules! color_query {
    () => {
        "\x1b]10;?\x1b\\\x1b]11;?\x1b\\\
         \x1b]4;1;?\x1b\\\x1b]4;2;?\x1b\\\x1b]4;3;?\x1b\\\x1b]4;4;?\x1b\\\x1b]4;5;?\x1b\\\x1b]4;6;?\x1b\\"
    };
}
const QUERY: &[u8] = concat!(
    "\r\x1b]66;w=2; \x07\x1b[6n",
    color_query!(),
    "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c"
)
.as_bytes();
/// The colors again, with DA1 as the sentinel.
const THEME_QUERY: &[u8] = concat!(color_query!(), "\x1b[c").as_bytes();
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
    let mut caps = query()
        .map(|r| {
            let mut caps = parse_response(&r);
            caps.pending_input = strip_replies(&r);
            caps
        })
        .unwrap_or_default();
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

/// Asks the terminal for its colors again, on a tty already in raw mode.
/// Returns them with the keys typed while waiting for the reply.
pub fn query_theme(tty: &mut File) -> io::Result<(Theme, Vec<u8>)> {
    tty.write_all(THEME_QUERY)?;
    tty.flush()?;
    let response = read_until_da1(tty)?;
    Ok((parse_theme(&response), strip_replies(&response)))
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
        if !wait_readable(tty.as_raw_fd(), left) {
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

/// Waits up to `timeout` for `fd` to have input. `select`, not `poll`: on
/// macOS `poll` doesn't support character devices such as `/dev/tty`; it
/// returns at once with `POLLNVAL` and the following `read` blocks until a
/// key arrives. Interrupted by a signal (`SIGWINCH`), it reports no input.
pub fn wait_readable(fd: RawFd, timeout: Duration) -> bool {
    // FD_SETSIZE on macOS and Linux; libc types it differently per platform.
    const FD_SET_CAPACITY: usize = 1024;
    if !usize::try_from(fd).is_ok_and(|slot| slot < FD_SET_CAPACITY) {
        return false;
    }
    // Field types differ per platform; our timeouts are well under a second.
    let mut tv = libc::timeval {
        tv_sec: timeout.as_secs().min(60) as libc::time_t,
        tv_usec: timeout.subsec_micros() as libc::suseconds_t,
    };
    // SAFETY: an all-zero fd_set is the empty set (what FD_ZERO produces).
    let mut set: libc::fd_set = unsafe { std::mem::zeroed() };
    // SAFETY: `fd` is below FD_SETSIZE, checked above.
    unsafe { libc::FD_SET(fd, &mut set) };
    // SAFETY: one fd_set and a timeval, both owned by this frame.
    let ready = unsafe {
        libc::select(
            fd + 1,
            &mut set,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut tv,
        )
    };
    ready > 0
}

/// Whether `buf` holds a DA1 reply, `CSI ? digits;… c`. Other `CSI ?`
/// sequences (a DEC 2031 theme notification) don't count, nor does a `c`
/// further on, in a color reply's hex digits.
fn has_da1(buf: &[u8]) -> bool {
    let mut rest = buf;
    while let Some(i) = find(rest, b"\x1b[?") {
        rest = &rest[i + 3..];
        let params = rest
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b';')
            .count();
        if rest.get(params) == Some(&b'c') {
            return true;
        }
    }
    false
}

fn parse_response(resp: &[u8]) -> Caps {
    Caps {
        text_sizing: cursor_column(resp) == Some(3),
        theme: parse_theme(resp),
        graphics: find(resp, b"\x1b_Gi=31;OK").is_some(),
        cell: cell_pixels(resp),
        pending_input: Vec::new(),
    }
}

fn parse_theme(resp: &[u8]) -> Theme {
    Theme {
        fg: osc_color(resp, b"]10;rgb:"),
        bg: osc_color(resp, b"]11;rgb:"),
        palette: std::array::from_fn(|i| osc_color(resp, format!("]4;{i};rgb:").as_bytes())),
    }
}

/// What is left of the probe's input once the terminal's replies are cut
/// out: keys the user pressed meanwhile. Replies are OSC (colors), APC
/// (kitty graphics) and CSI ending in `R` (CPR), `t` (cell size) or `c`
/// (DA1); other CSI sequences, like arrow keys, are kept. A reply cut short
/// by the timeout is dropped.
fn strip_replies(buf: &[u8]) -> Vec<u8> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != 0x1b {
            keys.push(buf[i]);
            i += 1;
            continue;
        }
        match buf.get(i + 1) {
            Some(b']' | b'_') => i = string_end(buf, i + 2),
            Some(b'[') => {
                let start = i + 2;
                let Some(len) = buf[start..].iter().position(|c| (0x40..=0x7e).contains(c)) else {
                    break;
                };
                let end = start + len + 1;
                if !matches!(buf[start + len], b'R' | b't' | b'c') {
                    keys.extend_from_slice(&buf[i..end]);
                }
                i = end;
            }
            _ => {
                keys.push(0x1b);
                i += 1;
            }
        }
    }
    keys
}

/// Index just past the BEL or ST that ends an OSC or APC string at `from`.
fn string_end(buf: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < buf.len() {
        match buf[i] {
            0x07 => return i + 1,
            0x1b if buf.get(i + 1) == Some(&b'\\') => return i + 2,
            _ => i += 1,
        }
    }
    buf.len()
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
        if !(1..=4).contains(&hex.len()) {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len())) - 1;
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
    raw: libc::termios,
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
        // SAFETY: tcgetattr succeeded, so `term` is initialised.
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
        Ok(Self { fd, saved, raw })
    }

    /// Gives the terminal back its own mode until the guard drops, for a
    /// program run in the foreground (an editor).
    pub fn pause(&self) -> io::Result<Paused<'_>> {
        set_termios(self.fd, &self.saved)?;
        Ok(Paused(self))
    }
}

/// Raw mode handed back for a while; see [`RawMode::pause`].
pub struct Paused<'a>(&'a RawMode);

impl Drop for Paused<'_> {
    fn drop(&mut self) {
        let _ = set_termios(self.0.fd, &self.0.raw);
    }
}

fn set_termios(fd: RawFd, term: &libc::termios) -> io::Result<()> {
    // SAFETY: `term` is a valid termios read from this terminal.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, term) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
        assert_eq!(
            parse_response(GHOSTTY_LIKE).theme.bg,
            Some((0x28, 0x2c, 0x34))
        );
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
            parse_response(GHOSTTY_LIKE).theme.palette[5],
            Some((0xc6, 0xa0, 0xf6))
        );
    }

    #[test]
    fn strip_replies_should_keep_only_typed_keys() {
        let mut input = b"j".to_vec();
        input.extend_from_slice(GHOSTTY_LIKE);
        input.extend_from_slice(b"\x1b[Bq");
        assert_eq!(strip_replies(&input), b"j\x1b[Bq");
    }

    #[test]
    fn strip_replies_should_drop_reply_cut_short() {
        assert_eq!(strip_replies(b"q\x1b]11;rgb:28"), b"q");
        assert_eq!(strip_replies(b"q\x1b[12;"), b"q");
    }

    #[test]
    fn strip_replies_should_keep_lone_escape() {
        assert_eq!(strip_replies(b"\x1b"), b"\x1b");
    }

    #[test]
    fn has_da1_should_skip_other_private_sequences() {
        assert!(!has_da1(b"\x1b[?997;1n\x1b]10;rgb:1c1c/1c1c/1c1c\x1b\\"));
        assert!(has_da1(b"\x1b[?997;1n\x1b]10;rgb:1c1c\x1b\\\x1b[?62;22c"));
    }

    #[test]
    fn osc_color_should_refuse_overlong_channels() {
        assert_eq!(osc_color(b"]11;rgb:ffffffff/0/0\x07", b"]11;rgb:"), None);
    }

    #[test]
    fn osc_color_should_scale_short_hex_channels() {
        assert_eq!(
            osc_color(b"]11;rgb:f/8/0\x07", b"]11;rgb:"),
            Some((255, 136, 0))
        );
    }
}
