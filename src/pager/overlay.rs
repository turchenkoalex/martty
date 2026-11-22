//! Screens drawn over the document: key help (`?`) and the table of
//! contents (`o`/`t`).

use unicode_width::UnicodeWidthStr;

use super::draw::truncate;
use super::keys::Key;
use super::{Action, Pager};

pub(super) enum Overlay {
    Help,
    Toc {
        /// Index into the view's headings.
        selected: usize,
        /// First heading shown.
        top: usize,
    },
}

const HELP: &[(&str, &str)] = &[
    ("j k ↑ ↓", "line down / up"),
    ("Space f b", "page down / up"),
    ("d u", "half page down / up"),
    ("g G", "top / bottom"),
    ("[ ]", "previous / next heading"),
    ("o t", "table of contents"),
    ("/ n N", "search, next / previous match, Esc clears"),
    ("Tab ⇧Tab", "next / previous link on screen"),
    ("Enter", "open the selected link"),
    ("⌫ H", "back from an opened file or diagram"),
    ("z", "diagram on screen full screen; z Space Esc close it"),
    ("← →", "full screen: previous / next diagram"),
    ("+ - 0", "zoom images and diagrams, reset"),
    (
        "h l ← →",
        "pan wide images, code and tables (h l in a diagram)",
    ),
    ("w", "wrap wide code and tables, or pan them"),
    ("y", "copy the code block on screen"),
    ("c C", "copy the screen / document as plain text"),
    ("m<a-z>", "set a mark"),
    ("'<a-z> ''", "go to a mark / back to before the last jump"),
    ("F", "follow changes to the file"),
    ("q", "quit"),
];

impl Pager<'_> {
    /// Opens the table of contents at the section the top line is in.
    pub(super) fn open_toc(&mut self) {
        let view = self.view();
        if view.doc.headings.is_empty() {
            self.message = Some("No headings".into());
            return;
        }
        let selected = view
            .doc
            .headings
            .iter()
            .rposition(|h| h.line <= view.top)
            .unwrap_or(0);
        self.overlay = Some(Overlay::Toc { selected, top: 0 });
        self.scroll_toc(0);
    }

    /// Moves the contents selection by `delta` and keeps it on screen.
    fn scroll_toc(&mut self, delta: isize) {
        let count = self.view().doc.headings.len();
        // One row is the title.
        let rows = self.view_rows().saturating_sub(1).max(1);
        if let Some(Overlay::Toc { selected, top }) = &mut self.overlay {
            *selected = selected
                .saturating_add_signed(delta)
                .min(count.saturating_sub(1));
            if *selected < *top {
                *top = *selected;
            } else if *selected >= *top + rows {
                *top = *selected + 1 - rows;
            }
        }
    }

    pub(super) fn handle_overlay(&mut self, key: &Key) -> Action {
        let Some(Overlay::Toc { selected, .. }) = &self.overlay else {
            // Any key closes help; `q` quits only from the document.
            self.overlay = None;
            return Action::Continue;
        };
        let selected = *selected;
        let page = self.view_rows().saturating_sub(1).max(1) as isize;
        match key {
            Key::Enter => {
                self.overlay = None;
                if let Some(line) = self.view().doc.headings.get(selected).map(|h| h.line) {
                    self.jump(line);
                }
            }
            Key::Char('j') | Key::Down | Key::Tab | Key::Ctrl(14) => self.scroll_toc(1),
            Key::Char('k') | Key::Up | Key::BackTab | Key::Ctrl(16) => self.scroll_toc(-1),
            Key::Char(' ' | 'f') | Key::PageDown => self.scroll_toc(page),
            Key::Char('b') | Key::PageUp => self.scroll_toc(-page),
            Key::Char('g') | Key::Home => self.scroll_toc(isize::MIN / 2),
            Key::Char('G') | Key::End => self.scroll_toc(isize::MAX / 2),
            Key::Ctrl(3) => return Action::Quit,
            _ => self.overlay = None,
        }
        Action::Continue
    }

    /// Rows of the overlay, without the status line.
    pub(super) fn overlay_rows(&self, overlay: &Overlay) -> Vec<String> {
        let cols = self.size.cols;
        let mut rows = Vec::new();
        match overlay {
            Overlay::Help => {
                rows.push(" \x1b[1mKeys\x1b[0m".to_owned());
                let key_width = HELP
                    .iter()
                    .map(|(k, _)| k.chars().count())
                    .max()
                    .unwrap_or(0);
                for (keys, what) in HELP {
                    let pad = key_width - keys.chars().count();
                    let line = format!("  {keys}{:pad$}  {what}", "");
                    rows.push(truncate(&line, cols));
                }
            }
            Overlay::Toc { selected, top } => {
                rows.push(" \x1b[1mContents\x1b[0m".to_owned());
                let headings = &self.view().doc.headings;
                let shown = self.view_rows().saturating_sub(1);
                for (i, h) in headings.iter().enumerate().skip(*top).take(shown) {
                    let indent = "  ".repeat(usize::from(h.level.saturating_sub(1)));
                    let line = truncate(&format!("  {indent}{}", h.text), cols);
                    rows.push(if i == *selected {
                        let pad = cols.saturating_sub(line.width());
                        format!("\x1b[7m{line}{:pad$}\x1b[27m", "")
                    } else {
                        line
                    });
                }
            }
        }
        rows
    }
}
