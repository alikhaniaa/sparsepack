//! Packing MatrixMarket input gives the same file as packing the TSV form.

mod common;

use common::{random_any, to_tsv, Matrix};
use sparsepack::ops;
use sparsepack::rng::SplitMix64;
use sparsepack::text::InputFormat;

/// The matrix as a MatrixMarket coordinate file (1-based indices).
fn to_mtx(m: &Matrix, comments: bool) -> String {
    let mut text = String::from("%%MatrixMarket matrix coordinate integer general\n");
    if comments {
        text.push_str("% a comment line\n%\n");
    }
    text.push_str(&format!(
        "{} {} {}\n",
        m.shape.rows,
        m.shape.cols,
        m.entries.len()
    ));
    for (r, c, v) in &m.entries {
        text.push_str(&format!("{} {} {v}\n", r + 1, c + 1));
    }
    text
}

fn pack(text: &str, format: InputFormat, block_rows: u32) -> Vec<u8> {
    let mut out = Vec::new();
    ops::pack(text.as_bytes(), format, &mut out, block_rows).expect("pack");
    out
}

#[test]
fn matrix_market_and_tsv_give_identical_files() {
    let mut rng = SplitMix64::new(31);
    for k in 0..100 {
        // Includes matrices with u64::MAX columns: the largest 0-based
        // column, 2^64 - 2, is still a valid 1-based u64.
        let m = random_any(&mut rng);
        let block_rows = [1u32, 8, 1024][k % 3];
        let from_tsv = pack(&to_tsv(&m), InputFormat::Tsv, block_rows);
        let from_mtx = pack(
            &to_mtx(&m, k % 2 == 0),
            InputFormat::MatrixMarket,
            block_rows,
        );
        assert_eq!(from_tsv, from_mtx, "matrix {k}");
    }
}

#[test]
fn unsorted_matrix_market_is_rejected() {
    let text = "%%MatrixMarket matrix coordinate integer general\n3 3 2\n2 1 1\n1 1 1\n";
    let mut out = Vec::new();
    let err = ops::pack(text.as_bytes(), InputFormat::MatrixMarket, &mut out, 4).unwrap_err();
    assert!(err.to_string().contains("line 4"), "{err}");
    assert!(err.to_string().contains("sorted"), "{err}");
}
