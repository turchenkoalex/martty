use std::fmt::Write as _;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::Pager;
use super::links;
use super::overlay::Overlay;
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
        let overlay = self.overlay.as_ref().map(|o| self.overlay_rows(o));
        for row in 0..self.view_rows() {
            let _ = write!(out, "\x1b[{};1H\x1b[0m\x1b[K", row + 1);
            if let Some(rows) = &overlay {
                out.push_str(rows.get(row).map_or("", String::as_str));
                out.push_str("\x1b[0m");
                continue;
            }
            let index = row.checked_sub(top_margin).map(|r| view.top + r);
            if let Some((index, line)) = index.and_then(|i| Some((i, view.doc.lines.get(i)?))) {
                let _ = write!(out, "{:left$}", "");
                let ranges = self.marked_ranges(index);
                if ranges.is_empty() {
                    out.push_str(&line.panned(view.pan));
                } else {
                    let marked = links::highlight_all(&line.text, ranges);
                    out.push_str(&line.pan_text(&marked, view.pan));
                }
            }
            out.push_str("\x1b[0m");
        }

        // Padded to the full width, so the reversed bar spans the screen.
        // The position sits at the right edge; the left part gives way to
        // it on a narrow screen.
        let row = self.size.rows.max(1);
        let right = self.position().filter(|p| p.width() < self.size.cols);
        let right_width = right.as_deref().map_or(0, UnicodeWidthStr::width);
        let status = truncate(&self.status(), self.size.cols - right_width);
        let used = status.width();
        let pad = self.size.cols - right_width - used;
        let right = right.unwrap_or_default();
        let _ = write!(
            out,
            "\x1b[{row};1H\x1b[0m\x1b[K\x1b[7m{status}{:pad$}{right}\x1b[0m",
            ""
        );
        if self.prompt.is_some() {
            // The cursor goes back to the end of the typed query.
            let _ = write!(out, "\x1b[{row};{}H\x1b[?25h", used + 1);
        } else {
            out.push_str("\x1b[?25l");
        }
        out.push_str("\x1b[?2026l");
    }

    /// The lines on screen and how far through the document the last one
    /// is, as ` 1-24/31  77% `, for the right end of the status line. None
    /// while a prompt or overlay takes the line, and for a full-screen diagram.
    pub(super) fn position(&self) -> Option<String> {
        let view = self.view();
        if self.prompt.is_some() || self.overlay.is_some() || view.is_diagram() {
            return None;
        }
        let total = view.total();
        let last = (view.top + self.view_rows()).min(total);
        let pct = (last * 100).checked_div(total).unwrap_or(100);
        let first = (view.top + 1).min(total);
        Some(format!(" {first}-{last}/{total}  {pct}% "))
    }

    pub(super) fn status(&self) -> String {
        if let Some(input) = &self.prompt {
            return format!("/{input}");
        }
        if let Some(msg) = &self.message {
            return msg.clone();
        }
        match (&self.overlay, self.pending) {
            (Some(Overlay::Help), _) => return " Keys  · any key closes".into(),
            (Some(Overlay::Toc { .. }), _) => {
                return " Contents  · j/k move · Enter go · Esc close".into();
            }
            (None, Some('m')) => return " Set mark: type a letter".into(),
            (None, Some(_)) => return " Go to mark: type a letter, or ' to go back".into(),
            (None, None) => {}
        }
        let view = self.view();
        let mut extra = String::new();
        if (view.zoom - 1.0).abs() > f32::EPSILON {
            let _ = write!(extra, "  zoom {:.0}%", view.zoom * 100.0);
        }
        if view.doc.max_pan() > 0 {
            let _ = write!(extra, "  ←{}/{}→", view.pan, view.doc.max_pan());
        }
        if let Some(counter) = self.hit_counter() {
            let _ = write!(extra, "  {counter}");
        }
        if self.follow {
            extra.push_str("  following");
        }
        if view.is_diagram() {
            let count = self
                .views
                .len()
                .checked_sub(2)
                .map_or(0, |d| self.views[d].doc.diagrams.len());
            let number = view
                .diagram
                .map(|i| format!(" {}/{count}", i + 1))
                .unwrap_or_default();
            return format!(
                " {} › diagram{number}{extra}  · v/Space/Esc back · ←/→ other diagrams · +/- zoom · hjkl move",
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
        format!(" {title}{extra}  · q quit{back} · / search · t contents · ? help")
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
