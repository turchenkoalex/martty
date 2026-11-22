//! Cutting a column range out of a line of ANSI text, for panning code and
//! tables wider than the screen.

use unicode_width::UnicodeWidthChar;

use crate::kitty;

/// The cells `from..from + count` of `text`. Escape sequences are all
/// kept, wherever they are, so styles and OSC 8 links stay balanced; a wide
/// character cut by an edge becomes spaces.
pub fn slice_cells(text: &str, from: usize, count: usize) -> String {
    let end = from + count;
    let mut out = String::with_capacity(text.len());
    let mut col = 0;
    let mut chars = text.char_indices().peekable();
    // Whether the last visible character was kept, for the combining marks after it.
    let mut kept = false;
    while let Some((i, c)) = chars.next() {
        if c == '\x1b' {
            let len = escape_len(&text[i..]);
            out.push_str(&text[i..i + len]);
            while chars.next_if(|(j, _)| *j < i + len).is_some() {}
            continue;
        }
        let width = if c == kitty::PLACEHOLDER {
            1
        } else {
            c.width().unwrap_or(0)
        };
        if width == 0 {
            if kept {
                out.push(c);
            }
            continue;
        }
        let (start, stop) = (col, col + width);
        col = stop;
        kept = start >= from && stop <= end;
        if kept {
            out.push(c);
        } else if start < end && stop > from {
            let shown = stop.min(end) - start.max(from);
            out.extend(std::iter::repeat_n(' ', shown));
        }
    }
    out
}

/// Length in bytes of the escape sequence `s` starts with: CSI up to its
/// final byte, OSC/APC up to BEL or ST, otherwise ESC and one character.
fn escape_len(s: &str) -> usize {
    let bytes = s.as_bytes();
    match bytes.get(1) {
        Some(b'[') => bytes[2..]
            .iter()
            .position(|b| (0x40..=0x7e).contains(b))
            .map_or(bytes.len(), |p| p + 3),
        Some(b']' | b'_') => {
            let mut i = 2;
            while i < bytes.len() {
                match bytes[i] {
                    0x07 => return i + 1,
                    0x1b if bytes.get(i + 1) == Some(&b'\\') => return i + 2,
                    _ => i += 1,
                }
            }
            bytes.len()
        }
        Some(_) => 1 + s[1..].chars().next().map_or(0, char::len_utf8),
        None => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_cells_should_cut_columns_and_keep_escapes() {
        let text = "\x1b[1mabc\x1b[0mdef";
        assert_eq!(slice_cells(text, 2, 3), "\x1b[1mc\x1b[0mde");
    }

    #[test]
    fn slice_cells_should_keep_links_balanced() {
        let text = "ab\x1b]8;;u\x1b\\cd\x1b]8;;\x1b\\ef";
        assert_eq!(slice_cells(text, 4, 2), "\x1b]8;;u\x1b\\\x1b]8;;\x1b\\ef");
    }

    #[test]
    fn slice_cells_should_pad_cut_wide_characters() {
        assert_eq!(slice_cells("a漢b", 2, 2), " b");
        assert_eq!(slice_cells("a漢b", 0, 2), "a ");
    }
}
