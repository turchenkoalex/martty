# Changelog

What changed in each release of martty, newest first. The binaries are on the
[releases page](https://github.com/turchenkoalex/martty/releases).

## Unreleased

### Changed

- **Headings are all one color.** Every level is drawn in the blue of your theme; H1 was magenta and H3–H6 had colors of their own. Levels still differ by size and underline.

## [0.1.6] - 2026-11-22

### New

- **Rex is supported.** It is built on libghostty, so images, diagrams, headings, theme switches and copying work as in Ghostty, and big headings are drawn in the font chosen in Rex's settings.

### Fixed

- **Search finds H1–H3 headings.** They are drawn as images, so `/` skipped them; now it moves to a heading that matches and counts it, without reversing its picture.

## [0.1.5] - 2026-11-22

### New

- **Images of a paragraph sit side by side**, as many as fit the width, the way a browser lays out inline images. A README's row of badges is a row again, not a column, and badges line up without stray gaps.
- **A broken Mermaid block says why in its panel:** `invalid mermaid: …` takes the label row where the language goes, instead of a red line above the block.

### Safer with untrusted documents

- **Escape sequences in a document no longer reach the terminal.** Control characters show as symbols (`␛`), so a Markdown file can't set your clipboard, retitle the window or fake a link, whether it holds raw bytes or `&#27;` entities.
- **Links open only what shows something:** `http`, `https` and `mailto` URLs and local files. Other schemes, programs, scripts and app bundles are not opened.
- **Images can't hang martty or eat memory:** local images are read only from regular files up to 64 MB (never `/dev/zero` or a pipe), and pictures over 64 megapixels are not decoded.
- **A diagram, formula or image that crashes its renderer falls back to text** instead of taking the viewer down.
- `--remote` refuses a few more ways of reaching the local network over IPv6.

### Fixed

- With `--remote`, a README with more than four remote images (badges) lost half of them.
- Switching the system theme twice in a row could make the pager jump around.
- Scrolling with the wheel during a slow layout could leave stray keys behind.
- Following a file (`-w`) could miss an edit saved while the document was being laid out.
- `0` resets the zoom exactly, so images aren't redrawn needlessly.
- Files that aren't UTF-8 open with replacement characters instead of an error.
- Links to files with `%` or `#` in their names open the right file.

## [0.1.4] - 2026-11-22

### New

- **Edit with `v`, as in `less`.** The pager hands the file to `$VISUAL` or `$EDITOR` (`vi` when neither is set) and lays it out again when the editor exits, so your edits show at once. Vim, Neovim, Nano, Micro, Helix, Emacs, Zed, Sublime Text, VS Code and Cursor open at the section on screen. The editor runs through `sh`, as git does, so the variable may hold arguments such as `zed --wait`.

### Changed

- **Keys:** the diagram on screen opens full screen with `o` (it was `v`).

## [0.1.3] - 2026-11-22

### Changed

- **Diagrams are readable at any height.** In the document a diagram fits the width only and keeps the terminal's text size; a tall one takes as many rows as it needs and scrolls with the document, instead of shrinking to two thirds of the screen.
- **Diagram labels wrap like Mermaid's.** They used to break after a dozen characters, often mid-word; long identifiers now stay whole.
- **Keys:**
  - `v` opens the diagram on screen full screen (it was `z`); `v`, `Space`, `Esc` or `q` close it.
  - `z` pages down, as in `less`.
  - The table of contents opens with `t` only (`o` no longer does).

## [0.1.2] - 2026-11-22

### New

- **`martty --help`** lists the options, the `MARTTY_` environment variables and the pager keys by topic, in color in a terminal and plain in a pipe.
- Run with no file and nothing on stdin, martty prints that help instead of an error.
- The pager's `?` screen shows the same keys as `--help`.
- **Homebrew:** `brew install turchenkoalex/tap/martty` (macOS on Apple silicon, Linux on x86_64 and arm64). The formula updates with every release.

## [0.1.1] - 2026-11-22

### New

- **Linux binaries** for x86_64 and arm64. They are static (musl), so they run on any distribution.
- `martty --version` (`-V`).
- **Faster on long documents.** Images, diagrams and headings are drawn only as they come within a screen of the view, on all cores. What was drawn is kept for the session, so zooming, resizing or saving a followed file redraws only what changed. On the demo, the first screen is ready in 22 ms instead of 96 ms.

### Fixed

- An image set as HTML (`<img>`) now has a blank line above it, and its alt text no longer appears under it.
- An image in a paragraph with text goes below the text, a blank line apart, without repeating its alt text.
- Alt text shown in place of an image is dimmed, without the 🖼 icon; an image with no alt text shows its file name.

### Changed

- No more build for Intel Macs; on one, build from source with `cargo install --git https://github.com/turchenkoalex/martty`.

## [0.1.0] - 2026-11-22

The first release of martty, a Markdown viewer built for [Ghostty](https://ghostty.org).

- **Images inline:** PNG, JPEG, GIF, WebP and SVG through kitty graphics; animated GIF and WebP play. `https://` images load with `--remote`.
- **Mermaid diagrams** rendered locally, without a browser, in your theme's colors. `z` opens one full screen, `+`/`-` zoom, `h`/`l` pan.
- **Big headings:** H1–H3 drawn large in the bold face of your Ghostty `font-family`, or in the bundled JetBrains Mono.
- **Math:** LaTeX formulas, inline and display, typeset with TeX's rules in STIX Two Math.
- **Theme-aware:** highlighted code, diagrams and headings take their colors from the terminal palette and follow Ghostty when it switches between light and dark.
- **The rest of Markdown:** tables that wrap inside their cells, GitHub alerts, task lists, footnotes, definition lists, front matter, emoji shortcodes, a subset of HTML, OSC 8 links.
- **A `less`-style pager:** search with highlighted matches, a table of contents, marks, following links (relative `.md` files open in the pager), copying as plain text over OSC 52, following a file as you edit it (`-w`), live re-layout on resize.
- **Fast on long documents:** images, diagrams and headings are drawn only as they come into view, on all cores, and kept for the session.

[0.1.6]: https://github.com/turchenkoalex/martty/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/turchenkoalex/martty/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/turchenkoalex/martty/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/turchenkoalex/martty/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/turchenkoalex/martty/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/turchenkoalex/martty/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/turchenkoalex/martty/releases/tag/v0.1.0
