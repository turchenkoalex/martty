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
        if self.bold {
            out.push_str(";1");
        }
        if self.dim {
            out.push_str(";2");
        }
        if self.italic {
            out.push_str(";3");
        }
        match self.underline {
            Underline::None => {}
            Underline::Single => out.push_str(";4"),
            Underline::Double => out.push_str(";4:2"),
        }
        if self.inverse {
            out.push_str(";7");
        }
        if self.strike {
            out.push_str(";9");
        }
        if let Some(c) = self.fg {
            c.write(out, 38);
        }
        if let Some(c) = self.bg {
            c.write(out, 48);
        }
        if let Some(c) = self.underline_color {
            c.write(out, 58);
        }
        out.push('m');
    }
}
