# martty

**Markdown, rendered properly, right in your terminal.**

A tiny, fast Markdown viewer built for [Ghostty](https://ghostty.org), with real images,
Mermaid diagrams, big headings and a `less`-style pager.

![Rust](https://img.shields.io/badge/rust-2024_edition-orange?logo=rust)
![Terminal](https://img.shields.io/badge/terminal-Ghostty-6f42c1)
![Status](https://img.shields.io/badge/status-early_development-yellow)

---

## Why martty?

Most terminal Markdown viewers have to stick to plain text. martty is built for one terminal
and uses everything it can do: kitty graphics with Unicode placeholders, OSC 8 hyperlinks,
synchronized output and theme color queries. Your README looks like a document, not a dump
of asterisks.

- 🖼️ **Images inline.** PNG, JPEG, GIF, WebP and SVG, drawn with the kitty graphics
  protocol; animated GIF and WebP play as kitty animations.
- 📊 **Mermaid diagrams.** Flowcharts, sequence diagrams, pie charts and more, rendered
  locally without a browser and colored to match your Ghostty theme.
- 🔠 **Big headings.** H1–H3 are drawn at large sizes with JetBrains Mono.
- ∑ **Math.** LaTeX formulas, inline and display, typeset with TeX's rules in STIX Two
  Math.
- 🎨 **Theme-aware.** Highlighted code, diagrams and headings take their colors from your
  terminal's palette, so they suit both light and dark themes.
- 📜 **Built-in pager.** Scrolling, search, mouse wheel, zoomable and pannable diagrams,
  and live re-layout when the window is resized.
- 🪶 **Self-contained.** A single static binary: no browser, no Node, no runtime to install.

## Supported Markdown

| Element | Notes |
|---|---|
| Headings | H1–H3 drawn as large images, H4–H6 styled as text |
| Emphasis | **bold**, *italic*, ~~strikethrough~~, `inline code`, `==highlight==` |
| Links | Clickable through OSC 8 (relative paths become `file://` URLs); in the pager also by `Tab` and `Enter` |
| Lists | Nested, ordered (numbers aligned on the dot), and task lists `- [x]` |
| Block quotes | Drawn with a left bar, including images inside quotes |
| Alerts | GitHub `> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]` with a colored bar and title |
| Code blocks | Syntax highlighting in the terminal palette's colors, on a background derived from your theme |
| Tables | Column alignment (`:---:`, `---:`); a table wider than the screen shrinks its columns by content and wraps the text in its cells |
| Footnotes | `[^1]` style |
| Definition lists | `Term` followed by `: definition` lines |
| Images | PNG, JPEG, GIF, WebP and SVG, local or (with the `remote` feature) from http(s); animated GIF and WebP play; a missing or unreadable image falls back to its alt text |
| Mermaid | ` ```mermaid ` blocks are rendered as diagrams, centered, with text at the terminal's size; click one to open it full screen |
| Math | `$…$` inline, sitting on the text baseline; `$$…$$` and ` ```math ` blocks centered on their own rows. A formula latex-rust can't render shows as code |
| Rules | `---` |
| Emoji | Common GitHub shortcodes such as `:rocket:` and `:+1:` |
| HTML | `<br>`, `<kbd>`, `<sub>`/`<sup>`, `<details>`/`<summary>` (expanded), `<b>`, `<i>`, `<mark>`, `<a>`, `<img>`; other tags are dropped and their text kept, comments hidden |
| Front matter | YAML (`---`) or TOML (`+++`), shown as dimmed `key  value` lines |

## Installation

martty is not published yet, so build it from source. You need a recent stable Rust
toolchain with edition 2024 support (Rust 1.100+).

```sh
git clone <repo-url> martty
cd martty
cargo install --path .
```

Or build without installing:

```sh
cargo build --release
./target/release/martty demo.md
```

Three optional features are on by default:

| Feature | Adds | Without it |
|---|---|---|
| `mermaid` | Mermaid diagrams via merman, about 10 MB | ` ```mermaid ` blocks show as code |
| `math` | LaTeX formulas via latex-rust with the STIX Two Math font, about 2.5 MB | Formulas show as code |
| `highlight` | Syntax highlighting via syntect, about 1 MB; builds the Oniguruma C library, so it needs a C compiler | Code blocks in one color |

To leave them all out, or pick some back:

```sh
cargo install --path . --no-default-features
cargo install --path . --no-default-features --features highlight
```

One more is off by default, since with it viewing a document makes network requests:

| Feature | Adds |
|---|---|
| `remote` | Images from `http(s)://` URLs via ureq and rustls. They are fetched in parallel before the document is shown (10 s timeout) and cached for a day in `~/Library/Caches/martty` (macOS) or `$XDG_CACHE_HOME/martty`; a stale copy is used when the network fails |

```sh
cargo install --path . --features remote
```

## Usage

```sh
martty README.md            # view a file
cat notes.md | martty       # read from stdin
martty - < notes.md         # same, explicitly
martty -p short.md          # always open the pager
martty -P long.md | head    # never open the pager
martty --width 72 notes.md  # lay out for 72 columns
```

```
usage: martty [-p|--pager] [-P|--no-pager] [--width N] [FILE]
```

| Flag | Meaning |
|---|---|
| `-p`, `--pager` | Always open the pager |
| `-P`, `--no-pager` | Never open the pager; print straight to stdout |
| `--width N` | Lay out text for `N` columns instead of the terminal width (80 when it is unknown, as in a pipe) |
| `-h`, `--help` | Show usage |

By default martty prints the document directly. If the output is taller than the screen
and stdout is a terminal, it opens the pager. Relative image paths are resolved against the
Markdown file's directory, or against the current directory when reading stdin.

Try the bundled demo to see everything at once:

```sh
cargo run --release -- demo.md
```

## Pager keys

The pager follows `less` conventions.

| Keys | Action |
|---|---|
| `j` `e` `↓` `Enter` `Ctrl-N` | Scroll down one line (`Enter` opens the selected link) |
| `k` `y` `↑` `Ctrl-P` | Scroll up one line |
| `Space` `f` `PgDn` `Ctrl-F` | Page down |
| `b` `PgUp` `Ctrl-B` | Page up |
| `d` `Ctrl-D` / `u` `Ctrl-U` | Half page down / up |
| `g` `<` `Home` / `G` `>` `End` | Go to the top / bottom |
| `/` | Search (smart case: case-sensitive only if the query has uppercase letters) |
| `n` / `N` | Next / previous match |
| `h` `←` / `l` `→` | Pan wide images and diagrams left / right |
| `+` `=` / `-` `_` / `0` | Zoom images and diagrams in / out / reset |
| `Tab` / `Shift-Tab` | Select the next / previous link, scrolling to it |
| `Enter` | Open the selected link: `#heading` jumps to it, a relative `.md` file opens in the pager, anything else opens with `open` (`xdg-open` on Linux) |
| `Backspace` `H` | Go back from an opened file or a full-screen diagram |
| `Esc` | Drop the link selection |
| `z` | Open the diagram on screen full screen |
| `Esc` `q` | Close the full-screen diagram |
| `q` `Ctrl-C` | Quit |
| Mouse wheel | Scroll |

> [!TIP]
> The pager leaves the mouse to Ghostty: select text and Cmd+click links as usual.

## Configuration

At startup martty sends the terminal a single round of queries to detect its features:
theme colors, kitty graphics support, cell size in pixels and OSC 66 text sizing. If
detection gets something wrong, override it with environment variables:

| Variable | Values | Effect |
|---|---|---|
| `MARTTY_IMAGES` | `0` / `1` | Disable or force images, diagrams and image headings |
| `MARTTY_TEXT_SIZING` | `0` / `1` | Disable or force OSC 66 text sizing for headings |
| `NO_COLOR` | any non-empty | No colors: text keeps bold, italic and underline; images, diagrams and image headings are off, as with `MARTTY_IMAGES=0` |

When graphics are not available, martty falls back to plain styled text: diagrams and
formulas show up as code and images as their alt text.

## How it works

```mermaid
flowchart LR
    MD[Markdown] --> P[pulldown-cmark]
    P --> R{Block}
    R -->|text| T[ANSI + OSC 8]
    R -->|mermaid| M[merman] --> S[resvg]
    R -->|image| I[PNG / JPEG / GIF / WebP / SVG]
    R -->|H1–H3| F[fontdue]
    R -->|math| X[latex-rust] --> S
    S --> K[kitty graphics<br/>Unicode placeholders]
    I --> K
    F --> K
    T --> G((Ghostty))
    K --> G
```

Images are sent once and then placed as Unicode placeholder characters, so every rendered
line is plain text. That keeps scrolling and panning in the pager cheap: it redraws a window
of lines and never re-sends pixels.

| Module | Responsibility |
|---|---|
| `render/` | Markdown → lines of styled text |
| `term.rs` | Capability probing, raw mode, terminal size |
| `kitty.rs`, `diacritics.rs` | kitty graphics transmission and Unicode placeholders |
| `mermaid.rs` | Diagram rendering with theme colors |
| `math.rs` | LaTeX formulas rasterized at the terminal's text size |
| `heading.rs` | Large headings rasterized with fontdue |
| `pager/` | The `less`-like pager |
| `style.rs` | Colors and text attributes |

## Limitations

- **Ghostty only, by design.** Other terminals with kitty graphics may work, but they are
  not a goal.
- Remote images are fetched before the document is shown, so a slow server delays the
  start (up to the 10 s timeout).
- Headings drawn as images can't be copied as text. They will switch back to text once
  Ghostty implements OSC 66.
- Formulas are images too: they can't be copied as text.

## Development

```sh
cargo build
cargo test
cargo clippy
cargo fmt
```

## Acknowledgements

- [Ghostty](https://ghostty.org), the terminal this is built for
- [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) for Markdown parsing
- [merman](https://github.com/Latias94/merman) and
  [resvg](https://github.com/linebender/resvg) for diagrams
- [fontdue](https://github.com/mooman219/fontdue) for glyph rasterization
- [syntect](https://github.com/trishume/syntect) for syntax highlighting
- [latex-rust](https://github.com/jscarr64/latex-rust) for typesetting math, with the
  [STIX Two Math](https://github.com/stipub/stixfonts) font it bundles under the SIL Open
  Font License
- [JetBrains Mono](https://www.jetbrains.com/lp/mono/), bundled under the
  [SIL Open Font License](assets/fonts/OFL.txt)

## License

martty is licensed under the [Apache License 2.0](LICENSE). The bundled JetBrains Mono font
is licensed separately under the [SIL Open Font License 1.1](assets/fonts/OFL.txt), as is
STIX Two Math, which latex-rust embeds with the `math` feature.
