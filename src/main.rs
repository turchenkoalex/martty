//! martty — a tiny Markdown renderer tuned for Ghostty.
//!
//! Usage: `martty [-p|-P] [-w] [--width N] [FILE]` (reads stdin when FILE is
//! absent or `-`; with neither, prints the help from `help.rs`). Output taller than the screen opens a `less`-like pager;
//! `-p` forces it, `-P` disables it. `-w` opens the pager and follows changes
//! to FILE. `--width` lays text out for N columns instead of the terminal
//! width. `--remote` (or `MARTTY_REMOTE=1`) loads https images; it is off
//! otherwise. `-V` prints the version. Set `MARTTY_TEXT_SIZING=0|1` / `MARTTY_IMAGES=0|1`
//! to override capability detection; `MARTTY_NO_COLOR=1` turns off colors and
//! images. Every setting martty reads from the environment has the `MARTTY_`
//! prefix.

mod cache;
mod diacritics;
mod heading;
mod help;
mod kitty;
mod math;
mod mermaid;
mod pager;
mod panics;
mod picture;
mod progress;
mod remote;
mod render;
mod style;
mod term;

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use heading::HeadingFont;
use pager::{Layout, Params};
use render::{Config, Diagrams, Document, HeadingImages, Images, MathImages};
use style::Color;
use term::{Caps, Rgb, Size, Theme, luma, mix};

const USAGE: &str = "usage: martty [options] [FILE]; martty --help lists them";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Paging {
    Auto,
    Always,
    Never,
}

struct Args {
    paging: Paging,
    /// Reload FILE when it changes (`-w`).
    follow: bool,
    /// Load https images (`--remote`, `MARTTY_REMOTE=1`).
    remote: bool,
    /// Layout width in cells; the terminal width when absent.
    width: Option<usize>,
    file: Option<PathBuf>,
}

