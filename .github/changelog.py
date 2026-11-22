#!/usr/bin/env python3
"""CHANGELOG.md is the one place a release is described; this turns it into
the GitHub release notes and the site's changelog page.

    changelog.py notes VERSION            release notes for VERSION, to stdout
    changelog.py page TEMPLATE OUTPUT     TEMPLATE with its <!-- changelog -->
                                          marker replaced by the changelog

It reads only the Markdown CHANGELOG.md uses: `##`/`###` headings, paragraphs,
`-` lists (nested by two spaces), **bold**, `code`, [links](url) and fenced
code. `notes` fails when VERSION has no section, so a release can't go out
undescribed.
"""

import html
import re
import sys
from pathlib import Path

CHANGELOG = Path(__file__).resolve().parent.parent / "CHANGELOG.md"
REPO = "https://github.com/turchenkoalex/martty"
VERSION = re.compile(r"^## \[(?P<version>[^\]]+)\](?: - (?P<date>\S+))?\s*$")
MARKER = "<!-- changelog -->"

INSTALL = """## Install

```sh
brew upgrade martty   # or: brew install turchenkoalex/tap/martty
```

Or download an archive below. The binaries are signed ad hoc, not with a \
Developer ID: on macOS, clear the quarantine flag of a file saved from a \
browser with `xattr -d com.apple.quarantine martty`."""


def sections(text):
    """(version, date, body lines) for each `## [version]` section, in order,
    and the lines before the first one (the intro, `## Unreleased`)."""
    intro, found, current = [], [], None
    for line in text.splitlines():
        m = VERSION.match(line)
        if m:
            current = (m["version"], m["date"], [])
            found.append(current)
        elif line.startswith("## "):
            current = None
            intro.append(line)
        elif re.match(r"^\[[^\]]+\]: ", line):
            continue
        elif current:
            current[2].append(line)
        else:
            intro.append(line)
    return intro, [(v, d, "\n".join(b).strip()) for v, d, b in found]


def notes(version):
    _, found = sections(CHANGELOG.read_text())
    body = next((b for v, _, b in found if v == version), "")
    if not body:
        sys.exit(f"CHANGELOG.md has no section for {version}")
    # Headings one level up: the release title is the version already.
    body = re.sub(r"^### ", "## ", body, flags=re.M)
    index = [v for v, _, _ in found].index(version)
    previous = found[index + 1][0] if index + 1 < len(found) else None
    log = (
        f"{REPO}/compare/v{previous}...v{version}"
        if previous
        else f"{REPO}/commits/v{version}"
    )
    return f"{body}\n\n{INSTALL}\n\n**Full Changelog**: {log}\n"


def inline(text):
    out, rest = [], text
    pattern = re.compile(r"`([^`]+)`|\*\*(.+?)\*\*|\[([^\]]+)\]\(([^)]+)\)")
    while (m := pattern.search(rest)):
        out.append(html.escape(rest[: m.start()]))
        if m[1] is not None:
            out.append(f"<code>{html.escape(m[1])}</code>")
        elif m[2] is not None:
            out.append(f"<strong>{inline(m[2])}</strong>")
        else:
            out.append(f'<a href="{html.escape(m[4])}">{inline(m[3])}</a>')
        rest = rest[m.end() :]
    out.append(html.escape(rest))
    return "".join(out)


def blocks(text):
    """HTML for one section's body."""
    out, para, lists = [], [], []
    lines = text.splitlines()

    def flush_para():
        if para:
            out.append(f"<p>{inline(' '.join(para))}</p>")
            para.clear()

    def close_lists(depth):
        while len(lists) > depth:
            lists.pop()
            out.append("</li></ul>")

    i = 0
    while i < len(lines):
        line = lines[i]
        item = re.match(r"^( *)- (.*)$", line)
        if line.startswith("```"):
            flush_para()
            close_lists(0)
            code = []
            i += 1
            while i < len(lines) and not lines[i].startswith("```"):
                code.append(lines[i])
                i += 1
            out.append(f"<pre><code>{html.escape(chr(10).join(code))}</code></pre>")
        elif line.startswith("### "):
            flush_para()
            close_lists(0)
            out.append(f"<h3>{inline(line[4:])}</h3>")
        elif item:
            flush_para()
            depth = len(item[1]) // 2 + 1
            if depth > len(lists):
                out.append("<ul><li>")
                lists.append(depth)
            else:
                close_lists(depth)
                out.append("</li><li>")
            out.append(inline(item[2]))
        elif not line.strip():
            flush_para()
        elif lists:
            out.append(" " + inline(line.strip()))
        else:
            para.append(line.strip())
        i += 1
    flush_para()
    close_lists(0)
    return "\n".join(out)


def page(template, output):
    # Released versions only: `## Unreleased` is for the next release notes.
    _, found = sections(CHANGELOG.read_text())
    parts = []
    for n, (version, date, body) in enumerate(found):
        previous = found[n + 1][0] if n + 1 < len(found) else None
        link = (
            f"{REPO}/compare/v{previous}...v{version}"
            if previous
            else f"{REPO}/releases/tag/v{version}"
        )
        when = f' <time datetime="{date}">{date}</time>' if date else ""
        parts.append(
            f'<article class="release" id="v{html.escape(version)}">\n'
            f'<h2><a href="#v{html.escape(version)}">{html.escape(version)}</a>{when}'
            f' <a class="diff" href="{link}">changes</a></h2>\n'
            f"{blocks(body)}\n</article>"
        )
    text = Path(template).read_text()
    if MARKER not in text:
        sys.exit(f"{template} has no {MARKER} marker")
    Path(output).write_text(text.replace(MARKER, "\n".join(parts)))


def main(args):
    if len(args) == 2 and args[0] == "notes":
        sys.stdout.write(notes(args[1]))
    elif len(args) == 3 and args[0] == "page":
        page(args[1], args[2])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
