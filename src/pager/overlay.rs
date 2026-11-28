//! Screens drawn over the document: key help (`?`), the table of contents
//! (`t`) and the views to go back to (`B`).

use unicode_width::UnicodeWidthStr;

use super::draw::truncate;
use super::keys::Key;
use super::view::Kind;
use super::{Action, Pager};
use crate::help;
use crate::render::visible_controls;

pub(super) enum Overlay {
    Help,
    /// A list to pick from, one row each.
    List {
        kind: List,
        /// Index of the row picked by `Enter`.
        selected: usize,
        /// First row shown.
        top: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum List {
    /// The view's headings.
    Toc,
    /// The views followed links led through, the document first: a path,
    /// since going back drops the views above.
    History,
}

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
        self.open_list(List::Toc, selected);
    }

    /// Opens the views to go back to, at the one shown.
    pub(super) fn open_history(&mut self) {
        if self.views.len() < 2 {
            self.message = Some("No links followed: nothing to go back to".into());
            return;
        }
        self.open_list(List::History, self.views.len() - 1);
    }

    fn open_list(&mut self, kind: List, selected: usize) {
        self.overlay = Some(Overlay::List {
            kind,
            selected,
            top: 0,
        });
        self.scroll_list(0);
    }

    fn list_len(&self, kind: List) -> usize {
        match kind {
            List::Toc => self.view().doc.headings.len(),
            List::History => self.views.len(),
        }
    }

    /// Moves the selection by `delta` and keeps it on screen.
    fn scroll_list(&mut self, delta: isize) {
        let Some(Overlay::List { kind, .. }) = &self.overlay else {
            return;
        };
        let count = self.list_len(*kind);
        // One row is the title.
        let rows = self.view_rows().saturating_sub(1).max(1);
        if let Some(Overlay::List { selected, top, .. }) = &mut self.overlay {
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
        let Some(Overlay::List { kind, selected, .. }) = &self.overlay else {
            // Any key closes help; `q` quits only from the document.
            self.overlay = None;
            return Action::Continue;
        };
        let (kind, selected) = (*kind, *selected);
        let page = self.view_rows().saturating_sub(1).max(1) as isize;
        match key {
            Key::Enter => {
                self.overlay = None;
                match kind {
                    List::Toc => {
                        let line = self.view().doc.headings.get(selected).map(|h| h.line);
                        if let Some(line) = line {
                            self.jump(line);
                        }
                    }
                    List::History => return Action::Return(selected),
                }
            }
            Key::Char('j') | Key::Down | Key::Tab | Key::Ctrl(14) => self.scroll_list(1),
            Key::Char('k') | Key::Up | Key::BackTab | Key::Ctrl(16) => self.scroll_list(-1),
            Key::Char(' ' | 'f') | Key::PageDown => self.scroll_list(page),
            Key::Char('b') | Key::PageUp => self.scroll_list(-page),
            Key::Char('g') | Key::Home => self.scroll_list(isize::MIN / 2),
            Key::Char('G') | Key::End => self.scroll_list(isize::MAX / 2),
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
                // The same keys as `--help`, without the topics, to fit the screen.
                let keys = || help::KEYS.iter().flat_map(|(_, keys)| keys.iter());
                let key_width = keys().map(|(k, _)| k.width()).max().unwrap_or(0);
                for (keys, what) in keys() {
                    let pad = key_width - keys.width();
                    let line = format!("  {keys}{:pad$}  {what}", "");
                    rows.push(truncate(&line, cols));
                }
            }
            Overlay::List {
                kind,
                selected,
                top,
            } => {
                let (title, items) = match kind {
                    List::Toc => ("Contents", self.toc_rows()),
                    List::History => ("Back to", self.history_rows()),
                };
                rows.push(format!(" \x1b[1m{title}\x1b[0m"));
                let shown = self.view_rows().saturating_sub(1);
                for (i, item) in items.iter().enumerate().skip(*top).take(shown) {
                    let line = truncate(&format!("  {item}"), cols);
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

    /// Headings, indented by level.
    fn toc_rows(&self) -> Vec<String> {
        self.view()
            .doc
            .headings
            .iter()
            .map(|h| {
                let indent = "  ".repeat(usize::from(h.level.saturating_sub(1)));
                format!("{indent}{}", h.text)
            })
            .collect()
    }

    /// Each view's name, indented by depth, and where it is: the file's
    /// path or the URL. Plain text, since rows are cut by width; names and
    /// paths come from links in documents, so they go through
    /// `visible_controls`.
    fn history_rows(&self) -> Vec<String> {
        (0..self.views.len())
            .map(|depth| {
                let view = &self.views[depth];
                let name = match &view.kind {
                    Kind::Diagram(_) => format!("diagram {}", view.diagram.map_or(1, |i| i + 1)),
                    _ => self.title_of(depth),
                };
                let place = match &view.kind {
                    Kind::Document => self.file.as_ref().map(|p| p.display().to_string()),
                    Kind::File(path) => Some(path.display().to_string()),
                    Kind::Remote { url, .. } => Some(url.clone()),
                    Kind::Diagram(_) => None,
                };
                let indent = "  ".repeat(depth);
                let name = visible_controls(&name, false);
                match place {
                    Some(place) => {
                        let place = visible_controls(&place, false);
                        format!("{indent}{name}   {place}")
                    }
                    None => format!("{indent}{name}"),
                }
            })
            .collect()
    }
}