fn main() -> ExitCode {
    panics::install();
    // Unwound out of the pager, a panic has restored the terminal; its
    // message was kept until now.
    let result = std::panic::catch_unwind(run).unwrap_or_else(|_| {
        let message = panics::take_deferred().unwrap_or_else(|| "internal error".into());
        Err(io::Error::other(message))
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("martty: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let args = parse_args()?;
    // Nothing to read: no file, and stdin is the keyboard.
    let no_file = args.file.as_deref().is_none_or(|f| f == Path::new("-"));
    if no_file && io::stdin().is_terminal() {
        print_help();
        return Ok(());
    }
    if args.remote {
        remote::enable();
    }
    let (mut markdown, base_dir) = read_input(args.file.as_deref())?;
    let title = args
        .file
        .as_deref()
        .and_then(Path::file_name)
        .map_or_else(|| "stdin".to_owned(), |n| n.to_string_lossy().into_owned());

    // Both overlap the terminal probe.
    if io::stdout().is_terminal() {
        HeadingFont::prefetch();
    }
    render::warm_up(&markdown);
    let mut caps = term::probe();
    // Images go too: placeholders carry the image id in their foreground
    // color, and diagrams and image headings are colored anyway. Without a
    // background, code blocks get a bar instead of a panel.
    let color = std::env::var_os("MARTTY_NO_COLOR").is_none_or(|v| v != "1");
    if !color {
        caps.graphics = false;
    }
    let pending_input = std::mem::take(&mut caps.pending_input);
    let mut next_id = image_id_seed();
    let file = args.file.as_deref().filter(|f| *f != Path::new("-"));
    let mut render = |layout: Layout<'_>, params: Params| {
        let Params {
            mut size,
            zoom,
            overflow,
            mut theme,
        } = params;
        if !color {
            theme.bg = None;
        }
        let _busy = progress::Busy::start();
        if let Some(width) = args.width {
            size.cols = width;
        }
        let first_id = next_id;
        let doc = match layout {
            Layout::Document => {
                let mut config = config(&caps, &theme, size, &base_dir, first_id, zoom, false);
                config.color = color;
                config.overflow = overflow;
                // Read again on every layout, so following picks up edits;
                // a file that can't be read right now (mid-save) keeps the
                // text read before.
                if let Some(text) = file.and_then(|f| read_text(f).ok()) {
                    markdown = text;
                }
                render::render(&markdown, &config)
            }
            Layout::File(path) => {
                // A link may point at a file that can't be read; say so in
                // the view instead of failing the pager.
                let markdown = read_text(path).unwrap_or_else(|e| {
                    format!("> [!CAUTION]\n> Can't read `{}`: {e}\n", path.display())
                });
                let dir = path.parent().unwrap_or(Path::new("."));
                let mut config = config(&caps, &theme, size, dir, first_id, zoom, false);
                config.color = color;
                config.overflow = overflow;
                render::render(&markdown, &config)
            }
            Layout::Diagram(source) => {
                let mut config = config(&caps, &theme, size, &base_dir, first_id, zoom, true);
                config.color = color;
                render::render(&mermaid_block(source), &config)
            }
        };
        // Ids are never reused while martty runs, so a layout's images can't
        // replace those of a view still shown.
        let used = u32::try_from(doc.image_ids.len()).unwrap_or(u32::MAX);
        next_id = next_id.wrapping_add(used);
        doc
    };

    let size = term::size();
    // Taken before the file is read for layout, so an edit saved meanwhile
    // still counts as a change when following.
    let modified = file.and_then(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok());
    let params = Params {
        size,
        zoom: 1.0,
        overflow: false,
        theme: caps.theme,
    };
    let doc = render(Layout::Document, params);
    let page = io::stdout().is_terminal()
        && match args.paging {
            _ if args.follow => true,
            Paging::Always => true,
            Paging::Never => false,
            Paging::Auto => doc.lines.len() >= size.rows,
        };
    if page {
        let options = pager::Options {
            title: &title,
            file,
            follow: args.follow,
            theme: caps.theme,
            modified,
        };
        return pager::run(&options, doc, &pending_input, render);
    }
    print(doc)
}

fn print(doc: Document) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(doc.into_ansi().as_bytes())?;
    stdout.flush()
}

fn parse_args() -> io::Result<Args> {
    let mut args = Args {
        paging: Paging::Auto,
        follow: false,
        remote: std::env::var_os("MARTTY_REMOTE").is_some_and(|v| v == "1"),
        width: None,
        file: None,
    };
    let mut argv = std::env::args_os().skip(1);
    while let Some(arg) = argv.next() {
        match arg.to_str() {
            Some("--width") => {
                let value = argv.next();
                args.width = Some(parse_width(value.as_ref().and_then(|v| v.to_str()))?);
            }
            Some(flag) if flag.starts_with("--width=") => {
                args.width = Some(parse_width(flag.strip_prefix("--width="))?);
            }
            Some("-p" | "--pager") => args.paging = Paging::Always,
            Some("-P" | "--no-pager") => args.paging = Paging::Never,
            Some("-w" | "--follow") => args.follow = true,
            Some("--remote") => args.remote = true,
            Some("-h" | "--help") => {
                print_help();
                std::process::exit(0);
            }
            // The release workflow checks that the tag matches this version.
            Some("-V" | "--version") => {
                println!("martty {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            Some(flag) if flag.starts_with('-') && flag != "-" => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown option {flag}\n{USAGE}"),
                ));
            }
            _ => args.file = Some(PathBuf::from(arg)),
        }
    }
    Ok(args)
}

/// Styled for a terminal, plain in a pipe or under `MARTTY_NO_COLOR`.
fn print_help() {
    let color = std::env::var_os("MARTTY_NO_COLOR").is_none_or(|v| v != "1");
    print!("{}", help::text(color && io::stdout().is_terminal()));
}

fn parse_width(value: Option<&str>) -> io::Result<usize> {
    value
        .and_then(|v| v.parse().ok())
        .filter(|&w| w > 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("--width needs a positive number of columns\n{USAGE}"),
            )
        })
}

/// Markdown text plus the directory relative image paths resolve against.
fn read_input(file: Option<&Path>) -> io::Result<(String, PathBuf)> {
    match file {
        Some(path) if path != Path::new("-") => {
            let text = read_text(path).map_err(|e| with_path(e, path))?;
            let dir = path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            Ok((text, dir))
        }
        _ => {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf)?;
            Ok((lossy(buf), PathBuf::from(".")))
        }
    }
}

