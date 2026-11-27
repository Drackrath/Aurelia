//! PowerShell `Format-Table` style tables.

use unicode_width::UnicodeWidthStr;

/// Column alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Right,
}

/// Rows under Title-case headers.
#[derive(Debug, Clone, Default)]
pub(crate) struct Table {
    cols: Vec<(String, Align)>,
    rows: Vec<Vec<String>>,
}

/// Narrowest last-column wrap room.
const MIN_ROOM: usize = 20;

impl Table {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Add a left-aligned column.
    pub(crate) fn left(mut self, header: &str) -> Self {
        self.cols.push((header.to_string(), Align::Left));
        self
    }

    /// Add a right-aligned column.
    pub(crate) fn right(mut self, header: &str) -> Self {
        self.cols.push((header.to_string(), Align::Right));
        self
    }

    /// Append a row; missing cells empty.
    pub(crate) fn row<I, S>(&mut self, cells: I)
    where
        I: IntoIterator<Item = S>,
        S: ToString,
    {
        self.rows
            .push(cells.into_iter().map(|c| c.to_string()).collect());
    }

    /// Render for terminal `width`.
    pub(crate) fn render(&self, width: usize) -> String {
        let n = self.cols.len();
        if n == 0 {
            return String::new();
        }
        let cell = |r: &Vec<String>, i: usize| -> String {
            // Newlines would break the grid.
            r.get(i).map(|c| c.replace('\n', " ")).unwrap_or_default()
        };
        let widths: Vec<usize> = (0..n)
            .map(|i| {
                self.rows
                    .iter()
                    .map(|r| cell(r, i).width())
                    .chain(std::iter::once(self.cols[i].0.width()))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let last = n - 1;
        let lead: usize = widths[..last].iter().sum::<usize>() + last;
        let wraps = self.cols[last].1 == Align::Left;
        let room = width.saturating_sub(lead).max(MIN_ROOM);

        let line = |cells: &[String]| -> String {
            let mut out = String::new();
            for (i, text) in cells.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                let pad = " ".repeat(widths[i].saturating_sub(text.width()));
                match self.cols[i].1 {
                    Align::Left if i == last => out.push_str(text),
                    Align::Left => {
                        out.push_str(text);
                        out.push_str(&pad);
                    }
                    Align::Right => {
                        out.push_str(&pad);
                        out.push_str(text);
                    }
                }
            }
            out.trim_end().to_string()
        };

        let headers: Vec<String> = self.cols.iter().map(|(h, _)| h.clone()).collect();
        let rule: Vec<String> = self
            .cols
            .iter()
            .map(|(h, _)| "-".repeat(h.width()))
            .collect();
        let mut out = format!("{}\n{}\n", line(&headers), line(&rule));
        for r in &self.rows {
            let mut cells: Vec<String> = (0..n).map(|i| cell(r, i)).collect();
            let tail = if wraps {
                wrap(&cells[last], room)
            } else {
                vec![cells[last].clone()]
            };
            let mut tail = tail.into_iter();
            cells[last] = tail.next().unwrap_or_default();
            out.push_str(&line(&cells));
            out.push('\n');
            for more in tail {
                out.push_str(&" ".repeat(lead));
                out.push_str(&more);
                out.push('\n');
            }
        }
        out
    }

    /// Print at the terminal width.
    pub(crate) fn print(&self) {
        crate::cli_print!("{}", self.render(super::terminal_width()));
    }
}

/// Greedy word wrap to `room` columns.
pub(crate) fn wrap(text: &str, room: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let next = current.width() + usize::from(!current.is_empty()) + word.width();
        if !current.is_empty() && next > room {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}
