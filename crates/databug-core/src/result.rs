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

/// Most cell text one result can hold: offsets are `u32`, so 4 GiB.
pub const MAX_TEXT: usize = u32::MAX as usize;

#[derive(Debug, Clone)]
pub struct ResultSet {
    width: usize,
    text: String,
    ends: Vec<u32>,
    nulls: Vec<u64>,
    /// Text limit (`MAX_TEXT`; smaller in tests).
    limit: usize,
    /// Set when text was dropped because the result reached `limit`.
    full: bool,
}

impl Default for ResultSet {
    fn default() -> Self {
        ResultSet { width: 0, text: String::new(), ends: Vec::new(), nulls: Vec::new(), limit: MAX_TEXT, full: false }
    }
}

impl ResultSet {
    pub fn new(width: usize) -> Self {
        ResultSet { width, ..Default::default() }
    }

    pub fn with_capacity(width: usize, rows: usize) -> Self {
        let cells = rows.saturating_mul(width);
        ResultSet {
            width,
            text: String::with_capacity(cells.saturating_mul(8).min(1 << 24)),
            ends: Vec::with_capacity(cells.min(1 << 22)),
            nulls: Vec::with_capacity(cells.div_ceil(64).min(1 << 16)),
            ..Default::default()
        }
    }

    /// Whether this result reached its size limit and stopped storing text.
    pub fn is_full(&self) -> bool {
        self.full
    }

    /// Offset of the end of the arena; always fits because pushes and
    /// appends never let the text grow past `limit` (at most `u32::MAX`).
    fn end_offset(&self) -> u32 {
        u32::try_from(self.text.len()).unwrap_or(u32::MAX)
    }

    /// Undo text written past the limit, so offsets can't wrap around.
    fn enforce_limit(&mut self, before: usize) {
        if self.text.len() > self.limit {
            self.text.truncate(before);
            self.full = true;
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn rows(&self) -> usize {
        self.ends.len().checked_div(self.width).unwrap_or(0)
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
        let before = self.text.len();
        match value {
            Some(v) => self.text.push_str(v),
            None => self.nulls[idx / 64] |= 1 << (idx % 64),
        }
        self.enforce_limit(before);
        self.ends.push(self.end_offset());
    }

    /// Append a cell by writing into the arena directly (avoids a temporary String).
    pub fn push_with(&mut self, f: impl FnOnce(&mut String)) {
        let idx = self.ends.len();
        if idx / 64 >= self.nulls.len() {
            self.nulls.push(0);
        }
        let before = self.text.len();
        f(&mut self.text);
        self.enforce_limit(before);
        self.ends.push(self.end_offset());
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
    /// Returns false, and keeps `self` unchanged, when the combined text
    /// would pass the size limit; the result is then marked full.
    pub fn append(&mut self, other: ResultSet) -> bool {
        if other.full {
            self.full = true;
        }
        if self.ends.is_empty() && self.width == other.width {
            let (limit, full) = (self.limit, self.full);
            *self = other;
            self.limit = limit;
            self.full |= full;
            return true;
        }
        if self.text.len().saturating_add(other.text.len()) > self.limit {
            self.full = true;
            return false;
        }
        let base = self.end_offset();
        let start_idx = self.ends.len();
        self.text.push_str(&other.text);
        // can't overflow: the combined text was checked against the limit above
        self.ends.extend(other.ends.iter().map(|e| e.saturating_add(base)));
        let total = self.ends.len();
        self.nulls.resize(total.div_ceil(64), 0);
        for i in 0..other.ends.len() {
            if other.nulls[i / 64] & (1 << (i % 64)) != 0 {
                let j = start_idx + i;
                self.nulls[j / 64] |= 1 << (j % 64);
            }
        }
        true
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

    fn limited(limit: usize) -> ResultSet {
        ResultSet { width: 1, limit, ..Default::default() }
    }

    #[test]
    fn push_stops_at_the_limit_instead_of_wrapping() {
        let mut r = limited(8);
        r.push(Some("abcd"));
        r.push(Some("efgh"));
        assert!(!r.is_full());
        r.push(Some("ij"));
        assert!(r.is_full());
        // earlier cells stay intact; the overflowing one is stored empty
        assert_eq!(r.cell(0, 0), Some("abcd"));
        assert_eq!(r.cell(1, 0), Some("efgh"));
        assert_eq!(r.cell(2, 0), Some(""));
        r.push_with(|s| s.push_str("klmnop"));
        assert_eq!(r.cell(3, 0), Some(""));
    }

    #[test]
    fn append_refuses_rows_past_the_limit() {
        let mut a = limited(10);
        let mut chunk = ResultSet::new(1);
        chunk.push(Some("123456"));
        assert!(a.append(chunk));
        let mut more = ResultSet::new(1);
        more.push(Some("7890"));
        assert!(a.append(more));
        let mut too_much = ResultSet::new(1);
        too_much.push(Some("x"));
        assert!(!a.append(too_much));
        assert!(a.is_full());
        assert_eq!(a.rows(), 2);
        assert_eq!(a.cell(1, 0), Some("7890"));
    }

    #[test]
    fn huge_capacity_hints_dont_overflow() {
        let r = ResultSet::with_capacity(usize::MAX, usize::MAX);
        assert_eq!(r.rows(), 0);
    }
}
