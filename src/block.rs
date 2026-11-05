//! A block of consecutive rows: the in-memory form and its compressed bytes.
//!
//! In memory a block uses the compressed sparse row (CSR) layout: all column
//! indices of all rows in one vector, all values in another, and a vector of
//! row boundaries (`row_ptr`). Row `i` of the block is
//! `cols[row_ptr[i]..row_ptr[i + 1]]` (and the same range of `vals`).
//!
//! On disk a block is a sequence of rows, each encoded as
//!
//! ```text
//! n                       number of non-zeros in the row (varint)
//! d_1 d_2 ... d_n         column gaps (varints)
//! v_1 v_2 ... v_n         values (varints)
//! ```
//!
//! The columns of a row are strictly increasing, so they are stored as gaps:
//! `d_1 = c_1` and `d_k = c_k - c_(k-1) - 1`. The `- 1` uses the fact that
//! two columns are never equal, so neighbouring columns give a gap of 0.
//! Small gaps and small values take one byte each as varints.

use crate::varint::Decoder;
use crate::varint::{self, VarintError};

/// The non-zeros of one row, borrowed from a [`Block`] (no copy is made).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row<'a> {
    /// Column indices, strictly increasing.
    pub cols: &'a [u64],
    /// Values, all non-zero; `vals[k]` is the value in column `cols[k]`.
    pub vals: &'a [u32],
}

/// Rows `first_row .. first_row + row_count()` of a matrix, in CSR layout.
///
/// Blocks are produced by the writer (while packing) and by the reader (when
/// decoding a block from a file). Both enforce the invariants: columns
/// strictly increasing within a row and every value non-zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    first_row: u64,
    /// `row_ptr[i]..row_ptr[i + 1]` is the range of row `i` in `cols` and
    /// `vals`. It has one more element than there are rows and starts at 0.
    row_ptr: Vec<usize>,
    cols: Vec<u64>,
    vals: Vec<u32>,
}

impl Block {
    /// An empty block (no rows yet) whose first row is `first_row`.
    pub(crate) fn new(first_row: u64) -> Self {
        Block {
            first_row,
            row_ptr: vec![0],
            cols: Vec::new(),
            vals: Vec::new(),
        }
    }

    /// Append an empty row at the end of the block.
    pub(crate) fn start_row(&mut self) {
        self.row_ptr.push(self.cols.len());
    }

    /// Append one non-zero to the last row. The caller (the writer) checks
    /// that `col` is larger than the previous column of the row.
    pub(crate) fn push(&mut self, col: u64, val: u32) {
        debug_assert!(self.row_ptr.len() > 1, "push before start_row");
        self.cols.push(col);
        self.vals.push(val);
        if let Some(end) = self.row_ptr.last_mut() {
            *end = self.cols.len();
        }
    }

    /// Matrix index of the first row in this block.
    pub fn first_row(&self) -> u64 {
        self.first_row
    }

    /// Number of rows in this block, empty rows included.
    pub fn row_count(&self) -> usize {
        self.row_ptr.len() - 1
    }

    /// Number of non-zeros in this block.
    pub fn nnz(&self) -> usize {
        self.cols.len()
    }

