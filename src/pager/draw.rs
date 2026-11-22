use std::fmt::Write as _;

use unicode_width::UnicodeWidthChar;

use super::Pager;
use super::links;
use super::view::Kind;

impl Pager<'_> {
    /// Left and top margins that centre a full-screen diagram.
    pub(super) fn margins(&self) -> (usize, usize) {
        let view = self.view();
        if !view.is_diagram() {
            return (0, 0);
        }
        let width = view
            .doc
            .lines
            .iter()
            .filter_map(|l| l.image.map(|i| i.visible as usize))
            .max()
            .unwrap_or(0);
        (
            self.size.cols.saturating_sub(width) / 2,
            self.view_rows().saturating_sub(view.total()) / 2,
        )
    }

    pub(super) fn draw(&self, out: &mut String) {
        let view = self.view();
        let (left, top_margin) = self.margins();
        out.push_str("\x1b[?2026h");
        // Each row is erased (EL) before it is drawn, not after: with
        // autowrap off (DEC 7) the cursor stays on the last column once a
        // full-width line is written, and EL would erase that column too.
        for row in 0..self.view_rows() {
            let _ = write!(out, "\x1b[{};1H\x1b[0m\x1b[K", row + 1);
            let index = row.checked_sub(top_margin).map(|r| view.top + r);
            if let Some((index, line)) = index.and_then(|i| Some((i, view.doc.lines.get(i)?))) {
                let _ = write!(out, "{:left$}", "");
                let text = line.panned(view.pan);
                match view.selected_range(index) {
                    Some(range) => out.push_str(&links::highlight(&text, &range)),
                    None => out.push_str(&text),
                }
            }
            out.push_str("\x1b[0m");
        }

        let _ = write!(
            out,
            "\x1b[{};1H\x1b[0m\x1b[K\x1b[7m{}\x1b[0m",
            self.size.rows.max(1),
            truncate(&self.status(), self.size.cols)
        );
        out.push_str(if self.prompt.is_some() {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });
        out.push_str("\x1b[?2026l");
    }

    pub(super) fn status(&self) -> String {
        if let Some(input) = &self.prompt {
            return format!("/{input}");
        }
        if let Some(msg) = &self.message {
            return msg.clone();
        }
        let view = self.view();
        let mut extra = String::new();
        if (view.zoom - 1.0).abs() > f32::EPSILON {
            let _ = write!(extra, "  zoom {:.0}%", view.zoom * 100.0);
        }
        if view.doc.max_pan() > 0 {
            let _ = write!(extra, "  ←{}/{}→", view.pan, view.doc.max_pan());
        }
        if view.is_diagram() {
            return format!(
                " {} › diagram{extra}  · q/Esc back · +/- zoom · hjkl move",
                self.title
            );
        }
        if let Some(link) = view.selected.and_then(|i| view.links.get(i)) {
            return format!(" → {}  · Enter open · Tab next · Esc cancel", link.url);
        }
        let title = match &view.kind {
            Kind::File(path) => path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            _ => self.title.to_owned(),
        };
        let back = if self.views.len() > 1 {
            " · ⌫ back"
        } else {
            ""
        };
        let total = view.total();
        let last = (view.top + self.view_rows()).min(total);
        let pct = (last * 100).checked_div(total).unwrap_or(100);
        format!(
            " {title}  {}-{}/{}  {pct}%{extra}  · q quit{back} · / search · Tab links · z diagram · +/- zoom",
            (view.top + 1).min(total),
            last,
            total
        )
    }
}

pub(super) fn truncate(s: &str, cols: usize) -> String {
    let mut used = 0;
    s.chars()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= cols
        })
        .collect()
}
