//! `martty --help`, also shown when there is nothing to read, and the pager
//! keys it lists, which the pager's `?` screen shows too.

use std::fmt::Write as _;

use unicode_width::UnicodeWidthStr;

/// Pager keys by topic. With a line for the title, the `?` screen shows them
/// all on a 24-row terminal; keep it that way.
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Moving",
        &[
            ("j k ↑ ↓", "line down / up"),
            ("Space f z b → ←", "page down / up"),
            ("d u", "half page down / up"),
            ("g G", "top / bottom"),
            ("[ ]", "previous / next heading"),
            ("t", "table of contents"),
            ("m<a-z>", "set a mark"),
            ("'<a-z> ''", "go to a mark / back to before the last jump"),
        ],
    ),
    (
        "Search and links",
        &[
            ("/ n N", "search, next / previous match, Esc clears"),
            ("Tab ⇧Tab", "next / previous link on screen"),
            ("Enter", "open the selected link"),
            (
                "⌫ H / B",
                "back from an opened file or diagram / pick how far",
            ),
        ],
    ),
    (
        "Images and diagrams",
        &[
            ("+ - 0", "zoom images and diagrams, reset"),
            ("h l", "pan wide images, code and tables"),
            ("w", "wrap wide code and tables, or pan them"),
            ("o", "diagram on screen full screen; o Space Esc close it"),
            ("← →", "full screen: previous / next diagram"),
        ],
    ),
    (
        "Copying",
        &[(
            "y c C",
            "copy the code on screen / the screen / the document",
        )],
    ),
    (
        "Other",
        &[
            ("s", "show the Markdown source / the rendered document"),
            ("v", "edit the file in $VISUAL or $EDITOR"),
            ("F", "follow changes to the file"),
            ("q", "quit"),
        ],
    ),
];

const OPTIONS: &[(&str, &str)] = &[
    ("-p, --pager", "always open the pager"),
    ("-P, --no-pager", "never open it: print to stdout"),
    (
        "-w, --follow",
        "open the pager and reload FILE when it changes",
    ),
    (
        "    --width N",
        "lay out for N columns instead of the terminal's",
    ),
    ("    --remote", "load https:// images"),
    ("-h, --help", "show this help"),
    ("-V, --version", "show the version"),
];

const ENVIRONMENT: &[(&str, &str)] = &[
    ("MARTTY_IMAGES=0|1", "turn images off, or force them on"),
    (
        "MARTTY_TEXT_SIZING=0|1",
        "turn OSC 66 headings off, or force them",
    ),
    ("MARTTY_NO_COLOR=1", "no colors, and so no images"),
    ("MARTTY_REMOTE=1", "load https:// images, as --remote does"),
];

/// The help text; `styled` adds bold titles and colored keys (SGR).
pub fn text(styled: bool) -> String {
    let (bold, accent, dim, reset) = if styled {
        ("\x1b[1m", "\x1b[36m", "\x1b[2m", "\x1b[0m")
    } else {
        ("", "", "", "")
    };
    let title = |out: &mut String, name: &str| {
        let _ = writeln!(out, "\n{bold}{name}{reset}");
    };
    let rows = |out: &mut String, rows: &[(&str, &str)], indent: &str, width: usize| {
        for (name, what) in rows {
            let pad = width.saturating_sub(name.width());
            let _ = writeln!(out, "{indent}{accent}{name}{reset}{:pad$}  {what}", "");
        }
    };
    let width = |rows: &[(&str, &str)]| rows.iter().map(|(n, _)| n.width()).max().unwrap_or(0);

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{bold}martty{reset} {}  {dim}Markdown viewer for Ghostty{reset}",
        env!("CARGO_PKG_VERSION")
    );
    title(&mut out, "Usage");
    let _ = writeln!(out, "  martty [options] FILE     view a Markdown file");
    let _ = writeln!(out, "  martty [options] URL      download one and view it");
    let _ = writeln!(out, "  … | martty [options]      read Markdown from stdin");
    let _ = writeln!(
        out,
        "\n  Output taller than the screen opens a less-like pager."
    );
    title(&mut out, "Options");
    rows(&mut out, OPTIONS, "  ", width(OPTIONS));
    title(&mut out, "Environment");
    rows(&mut out, ENVIRONMENT, "  ", width(ENVIRONMENT));
    let _ = writeln!(
        out,
        "\n{bold}Pager keys{reset}  {dim}? shows them in the pager{reset}"
    );
    let key_width = KEYS.iter().map(|(_, keys)| width(keys)).max().unwrap_or(0);
    for (i, (topic, keys)) in KEYS.iter().enumerate() {
        let gap = if i == 0 { "" } else { "\n" };
        let _ = writeln!(out, "{gap}  {dim}{topic}{reset}");
        rows(&mut out, keys, "    ", key_width);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_should_fit_eighty_columns() {
        let help = text(false);
        let long: Vec<&str> = help.lines().filter(|l| l.width() > 80).collect();
        assert!(long.is_empty(), "{long:?}");
    }

    #[test]
    fn text_should_be_plain_unless_styled() {
        assert!(!text(false).contains('\x1b'));
        assert!(text(true).contains("\x1b[1mOptions\x1b[0m"));
    }

    #[test]
    fn keys_should_fit_help_screen_of_24_rows() {
        let count: usize = KEYS.iter().map(|(_, keys)| keys.len()).sum();
        assert!(count <= 22, "{count} keys: with the title over 23 rows");
    }

    #[test]
    fn text_should_list_every_key() {
        let help = text(false);
        for (_, keys) in KEYS {
            for (key, what) in *keys {
                assert!(help.contains(key), "{key}");
                assert!(help.contains(what), "{what}");
            }
        }
    }
}
