# martty — Agent Guide

## What is this

A Markdown viewer for the terminal, built for [Ghostty](https://ghostty.org) only. It
renders Markdown to ANSI text, draws images, Mermaid diagrams and H1–H3 headings with
kitty graphics, and shows long output in a `less`-like pager. `README.md` describes the
user-facing behavior; keep it in sync when that behavior changes.

**Ghostty is the only target.** Use what it supports (kitty graphics with Unicode
placeholders, OSC 8, synchronized output, theme color queries) and don't add workarounds
for other terminals. When a capability is missing, fall back to plain styled text.

## Commands

```bash
cargo run -- demo.md                  # view the demo, which exercises every feature
cargo build --release                 # LTO + stripped binary
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

CI (`.github/workflows/ci.yml`) runs the last three on every push to `main` and every pull
request: fmt and clippy on Linux, tests on macOS and Linux. Run them before committing.

Rendering can only be checked by eye in a real Ghostty window. When a change affects
output, say what to look at in `demo.md` instead of claiming it looks right.

## Architecture

```
src/
├── main.rs        # args, reading input, choosing between stdout and the pager
├── render/        # pulldown-cmark events → lines of styled text
│   ├── mod.rs     # Config, `render`, event dispatch
│   ├── document.rs # rendered Document, lines, image rows for panning
│   ├── layout.rs  # containers (quotes, list items), line prefixes, block spacing
│   ├── inline.rs  # inline styles, code spans, list and task markers
│   ├── blocks.rs  # headings, code blocks, rules, front matter
│   ├── table.rs   # tables: column widths, wrapping inside cells
│   ├── formulas.rs # inline and display math placed as images
│   ├── links.rs   # link targets (relative → file://), heading anchors
│   ├── highlight.rs # syntect scopes → palette colors (feature `highlight`)
│   ├── images.rs  # images, diagrams and image headings placed via kitty
│   ├── span.rs    # styled text spans and their SGR/OSC 8 output
│   ├── slice.rs   # cutting a column range out of an ANSI line, for panning
│   └── wrap.rs    # word wrapping by display width
├── style.rs       # SGR colors and attributes
├── term.rs        # capability probe (one round-trip), raw mode, window size
├── kitty.rs       # kitty graphics transmission and Unicode placeholders
├── diacritics.rs  # generated row/column diacritics table, don't edit by hand
├── picture/       # image formats: sniffing, JPEG/GIF/WebP decoding, SVG, downscaling
├── remote.rs      # https images, opt-in per run, fenced in; disk cache
├── mermaid.rs     # merman → SVG → resvg → kitty (feature `mermaid`)
├── math.rs        # LaTeX → latex-rust SVG → resvg → kitty (feature `math`)
├── heading.rs     # H1–H3 rasterized with fontdue in Ghostty's font or bundled JetBrains Mono
├── progress.rs    # Ghostty's progress indicator (OSC 9;4) during slow layouts
└── pager/         # alternate-screen pager
    ├── mod.rs     # `run` loop, view stack, relayout, following files, terminal enter/leave
    ├── view.rs    # one view: document, scroll, pan, zoom, marks; position across relayouts
    ├── search.rs  # matches, their highlight and the counter
    ├── overlay.rs # key help and the table of contents
    ├── copy.rs    # OSC 52 and plain-text copies of code, the screen, the document
    ├── keys.rs    # reading the tty and keys (legacy and kitty keyboard protocol)
    ├── theme.rs   # theme switch reported by Ghostty (DEC 2031): colors again, relayout
    ├── links.rs   # links found by OSC 8, Tab selection highlight, opening
    ├── input.rs   # key bindings, scrolling, marks, zoom, pan
    └── draw.rs    # frame and status line
```

- Images are transmitted once and then placed as Unicode placeholder characters, so every
  rendered line is plain text. The pager scrolls, clips and pans by redrawing lines and
  never re-sends pixels. Keep this invariant: anything drawn as an image must still be a
  sequence of text lines.
- Terminal capabilities come from the single probe in `term.rs`. Add new queries to that
  round-trip, before the DA1 sentinel, instead of a separate one. `MARTTY_IMAGES` and
  `MARTTY_TEXT_SIZING` override detection.
- Colors follow the user's theme: derive them from the probed palette and
  foreground/background, never hardcode a light or dark scheme.
- An inline image (a `$…$` formula) is one row of placeholder cells inside a text span, so
  it wraps with the words around it; it must never be wider than the line.
- H1–H3 are images only until Ghostty supports OSC 66; then they go back to text.

## Dependencies

Every new crate must give something we can't write in an evening. Keep default features
off and enable only what is used; put heavy optional functionality behind a cargo feature.

## Code style

- Rust 2024 edition. Follow the `rust-best-practices` skill in `.agents/skills/`.
- **No `unwrap`/`expect`** in non-test code (clippy warns on both). Return a `Result` or
  handle the failure; a missing capability or a broken image falls back to text, it never
  panics.
- **`unsafe` stays in `term.rs` and `pager/keys.rs`** for the `libc` terminal calls (`poll`,
  `ioctl`, `tcgetattr`/`tcsetattr`). Keep each block to the single call and don't spread it
  to other modules.
- **Restore the terminal on every exit path**: raw mode, alternate screen, alternate scroll mode.
  Rely on `Drop` guards, not on cleanup at the end of a function.
- **Width is measured in cells** with `unicode-width`, never in bytes or `char`s.
- Comments say what the code cannot: why, terminal quirks, protocol details, units (cells
  vs pixels). Don't add comments that restate a name or narrate the next line. When
  changing code, delete such comments in the part you touch and fix stale ones; don't
  sweep untouched files just for that. Name the escape sequence a raw byte string sends
  (`DEC 2026`, `OSC 8`), since the bytes themselves are unreadable.

## Keeping files small

- **Files** — `tests/source_file_size.rs` caps each `src/` file at 500 code lines (blank
  lines, `//` comments, the trailing `#[cfg(test)]` module and `tests.rs` files don't
  count). Larger files are listed in `OVERSIZED_FILES` with their size; the test fails if
  they grow, and also if they shrink without the entry being lowered.
- **Functions** — clippy `too_many_lines` (over 100 lines) is enabled in `Cargo.toml`.

When a module grows, turn it into a directory module and split it by concern, as
`render/` is. When changing an oversized file, first move out the part being changed, then
lower its cap. Raising a cap is a deliberate exception; say why in the commit message.

## Testing

Unit tests sit inline in `#[cfg(test)] mod tests` blocks. Test rendering on its text
output without color codes. There is no terminal in tests: `render` takes a `Config`
built from the probed capabilities, so tests construct one instead of probing.

Snapshot tests render every `tests/fixtures/*.md` as plain text at 60 columns and compare
it with `tests/snapshots/*.snap` (`insta`). When a change alters output on purpose, update
them with `INSTA_UPDATE=always cargo test` (or `cargo insta review`) and read the diff of
the `.snap` files before committing. Cover a new block or inline feature with a fixture.
