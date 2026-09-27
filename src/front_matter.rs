//! YAML front matter shown as a table: each level of nesting is a column,
//! and a key spans the rows of its value.
//!
//! ```text
//! title: Notes          | title  | Notes              |
//! author:               | author | name  | A. Person  |
//!   name: A. Person     |        | email | a@b.c      |
//!   email: a@b.c        | tags   | one                |
//! tags: [one, two]      |        | two                |
//! ```
//!
//! A sequence of scalars is a column of its own rows; a sequence with
//! collections in it numbers its items in a column of their own.

use crate::yaml::Node;

/// Narrowest column, in ems of the body text. Front matter nested so deep
/// that its columns would be narrower is left out.
pub const MIN_COLUMN_EM: f32 = 6.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
    /// Rows and columns the cell spans.
    pub rows: usize,
    pub cols: usize,
    pub text: String,
    /// Keys are set in bold.
    pub key: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub rows: usize,
    pub cols: usize,
    /// In reading order: by row, then column.
    pub cells: Vec<Cell>,
}

/// Columns needed to show `node`.
pub fn columns(node: &Node) -> usize {
    match node {
        Node::Map(entries) if !entries.is_empty() => {
            1 + entries.iter().map(|(_, v)| columns(v)).max().unwrap_or(1)
        }
        Node::Seq(items) if !scalars(items) => 1 + items.iter().map(columns).max().unwrap_or(1),
        _ => 1,
    }
}

fn scalars(items: &[Node]) -> bool {
    items.iter().all(is_scalar)
}

/// Scalars and empty collections take a single cell.
fn is_scalar(node: &Node) -> bool {
    match node {
        Node::Scalar(_) => true,
        Node::Map(e) => e.is_empty(),
        Node::Seq(i) => i.is_empty(),
    }
}

pub fn table(node: &Node) -> Table {
    let cols = columns(node);
    let mut cells = Vec::new();
    let rows = build(node, 0, 0, cols, &mut cells);
    cells.sort_by_key(|c| (c.row, c.col));
    Table { rows, cols, cells }
}

/// Add the cells of `node` with its top left at `row`, `col`, spanning the
/// columns up to `cols`; returns the rows it takes.
fn build(node: &Node, row: usize, col: usize, cols: usize, out: &mut Vec<Cell>) -> usize {
    let mut cell = |row, col, rows, key, text: &str| {
        out.push(Cell {
            row,
            col,
            rows,
            cols: if key { 1 } else { cols - col },
            text: text.to_string(),
            key,
        })
    };
    match node {
        Node::Scalar(s) => {
            cell(row, col, 1, false, s);
            1
        }
        _ if is_scalar(node) => {
            cell(row, col, 1, false, "");
            1
        }
        Node::Seq(items) if scalars(items) => {
            for (k, item) in items.iter().enumerate() {
                build(item, row + k, col, cols, out);
            }
            items.len()
        }
        Node::Seq(items) => {
            let mut r = row;
            for (k, item) in items.iter().enumerate() {
                let n = build(item, r, col + 1, cols, out);
                out.push(Cell {
                    row: r,
                    col,
                    rows: n,
                    cols: 1,
                    text: format!("{}.", k + 1),
                    key: false,
                });
                r += n;
            }
            r - row
        }
        Node::Map(entries) => {
            let mut r = row;
            for (k, v) in entries {
                let n = build(v, r, col + 1, cols, out);
                out.push(Cell {
                    row: r,
                    col,
                    rows: n,
                    cols: 1,
                    text: k.clone(),
                    key: true,
                });
                r += n;
            }
            r - row
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml;

    /// Check the cells of `src`'s table: row, column, rows and columns
    /// spanned, and text.
    fn check(src: &str, expected: &[(usize, usize, usize, usize, &str)]) {
        let t = table(&yaml::parse(src, 1).unwrap());
        let got: Vec<_> = t
            .cells
            .iter()
            .map(|c| (c.row, c.col, c.rows, c.cols, c.text.as_str()))
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn keys_span_their_values() {
        let t = table(&yaml::parse("a: 1\nb:\n  c: 2\n  d: [x, y]\n", 1).unwrap());
        assert_eq!((t.rows, t.cols), (4, 3));
        check(
            "a: 1\nb:\n  c: 2\n  d: [x, y]\n",
            &[
                (0, 0, 1, 1, "a"),
                (0, 1, 1, 2, "1"),
                (1, 0, 3, 1, "b"),
                (1, 1, 1, 1, "c"),
                (1, 2, 1, 1, "2"),
                (2, 1, 2, 1, "d"),
                (2, 2, 1, 1, "x"),
                (3, 2, 1, 1, "y"),
            ],
        );
    }

    #[test]
    fn sequences_of_collections_are_numbered() {
        check(
            "p:\n- n: A\n  r: B\n- C\ne: []\n",
            &[
                (0, 0, 3, 1, "p"),
                (0, 1, 2, 1, "1."),
                (0, 2, 1, 1, "n"),
                (0, 3, 1, 1, "A"),
                (1, 2, 1, 1, "r"),
                (1, 3, 1, 1, "B"),
                (2, 1, 1, 1, "2."),
                (2, 2, 1, 2, "C"),
                (3, 0, 1, 1, "e"),
                (3, 1, 1, 3, ""),
            ],
        );
    }
}