/// `fullscreen` lays out a lone diagram: no cap on its text size.
fn config(
    caps: &Caps,
    theme: &Theme,
    size: Size,
    base_dir: &Path,
    first_image_id: u32,
    zoom: f32,
    fullscreen: bool,
) -> Config {
    let cell = caps.graphics.then(|| caps.cell.or(size.cell)).flatten();
    let bg = theme.bg.unwrap_or((0x1e, 0x1e, 0x1e));
    let fg = theme.fg.unwrap_or_else(|| contrast(bg));
    let max_rows = u32::try_from(size.rows * 2 / 3)
        .unwrap_or(u32::MAX)
        .clamp(4, kitty::MAX_ROWS);

    let headings = cell.filter(|_| !caps.text_sizing).and_then(|cell| {
        Some(HeadingImages {
            font: HeadingFont::load()?,
            cell,
            colors: [5, 4, 6].map(|i| theme.palette[i].unwrap_or(fg)),
        })
    });
    let diagrams = cell.map(|cell| Diagrams {
        palette: mermaid::Palette {
            bg,
            fg,
            panel: panel_rgb(bg, fg),
            accent: theme.palette[4].unwrap_or(fg),
            ansi: std::array::from_fn(|i| theme.palette[i + 1].unwrap_or(fg)),
        },
        cell,
        // Full screen, a diagram at zoom 1 fills the screen above the pager
        // status line. In the document it fits the width only and keeps the
        // terminal's text size, however many rows that takes (a tall one is
        // scrolled, not shrunk to unreadable text); `o` opens it whole.
        max_rows: if fullscreen {
            u32::try_from(size.rows.saturating_sub(1))
                .unwrap_or(u32::MAX)
                .clamp(1, kitty::MAX_ROWS)
        } else {
            kitty::MAX_ROWS
        },
        zoom,
        max_upscale: if fullscreen { f32::INFINITY } else { 1.0 },
        // The pager centres a full-screen diagram itself.
        center: !fullscreen,
    });
    let math = cell.map(|cell| MathImages { cell, color: fg });
    let images = cell.map(|cell| Images {
        cell,
        max_rows,
        zoom,
    });
    Config {
        width: size.cols,
        color: true,
        overflow: false,
        text_sizing: caps.text_sizing,
        code_bg: theme.bg.map(|bg| {
            let (r, g, b) = panel_rgb(bg, fg);
            Color::Rgb(r, g, b)
        }),
        images,
        headings,
        diagrams,
        math,
        first_image_id,
        base_dir: base_dir.to_path_buf(),
    }
}

/// Image ids live in the terminal, shared with every other program in it;
/// start from a per-process value to make collisions unlikely.
fn image_id_seed() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    ((std::process::id().wrapping_mul(2_654_435_761) ^ nanos) & 0x00FF_FFFF).max(1)
}

/// A text file, with invalid UTF-8 (a Latin-1 file) shown as `�` rather
/// than refused.
fn read_text(path: &Path) -> io::Result<String> {
    std::fs::read(path).map(lossy)
}

fn lossy(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// `source` as a fenced mermaid block, the fence longer than any run of
/// backticks inside it.
fn mermaid_block(source: &str) -> String {
    let longest = source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}mermaid\n{source}\n{fence}\n")
}

fn with_path(e: io::Error, path: &Path) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

/// Background for code and diagrams: the terminal background nudged ~8%
/// towards the foreground, so it fits whatever Ghostty theme is active.
fn panel_rgb(bg: Rgb, fg: Rgb) -> Rgb {
    mix(bg, fg, 0.08)
}

fn contrast(bg: Rgb) -> Rgb {
    if luma(bg) < 128.0 {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mermaid_block_should_outfence_backticks_in_the_source() {
        assert_eq!(mermaid_block("A"), "```mermaid\nA\n```\n");
        assert_eq!(
            mermaid_block("A[\"```\"]"),
            "````mermaid\nA[\"```\"]\n````\n"
        );
    }

    #[test]
    fn lossy_should_keep_latin1_text_readable() {
        assert_eq!(lossy(b"caf\xe9".to_vec()), "caf\u{fffd}");
    }
}
