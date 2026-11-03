//! Compact, row-major result storage.
//!
//! All cell text for a result lives in one `String` arena with a `u32` end
//! offset per cell and a null bitmap, so a cell costs 4 bytes plus its text
//! instead of a 24-byte `String` header and a separate heap allocation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub type_name: String,
}

impl ColumnMeta {
    pub fn new(name: impl Into<String>, type_name: impl Into<String>) -> Self {
        ColumnMeta { name: name.into(), type_name: type_name.into() }
    }

    /// Heuristic used for right-aligning and numeric sorting.
    pub fn is_numeric(&self) -> bool {
        let t = self.type_name.to_ascii_lowercase();
        ["int", "numeric", "decimal", "float", "double", "real", "money", "serial", "number"]
            .iter()
            .any(|k| t.contains(k))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    width: usize,
    text: String,
    ends: Vec<u32>,
    nulls: Vec<u64>,
}

impl ResultSet {
    pub fn new(width: usize) -> Self {
        ResultSet { width, ..Default::default() }
    }

    pub fn with_capacity(width: usize, rows: usize) -> Self {
        ResultSet {
            width,
            text: String::with_capacity(rows * width * 8),
            ends: Vec::with_capacity(rows * width),
            nulls: Vec::with_capacity((rows * width).div_ceil(64)),
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn rows(&self) -> usize {
        if self.width == 0 {
            0
        } else {
            self.ends.len() / self.width
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// Append one cell. Cells must be pushed row by row, left to right.
    pub fn push(&mut self, value: Option<&str>) {
        let idx = self.ends.len();
        if idx / 64 >= self.nulls.len() {
            self.nulls.push(0);
        }
        match value {
            Some(v) => self.text.push_str(v),
            None => self.nulls[idx / 64] |= 1 << (idx % 64),
        }
        self.ends.push(self.text.len() as u32);
    }

    /// Append a cell by writing into the arena directly (avoids a temporary String).
    pub fn push_with(&mut self, f: impl FnOnce(&mut String)) {
        let idx = self.ends.len();
        if idx / 64 >= self.nulls.len() {
            self.nulls.push(0);
        }
        f(&mut self.text);
        self.ends.push(self.text.len() as u32);
    }

    /// Remove the last row.
    pub fn pop_row(&mut self) {
        let n = self.rows();
        if n == 0 {
            return;
        }
        let new_len = (n - 1) * self.width;
        for idx in new_len..self.ends.len() {
            self.nulls[idx / 64] &= !(1 << (idx % 64));
        }
        self.ends.truncate(new_len);
        let text_len = if new_len == 0 { 0 } else { self.ends[new_len - 1] as usize };
        self.text.truncate(text_len);
    }

    /// Mark the most recently pushed cell as NULL (it must not have written text).
    pub fn mark_last_null(&mut self) {
        if let Some(idx) = self.ends.len().checked_sub(1) {
            self.nulls[idx / 64] |= 1 << (idx % 64);
        }
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&str> {
        let idx = row * self.width + col;
        if idx >= self.ends.len() || self.nulls[idx / 64] & (1 << (idx % 64)) != 0 {
            return None;
        }
        let start = if idx == 0 { 0 } else { self.ends[idx - 1] as usize };
        Some(&self.text[start..self.ends[idx] as usize])
    }

    /// Move all rows from `other` (same width) onto the end of `self`.
    pub fn append(&mut self, other: ResultSet) {
        if self.ends.is_empty() && self.width == other.width {
            *self = other;
            return;
        }
        let base = self.text.len() as u32;
        let start_idx = self.ends.len();
        self.text.push_str(&other.text);
        self.ends.extend(other.ends.iter().map(|e| e + base));
        let total = self.ends.len();
        self.nulls.resize(total.div_ceil(64), 0);
        for i in 0..other.ends.len() {
            if other.nulls[i / 64] & (1 << (i % 64)) != 0 {
                let j = start_idx + i;
                self.nulls[j / 64] |= 1 << (j % 64);
            }
        }
    }

    /// Bytes held on the heap by this result.
    pub fn heap_bytes(&self) -> usize {
        self.text.capacity() + self.ends.capacity() * 4 + self.nulls.capacity() * 8
    }

    pub fn shrink_to_fit(&mut self) {
        self.text.shrink_to_fit();
        self.ends.shrink_to_fit();
        self.nulls.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_read() {
        let mut r = ResultSet::new(2);
        r.push(Some("a"));
        r.push(None);
        r.push(Some(""));
        r.push(Some("dd"));
        assert_eq!(r.rows(), 2);
        assert_eq!(r.cell(0, 0), Some("a"));
        assert_eq!(r.cell(0, 1), None);
        assert_eq!(r.cell(1, 0), Some(""));
        assert_eq!(r.cell(1, 1), Some("dd"));
    }

    #[test]
    fn pop_row_clears_nulls() {
        let mut r = ResultSet::new(1);
        r.push(Some("a"));
        r.push(None);
        r.pop_row();
        r.push(Some("b"));
        assert_eq!(r.cell(1, 0), Some("b"));
        assert_eq!(r.rows(), 2);
    }

    #[test]
    fn append_keeps_nulls_and_offsets() {
        let mut a = ResultSet::new(1);
        for i in 0..70 {
            a.push(if i % 3 == 0 { None } else { Some("x") });
        }
        let mut b = ResultSet::new(1);
        b.push(None);
        b.push(Some("yz"));
        a.append(b);
        assert_eq!(a.rows(), 72);
        assert_eq!(a.cell(69, 0), None);
        assert_eq!(a.cell(70, 0), None);
        assert_eq!(a.cell(71, 0), Some("yz"));
        assert_eq!(a.cell(1, 0), Some("x"));
    }
}
