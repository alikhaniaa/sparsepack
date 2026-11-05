//! Reading row and column ranges through the block index.

mod common;

use std::io::Cursor;
use std::ops::Range;

use common::{pack, random_any, random_matrix, Entry, Matrix};
use sparsepack::rng::SplitMix64;
use sparsepack::Reader;

/// Entries of `rows` x `cols` read through the index, and the number of
/// blocks that were read.
fn read_range(bytes: &[u8], rows: Range<u64>, cols: Range<u64>) -> (Vec<Entry>, usize) {
    let mut reader = Reader::new(Cursor::new(bytes)).unwrap();
    let mut out = Vec::new();
    reader
        .for_each_in(rows, cols, |r, c, v| {
            out.push((r, c, v));
            Ok(())
        })
        .unwrap();
    (out, reader.blocks_read())
}

/// The same range taken from the full list of entries.
fn expected(m: &Matrix, rows: &Range<u64>, cols: &Range<u64>) -> Vec<Entry> {
    m.entries
        .iter()
        .copied()
        .filter(|(r, c, _)| rows.contains(r) && cols.contains(c))
        .collect()
}

/// Number of blocks of `block_rows` rows that overlap `rows` in a matrix
/// with `total` rows, computed by arithmetic instead of the index.
fn blocks_touched(rows: &Range<u64>, total: u64, block_rows: u32) -> usize {
    let end = rows.end.min(total);
    if rows.start >= end {
        return 0;
    }
    let b = u64::from(block_rows);
    ((end - 1) / b - rows.start / b + 1) as usize
}

fn random_range(rng: &mut SplitMix64, limit: u64) -> Range<u64> {
    let a = rng.below(limit + 2);
    let b = rng.below(limit + 2);
    a.min(b)..a.max(b)
}

#[test]
fn random_ranges_match_full_read() {
    let mut rng = SplitMix64::new(77);
    for _ in 0..200 {
        let m = random_any(&mut rng);
        let block_rows = [1u32, 4, 16, 100][rng.below(4) as usize];
        let bytes = pack(&m, block_rows);
        for _ in 0..5 {
            let rows = random_range(&mut rng, m.shape.rows);
            let cols = if rng.below(2) == 0 {
                0..u64::MAX
            } else {
                random_range(&mut rng, m.shape.cols.min(6000))
            };
            let (got, blocks) = read_range(&bytes, rows.clone(), cols.clone());
            assert_eq!(
                got,
                expected(&m, &rows, &cols),
                "rows {rows:?} cols {cols:?}"
            );
            assert_eq!(blocks, blocks_touched(&rows, m.shape.rows, block_rows));
        }
    }
}

#[test]
fn one_row_reads_one_block() {
    let mut rng = SplitMix64::new(3);
    let m = random_matrix(&mut rng, 1000, 200, 10, 0.0);
    let bytes = pack(&m, 64);
    for row in [0, 63, 64, 500, 999] {
        let (got, blocks) = read_range(&bytes, row..row + 1, 0..200);
        assert_eq!(blocks, 1);
        assert_eq!(got, expected(&m, &(row..row + 1), &(0..200)));
    }
}

#[test]
fn ranges_outside_the_matrix_read_nothing() {
    let mut rng = SplitMix64::new(4);
    let m = random_matrix(&mut rng, 100, 50, 10, 0.0);
    let bytes = pack(&m, 16);
    // Struct syntax for the empty and reversed ranges, which clippy would
    // otherwise flag as mistakes.
    let empty = Range { start: 5, end: 5 };
    let reversed = Range { start: 50, end: 10 };
    for rows in [100..200, empty, reversed] {
        let (got, blocks) = read_range(&bytes, rows, 0..50);
        assert!(got.is_empty());
        assert_eq!(blocks, 0);
    }
    let (got, blocks) = read_range(&bytes, 0..100, 50..1000);
    assert!(got.is_empty());
    assert_eq!(blocks, 7);
}
