//! SGR styling. Ghostty understands truecolor, styled underlines (`4:n`)
//! and underline colors (`58`), so we use them freely.

use std::fmt::Write as _;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// Palette index: follows the user's Ghostty theme.
    Ansi(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    fn write(self, out: &mut String, base: u8) {
        let _ = match self {
            Color::Ansi(n) => write!(out, ";{base};5;{n}"),
            Color::Rgb(r, g, b) => write!(out, ";{base};2;{r};{g};{b}"),
        };
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub underline_color: Option<Color>,
    pub underline: Underline,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub strike: bool,
    /// Reverse video (SGR 7).
    pub inverse: bool,
    /// OSC 8 hyperlink target.
    pub link: Option<Rc<str>>,
}

impl Style {
    pub fn fg(color: Color) -> Self {
        Self {
            fg: Some(color),
            ..Self::default()
        }
    }

    pub fn dim() -> Self {
        Self {
            dim: true,
            ..Self::default()
        }
    }

    /// Writes a full SGR sequence, always starting from a reset.
    pub fn write_sgr(&self, out: &mut String) {
        out.push_str("\x1b[0");
        self.write_params(&Style::default(), out);
        out.push('m');
    }

    /// Writes the SGR sequence that turns `prev` into this style: only what
    /// changed, or a reset and the full style when that is shorter. Ignores
    /// `link`, which is OSC 8, not SGR.
    pub fn write_sgr_from(&self, prev: &Style, out: &mut String) {
        let mut diff = String::new();
        self.write_params(prev, &mut diff);
        if diff.is_empty() {
            return;
        }
        let mut full = String::from("0");
        self.write_params(&Style::default(), &mut full);
        out.push_str("\x1b[");
        // `diff` starts with the separator `;`.
        out.push_str(if full.len() < diff.len() - 1 {
            &full
        } else {
            &diff[1..]
        });
        out.push('m');
    }

    /// `;`-prefixed SGR parameters that take the terminal from `prev` to this style.
    fn write_params(&self, prev: &Style, out: &mut String) {
        // SGR 22 clears bold and dim together, so dropping either one
        // drops both and the other is set again below.
        let intensity_off = (prev.bold && !self.bold) || (prev.dim && !self.dim);
        if intensity_off {
            out.push_str(";22");
        }
        if self.bold && (intensity_off || !prev.bold) {
            out.push_str(";1");
        }
        if self.dim && (intensity_off || !prev.dim) {
            out.push_str(";2");
        }
        flag(out, prev.italic, self.italic, ";3", ";23");
        if prev.underline != self.underline {
            out.push_str(match self.underline {
                Underline::None => ";24",
                Underline::Single => ";4",
                Underline::Double => ";4:2",
            });
        }
        flag(out, prev.inverse, self.inverse, ";7", ";27");
        flag(out, prev.strike, self.strike, ";9", ";29");
        color(out, prev.fg, self.fg, 38);
        color(out, prev.bg, self.bg, 48);
        color(out, prev.underline_color, self.underline_color, 58);
    }

    /// The style with colors removed, for `NO_COLOR`.
    pub fn without_colors(&self) -> Self {
        Self {
            fg: None,
            bg: None,
            underline_color: None,
            ..self.clone()
        }
    }
}

fn flag(out: &mut String, prev: bool, next: bool, on: &str, off: &str) {
    if prev != next {
        out.push_str(if next { on } else { off });
    }
}

/// `base` is 38, 48 or 58; base + 1 resets that color to the default.
fn color(out: &mut String, prev: Option<Color>, next: Option<Color>, base: u8) {
    if prev == next {
        return;
    }
    match next {
        Some(c) => c.write(out, base),
        None => {
            let _ = write!(out, ";{}", base + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr_from(prev: &Style, next: &Style) -> String {
        let mut out = String::new();
        next.write_sgr_from(prev, &mut out);
        out
    }

    fn bold() -> Style {
        Style {
            bold: true,
            ..Style::default()
        }
    }

    #[test]
    fn sgr_from_should_write_nothing_for_same_style() {
        assert_eq!(sgr_from(&bold(), &bold()), "");
    }

    #[test]
    fn sgr_from_should_write_only_added_attribute() {
        let next = Style {
            italic: true,
            ..bold()
        };
        assert_eq!(sgr_from(&bold(), &next), "\x1b[3m");
    }

    #[test]
    fn sgr_from_should_reset_single_color() {
        let prev = Style {
            fg: Some(Color::Ansi(4)),
            ..bold()
        };
        assert_eq!(sgr_from(&prev, &bold()), "\x1b[39m");
    }

    #[test]
    fn sgr_from_should_keep_dim_when_dropping_bold() {
        let next = Style {
            fg: Some(Color::Ansi(4)),
            ..Style::dim()
        };
        let prev = Style {
            bold: true,
            ..next.clone()
        };
        assert_eq!(sgr_from(&prev, &next), "\x1b[22;2m");
    }

    #[test]
    fn sgr_from_should_reset_when_shorter() {
        let prev = Style {
            italic: true,
            strike: true,
            fg: Some(Color::Rgb(1, 2, 3)),
            ..bold()
        };
        assert_eq!(sgr_from(&prev, &Style::default()), "\x1b[0m");
    }
}
