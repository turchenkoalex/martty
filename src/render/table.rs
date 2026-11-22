//! Tables with box-drawing borders. A table wider than the available width
//! shrinks its columns and wraps text inside the cells.

use pulldown_cmark::Alignment;

use super::Renderer;
use super::span::{Span, spans_width};
use super::wrap::{Token, tokenize, wrap};
use crate::style::{Color, Style};

pub(super) type Cell = Vec<Span>;

pub(super) struct Table {
    pub(super) aligns: Vec<Alignment>,
    pub(super) rows: Vec<Vec<Cell>>,
    pub(super) header_rows: usize,
    pub(super) row: Vec<Cell>,
}

impl Renderer<'_> {
    pub(super) fn render_table(&mut self, table: &Table) {
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let column = |i: usize| table.rows.iter().filter_map(move |row| row.get(i));
        let natural: Vec<usize> = (0..cols)
            .map(|i| column(i).map(|c| line_width(c)).max().unwrap_or(0).max(1))
            .collect();
        let words: Vec<usize> = (0..cols)
            .map(|i| column(i).map(|c| longest_word(c)).max().unwrap_or(0).max(1))
            .collect();
        // Each column takes its content plus a space on both sides and a
        // border on the right; the table adds one border on the left.
        let widths = if self.cfg.overflow {
            natural
        } else {
            let budget = self.avail().saturating_sub(3 * cols + 1);
            column_widths(&natural, &words, budget)
        };

        let border = Style::fg(Color::Ansi(8));
        let rule = |l: &str, m: &str, r: &str| {
            let inner: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            vec![Span::new(
                format!("{l}{}{r}", inner.join(m)),
                border.clone(),
            )]
        };

        let rows: Vec<Vec<Vec<Cell>>> = table
            .rows
            .iter()
            .enumerate()
            .map(|(r, row)| {
                let header = r < table.header_rows;
                widths
                    .iter()
                    .enumerate()
                    .map(|(i, &width)| {
                        let mut cell = row.get(i).cloned().unwrap_or_default();
                        if header {
                            cell.iter_mut().for_each(|s| s.style.bold = true);
                        }
                        wrap(&cell, width)
                    })
                    .collect()
            })
            .collect();
        // Once a cell wraps, rows can't be told apart by lines alone.
        let wrapped = rows.iter().flatten().any(|lines| lines.len() > 1);

        self.emit_wide_line(&rule("┌", "┬", "┐"));
        for (r, cells) in rows.iter().enumerate() {
            if r > 0 && (r == table.header_rows || wrapped) {
                self.emit_wide_line(&rule("├", "┼", "┤"));
            }
            let height = cells.iter().map(Vec::len).max().unwrap_or(0).max(1);
            for n in 0..height {
                let mut line = vec![Span::new("│", border.clone())];
                for (i, &width) in widths.iter().enumerate() {
                    let text = cells[i].get(n).cloned().unwrap_or_default();
                    let gap = width.saturating_sub(spans_width(&text));
                    let left = match table.aligns.get(i) {
                        Some(Alignment::Right) => gap,
                        Some(Alignment::Center) => gap / 2,
                        _ => 0,
                    };
                    line.push(Span::new(" ".repeat(left + 1), Style::default()));
                    line.extend(text);
                    line.push(Span::new(" ".repeat(gap - left + 1), Style::default()));
                    line.push(Span::new("│", border.clone()));
                }
                self.emit_wide_line(&line);
            }
        }
        self.emit_wide_line(&rule("└", "┴", "┘"));
    }
}

/// Width of the widest line of a cell (`<br>` breaks it into lines).
fn line_width(cell: &[Span]) -> usize {
    wrap(cell, usize::MAX)
        .iter()
        .map(|l| spans_width(l))
        .max()
        .unwrap_or(0)
}

fn longest_word(cell: &[Span]) -> usize {
    tokenize(cell)
        .iter()
        .filter_map(|t| match t {
            Token::Word(parts) => Some(spans_width(parts)),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// Column widths in cells that fit `budget`, the way browsers lay out
/// tables: natural widths when they fit; otherwise every column gets its
/// longest word and the rest is shared in proportion to how much more each
/// column wants. When even the words don't fit, they will be split and the
/// share starts from one cell per column.
fn column_widths(natural: &[usize], words: &[usize], budget: usize) -> Vec<usize> {
    if natural.iter().sum::<usize>() <= budget {
        return natural.to_vec();
    }
    let min: Vec<usize> = if words.iter().sum::<usize>() <= budget {
        words.iter().zip(natural).map(|(w, n)| *w.min(n)).collect()
    } else {
        vec![1; natural.len()]
    };
    let slack: Vec<usize> = natural.iter().zip(&min).map(|(n, m)| n - m).collect();
    let total_slack: usize = slack.iter().sum();
    let extra = budget.saturating_sub(min.iter().sum());
    let mut widths: Vec<usize> = min
        .iter()
        .zip(&slack)
        .map(|(m, s)| m + extra * s / total_slack.max(1))
        .collect();
    // Hand out what the rounding down left, one cell at a time, to the
    // columns furthest from their natural width.
    let mut left = extra.saturating_sub(widths.iter().zip(&min).map(|(w, m)| w - m).sum());
    while left > 0 {
        let Some(i) = (0..widths.len())
            .filter(|&i| widths[i] < natural[i])
            .max_by_key(|&i| natural[i] - widths[i])
        else {
            break;
        };
        widths[i] += 1;
        left -= 1;
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_widths_should_keep_natural_widths_that_fit() {
        assert_eq!(column_widths(&[5, 10], &[5, 4], 20), [5, 10]);
    }

    #[test]
    fn column_widths_should_shrink_only_columns_with_room_to_wrap() {
        // The first column is a single word: it keeps its width.
        assert_eq!(column_widths(&[8, 40], &[8, 6], 30), [8, 22]);
    }

    #[test]
    fn column_widths_should_share_proportionally_to_content() {
        assert_eq!(column_widths(&[20, 60], &[4, 4], 40), [11, 29]);
    }

    #[test]
    fn column_widths_should_fill_budget_exactly() {
        let widths = column_widths(&[33, 33, 33], &[3, 3, 3], 50);
        assert_eq!(widths.iter().sum::<usize>(), 50);
    }

    #[test]
    fn column_widths_should_split_words_when_they_do_not_fit() {
        assert_eq!(column_widths(&[10, 10], &[10, 10], 10), [5, 5]);
    }
}
