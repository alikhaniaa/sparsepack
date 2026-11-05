//! Helpers shared by the integration tests.

// Every test file compiles this module separately and uses only some of the
// helpers, so the others would be reported as dead code.
#![allow(dead_code)]

use std::io::Cursor;

use sparsepack::rng::SplitMix64;
use sparsepack::{Index, Result, Shape, Writer};

/// One non-zero: (row, column, value).
pub type Entry = (u64, u64, u32);

/// A matrix as a sorted list of non-zeros, the simplest possible reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matrix {
    pub shape: Shape,
    pub entries: Vec<Entry>,
}

impl Matrix {
    pub fn new(rows: u64, cols: u64, entries: Vec<Entry>) -> Self {
        Matrix {
            shape: Shape { rows, cols },
            entries,
        }
    }
}

/// A random value: mostly small, sometimes large, sometimes `u32::MAX`.
pub fn random_value(rng: &mut SplitMix64) -> u32 {
    match rng.below(10) {
        0 => u32::MAX - rng.below(3) as u32,
        1 => 1 + rng.below(1 << 20) as u32,
        _ => 1 + rng.below(5) as u32,
    }
}

/// A random matrix: each row is empty with probability `p_empty`, otherwise
/// it has up to `max_per_row` non-zeros in random columns.
pub fn random_matrix(
    rng: &mut SplitMix64,
    rows: u64,
    cols: u64,
    max_per_row: u64,
    p_empty: f64,
) -> Matrix {
    let mut entries = Vec::new();
    let mut row_cols = Vec::new();
    for row in 0..rows {
        if cols == 0 || rng.next_f64() < p_empty {
            continue;
        }
        row_cols.clear();
        for _ in 0..=rng.below(max_per_row) {
            row_cols.push(rng.below(cols));
        }
        row_cols.sort_unstable();
        row_cols.dedup();
        for &col in &row_cols {
            entries.push((row, col, random_value(rng)));
        }
    }
    Matrix::new(rows, cols, entries)
}

/// A random matrix with random shape, density and empty-row rate.
pub fn random_any(rng: &mut SplitMix64) -> Matrix {
    let rows = rng.below(300);
    let cols = match rng.below(4) {
        0 => 1 + rng.below(3),
        1 => 1 + rng.below(100),
        2 => 1 + rng.below(5000),
        _ => u64::MAX - rng.below(2),
    };
    let max_per_row = 1 + rng.below(40);
    let p_empty = [0.0, 0.1, 0.5, 0.95][rng.below(4) as usize];
    random_matrix(rng, rows, cols, max_per_row, p_empty)
}

/// Pack a matrix into an in-memory file.
pub fn pack(m: &Matrix, block_rows: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut writer = Writer::new(&mut bytes, block_rows).expect("header");
    for &(r, c, v) in &m.entries {
        writer.push(r, c, v).expect("push");
    }
    writer.finish(Some(m.shape)).expect("finish");
    bytes
}

/// Read every block of an in-memory file back into a matrix.
pub fn unpack(bytes: &[u8]) -> Result<Matrix> {
    let mut src = Cursor::new(bytes);
    let index = Index::read_from(&mut src)?;
    let mut entries = Vec::new();
    for i in 0..index.blocks().len() {
        let block = index.read_block(&mut src, i)?;
        entries.extend(block.entries());
    }
    Ok(Matrix {
        shape: index.shape(),
        entries,
    })
}

/// A fresh directory under the system temp directory, deleted on drop.
pub struct TempDir(std::path::PathBuf);

impl TempDir {
    /// `name` must be unique among the tests of one test binary.
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sparsepack-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    /// Path of `file` inside the directory.
    pub fn join(&self, file: &str) -> std::path::PathBuf {
        self.0.join(file)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Text of a matrix in the TSV form that `unpack` writes.
pub fn to_tsv(m: &Matrix) -> String {
    let mut text = format!("# shape {} {}\n", m.shape.rows, m.shape.cols);
    for (r, c, v) in &m.entries {
        text.push_str(&format!("{r}\t{c}\t{v}\n"));
    }
    text
}