    /// Row `i` of the block (row `first_row() + i` of the matrix).
    ///
    /// Panics if `i >= row_count()`, like indexing a slice out of bounds.
    pub fn row(&self, i: usize) -> Row<'_> {
        let range = self.row_ptr[i]..self.row_ptr[i + 1];
        Row {
            cols: &self.cols[range.clone()],
            vals: &self.vals[range],
        }
    }

    /// All rows with their matrix row index, empty rows included.
    pub fn rows(&self) -> impl Iterator<Item = (u64, Row<'_>)> + '_ {
        (0..self.row_count()).map(move |i| (self.first_row + i as u64, self.row(i)))
    }

    /// All non-zeros as `(row, col, value)` in row-major order.
    pub fn entries(&self) -> impl Iterator<Item = (u64, u64, u32)> + '_ {
        self.rows()
            .flat_map(|(r, row)| row.cols.iter().zip(row.vals).map(move |(&c, &v)| (r, c, v)))
    }

    /// Compressed bytes of the block (see the module documentation).
    pub(crate) fn encode(&self) -> Vec<u8> {
        // Most rows, gaps and values take one byte; this is a guess, not a bound.
        let mut out = Vec::with_capacity(self.row_count() + 2 * self.nnz());
        for i in 0..self.row_count() {
            let row = self.row(i);
            varint::encode(row.cols.len() as u64, &mut out);
            // `next_min` is the smallest column the next entry may have.
            let mut next_min = 0u64;
            for &col in row.cols {
                varint::encode(col - next_min, &mut out);
                next_min = col + 1;
            }
            for &val in row.vals {
                varint::encode(u64::from(val), &mut out);
            }
        }
        out
    }

    /// Decode a block from its compressed bytes.
    ///
    /// `row_count` and `nnz` come from the block index and `ncols` from the
    /// matrix shape. The bytes may be corrupt, so every count is checked
    /// before it is used, and an error message is returned instead of
    /// panicking.
    pub(crate) fn decode(
        bytes: &[u8],
        first_row: u64,
        row_count: u32,
        ncols: u64,
        nnz: u64,
    ) -> Result<Block, String> {
        let rows = row_count as usize;
        // Every row takes at least one byte and every non-zero at least two
        // (gap and value). Checking this first keeps a corrupt count from
        // causing a huge allocation below.
        let room = bytes.len().checked_sub(rows).ok_or_else(|| {
            format!(
                "{} bytes cannot hold {rows} rows (at least one byte per row)",
                bytes.len()
            )
        })?;
        if nnz > (room / 2) as u64 {
            return Err(format!(
                "{} bytes cannot hold {rows} rows with {nnz} non-zeros",
                bytes.len()
            ));
        }
        let mut block = Block {
            first_row,
            row_ptr: Vec::with_capacity(rows + 1),
            cols: Vec::with_capacity(nnz as usize),
            vals: Vec::with_capacity(nnz as usize),
        };
        block.row_ptr.push(0);

        let mut dec = Decoder::new(bytes);
        let at = |dec: &Decoder<'_>, err: VarintError| format!("byte {}: {err}", dec.position());
        for i in 0..rows {
            let n = dec.read().map_err(|e| at(&dec, e))?;
            if n > (dec.remaining() / 2) as u64 {
                return Err(format!(
                    "row {} claims {n} non-zeros, more than the remaining bytes can hold",
                    first_row + i as u64
                ));
            }
            let mut next_min = 0u64;
            for _ in 0..n {
                let gap = dec.read().map_err(|e| at(&dec, e))?;
                let col = next_min
                    .checked_add(gap)
                    .filter(|&c| c < ncols)
                    .ok_or_else(|| {
                        format!(
                            "row {}: column out of range (matrix has {ncols} columns)",
                            first_row + i as u64
                        )
                    })?;
                block.cols.push(col);
                next_min = col + 1;
            }
            for _ in 0..n {
                let val = dec.read().map_err(|e| at(&dec, e))?;
                if val == 0 || val > u64::from(u32::MAX) {
                    return Err(format!(
                        "row {}: value {val} is not in 1..=4294967295",
                        first_row + i as u64
                    ));
                }
                block.vals.push(val as u32);
            }
            block.row_ptr.push(block.cols.len());
        }
        if dec.remaining() != 0 {
            return Err(format!(
                "{} unused bytes after the last row",
                dec.remaining()
            ));
        }
        if block.nnz() as u64 != nnz {
            return Err(format!(
                "index says {nnz} non-zeros, the block has {}",
                block.nnz()
            ));
        }
        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows: [], [3:1, 4:2, 10:300], [] starting at matrix row 5.
    fn example() -> Block {
        let mut block = Block::new(5);
        block.start_row();
        block.start_row();
        block.push(3, 1);
        block.push(4, 2);
        block.push(10, 300);
        block.start_row();
        block
    }

    #[test]
    fn accessors() {
        let block = example();
        assert_eq!(block.first_row(), 5);
        assert_eq!(block.row_count(), 3);
        assert_eq!(block.nnz(), 3);
        assert_eq!(block.row(0).cols, &[] as &[u64]);
        assert_eq!(block.row(1).cols, &[3, 4, 10]);
        assert_eq!(block.row(1).vals, &[1, 2, 300]);
        let entries: Vec<_> = block.entries().collect();
        assert_eq!(entries, [(6, 3, 1), (6, 4, 2), (6, 10, 300)]);
        let row_ids: Vec<u64> = block.rows().map(|(r, _)| r).collect();
        assert_eq!(row_ids, [5, 6, 7]);
    }

    #[test]
    fn known_encoding() {
        // Row 0: n=0. Row 1: n=3, gaps 3, 0 (4-3-1), 5 (10-4-1), values
        // 1, 2, 300 (two bytes). Row 2: n=0.
        let expected = [0x00, 0x03, 0x03, 0x00, 0x05, 0x01, 0x02, 0xac, 0x02, 0x00];
        assert_eq!(example().encode(), expected);
    }

    #[test]
    fn decode_inverts_encode() {
        let block = example();
        let bytes = block.encode();
        assert_eq!(Block::decode(&bytes, 5, 3, 11, 3), Ok(block));
    }

    #[test]
    fn decode_rejects_bad_input() {
        let bytes = example().encode();
        // Wrong context from the index.
        assert!(
            Block::decode(&bytes, 5, 3, 10, 3).is_err(),
            "column 10 >= 10 columns"
        );
        assert!(Block::decode(&bytes, 5, 3, 11, 4).is_err(), "wrong nnz");
        assert!(
            Block::decode(&bytes, 5, 2, 11, 3).is_err(),
            "trailing bytes"
        );
        assert!(Block::decode(&bytes, 5, 4, 11, 3).is_err(), "missing row");
        // Every cut-off prefix fails.
        for len in 0..bytes.len() {
            assert!(
                Block::decode(&bytes[..len], 5, 3, 11, 3).is_err(),
                "prefix {len}"
            );
        }
        // A zero value, and a value above u32::MAX.
        assert!(Block::decode(&[0x01, 0x00, 0x00], 0, 1, 1, 1).is_err());
        let mut big = vec![0x01, 0x00];
        varint::encode(1 << 32, &mut big);
        assert!(Block::decode(&big, 0, 1, 1, 1).is_err());
        // A row that claims more non-zeros than there are bytes.
        let mut huge = Vec::new();
        varint::encode(u64::MAX, &mut huge);
        assert!(Block::decode(&huge, 0, 1, u64::MAX, 0).is_err());
        // An index that claims more non-zeros than the bytes can hold.
        assert!(Block::decode(&[0x00], 0, 1, 10, u64::MAX).is_err());
        assert!(Block::decode(&[], 0, u32::MAX, 10, 0).is_err());
    }
}
