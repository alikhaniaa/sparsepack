//! Row and column sums against plain loops, at several thread counts.

mod common;

use std::fs;

use common::{pack, random_any, random_matrix, Matrix, TempDir};
use sparsepack::ops;
use sparsepack::rng::SplitMix64;
use sparsepack::Reader;

fn naive_sums(m: &Matrix) -> (Vec<u64>, Vec<u64>) {
    let mut rows = vec![0u64; m.shape.rows as usize];
    let mut cols = vec![0u64; m.shape.cols as usize];
    for &(r, c, v) in &m.entries {
        rows[r as usize] += u64::from(v);
        cols[c as usize] += u64::from(v);
    }
    (rows, cols)
}

#[test]
fn sums_match_plain_loops() {
    let dir = TempDir::new("sums");
    let mut rng = SplitMix64::new(41);
    for _ in 0..100 {
        let mut m = random_any(&mut rng);
        if m.shape.cols > 10_000 {
            m = random_matrix(&mut rng, m.shape.rows, 1234, 30, 0.2);
        }
        let path = dir.join("m.spk");
        fs::write(&path, pack(&m, [1u32, 7, 128][rng.below(3) as usize])).unwrap();
        let reader = Reader::open(&path).unwrap();
        let (rows, cols) = naive_sums(&m);
        for threads in [1, 3, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let (r, c) = pool.install(|| {
                (
                    ops::row_sums(&path, reader.index()).unwrap(),
                    ops::col_sums(&path, reader.index()).unwrap(),
                )
            });
            assert_eq!(r, rows);
            assert_eq!(c, cols);
        }
    }
}

#[test]
fn column_counts_match_plain_loops() {
    let dir = TempDir::new("colnnz");
    let mut rng = SplitMix64::new(42);
    let m = random_matrix(&mut rng, 500, 300, 40, 0.1);
    let path = dir.join("m.spk");
    fs::write(&path, pack(&m, 16)).unwrap();
    let reader = Reader::open(&path).unwrap();
    let mut want = vec![0u64; 300];
    for &(_, c, _) in &m.entries {
        want[c as usize] += 1;
    }
    assert_eq!(ops::col_nnz(&path, reader.index()).unwrap(), want);
}

#[test]
fn huge_column_counts_are_refused() {
    let dir = TempDir::new("hugecols");
    let path = dir.join("m.spk");
    fs::write(&path, pack(&Matrix::new(3, u64::MAX, vec![(1, 5, 1)]), 4)).unwrap();
    let reader = Reader::open(&path).unwrap();
    assert!(ops::col_sums(&path, reader.index()).is_err());
    // Row sums need no column table.
    assert_eq!(ops::row_sums(&path, reader.index()).unwrap(), [0, 1, 0]);
}

#[test]
fn tiny_files_with_huge_column_counts_fail_cleanly() {
    // A valid 72-byte file (no rows, no blocks) can declare any number of
    // columns. Per-column operations must refuse it with an error instead
    // of trying to allocate the table.
    let dir = TempDir::new("hugetiny");
    let path = dir.join("m.spk");
    for cols in [(1u64 << 27) + 1, 1 << 30, 1 << 32, u64::MAX] {
        let bytes = pack(&Matrix::new(0, cols, vec![]), 1024);
        assert_eq!(bytes.len(), 72);
        fs::write(&path, &bytes).unwrap();
        let reader = Reader::open(&path).unwrap();
        assert!(ops::verify(&path, reader.index()).is_empty());
        assert!(matches!(
            ops::col_sums(&path, reader.index()),
            Err(sparsepack::Error::InvalidInput(_))
        ));
        assert!(ops::col_nnz(&path, reader.index()).is_err());
        let options = ops::FilterOptions {
            min_row_sum: 0,
            min_col_nnz: 0,
            block_rows: 1024,
        };
        let (mut out, mut map) = (Vec::new(), Vec::new());
        assert!(ops::filter(&path, reader.index(), options, &mut out, &mut map).is_err());
        assert!(ops::row_sums(&path, reader.index()).unwrap().is_empty());
    }
    // At the limit itself the table is allocated and the sums are all zero.
    let bytes = pack(&Matrix::new(0, 1 << 20, vec![]), 1024);
    fs::write(&path, &bytes).unwrap();
    let reader = Reader::open(&path).unwrap();
    let sums = ops::col_sums(&path, reader.index()).unwrap();
    assert_eq!(sums.len(), 1 << 20);
    assert!(sums.iter().all(|&s| s == 0));
}
