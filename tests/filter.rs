//! `filter` against a naive in-memory reference implementation.

mod common;

use std::fs;

use common::{pack, random_any, random_matrix, unpack, Entry, Matrix, TempDir};
use sparsepack::ops::{self, FilterOptions};
use sparsepack::rng::SplitMix64;
use sparsepack::Reader;

/// The reference: filter a list of entries with plain loops.
/// Returns the filtered matrix and the map lines (without the header).
fn naive_filter(m: &Matrix, min_row_sum: u64, min_col_nnz: u64) -> (Matrix, Vec<String>) {
    let mut col_count = std::collections::BTreeMap::new();
    let mut row_sum = std::collections::BTreeMap::new();
    for &(r, c, v) in &m.entries {
        *col_count.entry(c).or_insert(0u64) += 1;
        *row_sum.entry(r).or_insert(0u64) += u64::from(v);
    }
    let kept_cols: Vec<u64> = (0..m.shape.cols)
        .filter(|c| col_count.get(c).copied().unwrap_or(0) >= min_col_nnz)
        .collect();
    let kept_rows: Vec<u64> = (0..m.shape.rows)
        .filter(|r| row_sum.get(r).copied().unwrap_or(0) >= min_row_sum)
        .collect();
    let new_index = |kept: &[u64], old: u64| kept.binary_search(&old).ok().map(|i| i as u64);
    let entries: Vec<Entry> = m
        .entries
        .iter()
        .filter_map(|&(r, c, v)| Some((new_index(&kept_rows, r)?, new_index(&kept_cols, c)?, v)))
        .collect();
    let mut map: Vec<String> = Vec::new();
    for (new, old) in kept_cols.iter().enumerate() {
        map.push(format!("col\t{new}\t{old}"));
    }
    for (new, old) in kept_rows.iter().enumerate() {
        map.push(format!("row\t{new}\t{old}"));
    }
    let shape_rows = kept_rows.len() as u64;
    let shape_cols = kept_cols.len() as u64;
    (Matrix::new(shape_rows, shape_cols, entries), map)
}

fn run_filter(
    dir: &TempDir,
    m: &Matrix,
    block_rows: u32,
    min_row_sum: u64,
    min_col_nnz: u64,
) -> (Matrix, Vec<String>) {
    let path = dir.join("in.spk");
    fs::write(&path, pack(m, block_rows)).unwrap();
    let reader = Reader::open(&path).unwrap();
    let mut out = Vec::new();
    let mut map = Vec::new();
    let options = FilterOptions {
        min_row_sum,
        min_col_nnz,
        block_rows,
    };
    let summary = ops::filter(&path, reader.index(), options, &mut out, &mut map).unwrap();
    let result = unpack(&out).unwrap();
    assert_eq!(summary.index.shape(), result.shape);
    assert_eq!(
        summary.dropped_nnz,
        (m.entries.len() - result.entries.len()) as u64
    );
    let map = String::from_utf8(map).unwrap();
    let mut lines = map.lines();
    assert_eq!(lines.next(), Some("# kind\tnew\told"));
    (result, lines.map(String::from).collect())
}

#[test]
fn matches_naive_filter_on_random_matrices() {
    let dir = TempDir::new("filter-random");
    let mut rng = SplitMix64::new(21);
    for _ in 0..150 {
        let mut m = random_any(&mut rng);
        // Per-column tables need a moderate column count.
        if m.shape.cols > 10_000 {
            m = random_matrix(&mut rng, m.shape.rows, 777, 30, 0.3);
        }
        let block_rows = [1u32, 5, 64][rng.below(3) as usize];
        let min_row_sum = [0, 1, 5, 50, u64::MAX][rng.below(5) as usize];
        let min_col_nnz = [0, 1, 2, 10][rng.below(4) as usize];
        let got = run_filter(&dir, &m, block_rows, min_row_sum, min_col_nnz);
        let want = naive_filter(&m, min_row_sum, min_col_nnz);
        assert_eq!(
            got, want,
            "row sum >= {min_row_sum}, col nnz >= {min_col_nnz}"
        );
    }
}

#[test]
fn zero_thresholds_keep_everything() {
    let dir = TempDir::new("filter-all");
    let mut rng = SplitMix64::new(22);
    let m = random_matrix(&mut rng, 300, 200, 10, 0.3);
    let (got, map) = run_filter(&dir, &m, 32, 0, 0);
    assert_eq!(got, m);
    assert_eq!(map.len(), 300 + 200);
}

#[test]
fn thresholds_use_the_input_matrix() {
    // Column 0 has 2 non-zeros, column 1 has 1. Row 0 sums to 1 + 10 = 11.
    // With min_col_nnz = 2 column 1 goes, but row 0 is still kept for
    // min_row_sum = 11 because its sum is taken before columns are dropped.
    let dir = TempDir::new("filter-input");
    let m = Matrix::new(2, 2, vec![(0, 0, 1), (0, 1, 10), (1, 0, 3)]);
    let (got, map) = run_filter(&dir, &m, 4, 11, 2);
    assert_eq!(got, Matrix::new(1, 1, vec![(0, 0, 1)]));
    assert_eq!(map, ["col\t0\t0", "row\t0\t0"]);
}
