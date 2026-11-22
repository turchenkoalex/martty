use std::fmt::Write as _;

use unicode_width::UnicodeWidthChar;

use super::Pager;

impl Pager<'_> {
    /// Left and top margins that centre a full-screen diagram.
    pub(super) fn margins(&self) -> (usize, usize) {
        let view = self.view();
        if view.source.is_none() {
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
        for row in 0..self.view_rows() {
            let _ = write!(out, "\x1b[{};1H\x1b[0m", row + 1);
            let line = row
                .checked_sub(top_margin)
                .and_then(|r| view.doc.lines.get(view.top + r));
            if let Some(line) = line {
                let _ = write!(out, "{:left$}", "");
                out.push_str(&line.panned(view.pan));
            }
            out.push_str("\x1b[0m\x1b[K");
        }

        let _ = write!(
            out,
            "\x1b[{};1H\x1b[0;7m{}\x1b[0m\x1b[K",
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
        if view.source.is_some() {
            return format!(
                " {} › diagram{extra}  · q/Esc/click back · +/- zoom · hjkl move",
                self.title
            );
        }
        let total = view.total();
        let last = (view.top + self.view_rows()).min(total);
        let pct = (last * 100).checked_div(total).unwrap_or(100);
        format!(
            " {}  {}-{}/{}  {pct}%{extra}  · q quit · / search · click/z diagram · +/- zoom",
            self.title,
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
