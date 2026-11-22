//! martty — a tiny Markdown renderer tuned for Ghostty.
//!
//! Usage: `martty [-p|-P] [--width N] [FILE]` (reads stdin when FILE is absent
//! or `-`). Output taller than the screen opens a `less`-like pager; `-p`
//! forces it, `-P` disables it. `--width` lays text out for N columns instead
//! of the terminal width. Set `MARTTY_TEXT_SIZING=0|1` / `MARTTY_IMAGES=0|1`
//! to override capability detection; `NO_COLOR` turns off colors and images.

mod diacritics;
mod heading;
mod kitty;
mod math;
mod mermaid;
mod pager;
mod render;
mod style;
mod term;

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use heading::HeadingFont;
use pager::Layout;
use render::{Config, Diagrams, Document, HeadingImages, Images, MathImages};
use style::Color;
use term::{Caps, Rgb, Size, luma, mix};

const USAGE: &str = "usage: martty [-p|--pager] [-P|--no-pager] [--width N] [FILE]";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Paging {
    Auto,
    Always,
    Never,
}

struct Args {
    paging: Paging,
    /// Layout width in cells; the terminal width when absent.
    width: Option<usize>,
    file: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
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
    let (markdown, base_dir) = read_input(args.file.as_deref())?;
    let title = args
        .file
        .as_deref()
        .and_then(Path::file_name)
        .map_or_else(|| "stdin".to_owned(), |n| n.to_string_lossy().into_owned());

    let mut caps = term::probe();
    // https://no-color.org: any non-empty value. Images go too: placeholders
    // carry the image id in their foreground color, and diagrams and image
    // headings are colored anyway. Without a background, code blocks get a
    // bar instead of a panel.
    let color = std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    if !color {
        caps.graphics = false;
        caps.bg = None;
    }
    let pending_input = std::mem::take(&mut caps.pending_input);
    let mut next_id = image_id_seed();
    let mut render = |layout: Layout<'_>, mut size: Size, zoom: f32| {
        if let Some(width) = args.width {
            size.cols = width;
        }
        let first_id = next_id;
        // Fresh ids per layout, so a re-render never reuses live images.
        next_id = next_id.wrapping_add(0x1000);
        match layout {
            Layout::Document => {
                let mut config = config(&caps, size, &base_dir, first_id, zoom, false);
                config.color = color;
                render::render(&markdown, &config)
            }
            Layout::Diagram(source) => {
                let mut config = config(&caps, size, &base_dir, first_id, zoom, true);
                config.color = color;
                render::render(&format!("```mermaid\n{source}\n```\n"), &config)
            }
        }
    };

    let size = term::size();
    let doc = render(Layout::Document, size, 1.0);
    let page = io::stdout().is_terminal()
        && match args.paging {
            Paging::Always => true,
            Paging::Never => false,
            Paging::Auto => doc.lines.len() >= size.rows,
        };
    if page {
        return pager::run(&title, doc, &pending_input, render);
    }
    print(&doc)
}

fn print(doc: &Document) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(doc.to_ansi().as_bytes())?;
    stdout.flush()
}

fn parse_args() -> io::Result<Args> {
    let mut args = Args {
        paging: Paging::Auto,
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
            Some("-h" | "--help") => {
                println!("{USAGE}");
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
            let text = std::fs::read_to_string(path).map_err(|e| with_path(e, path))?;
            let dir = path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            Ok((text, dir))
        }
        _ if io::stdin().is_terminal() => Err(io::Error::new(io::ErrorKind::InvalidInput, USAGE)),
        _ => {
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            Ok((buf, PathBuf::from(".")))
        }
    }
}

/// `fullscreen` lays out a lone diagram: no cap on its text size.
fn config(
    caps: &Caps,
    size: Size,
    base_dir: &Path,
    first_image_id: u32,
    zoom: f32,
    fullscreen: bool,
) -> Config {
    let cell = caps.graphics.then(|| caps.cell.or(size.cell)).flatten();
    let bg = caps.bg.unwrap_or((0x1e, 0x1e, 0x1e));
    let fg = caps.fg.unwrap_or_else(|| contrast(bg));
    let max_rows = u32::try_from(size.rows * 2 / 3)
        .unwrap_or(u32::MAX)
        .clamp(4, kitty::MAX_ROWS);

    let headings = cell.filter(|_| !caps.text_sizing).and_then(|cell| {
        Some(HeadingImages {
            font: HeadingFont::load()?,
            cell,
            colors: [5, 4, 6].map(|i| caps.palette[i].unwrap_or(fg)),
        })
    });
    // Without the feature, mermaid blocks are shown as code.
    let diagrams = cell
        .filter(|_| cfg!(feature = "mermaid"))
        .map(|cell| Diagrams {
            palette: mermaid::Palette {
                bg,
                fg,
                panel: panel_rgb(bg, fg),
                accent: caps.palette[4].unwrap_or(fg),
                ansi: std::array::from_fn(|i| caps.palette[i + 1].unwrap_or(fg)),
            },
            cell,
            // Full screen, a diagram at zoom 1 fills the screen above the pager
            // status line. In the document it keeps the terminal's text size and
            // the image height cap; a click opens it larger.
            max_rows: if fullscreen {
                u32::try_from(size.rows.saturating_sub(1))
                    .unwrap_or(u32::MAX)
                    .clamp(1, kitty::MAX_ROWS)
            } else {
                max_rows
            },
            zoom,
            max_upscale: if fullscreen { f32::INFINITY } else { 1.0 },
            // The pager centres a full-screen diagram itself.
            center: !fullscreen,
        });
    // Without the feature, formulas are shown as code.
    let math = cell
        .filter(|_| cfg!(feature = "math"))
        .map(|cell| MathImages { cell, color: fg });
    let images = cell.map(|cell| Images {
        cell,
        max_rows,
        base_dir: base_dir.to_path_buf(),
    });
    Config {
        width: size.cols,
        color: true,
        text_sizing: caps.text_sizing,
        code_bg: caps.bg.map(|bg| {
            let (r, g, b) = panel_rgb(bg, fg);
            Color::Rgb(r, g, b)
        }),
        images,
        headings,
        diagrams,
        math,
        first_image_id,
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
