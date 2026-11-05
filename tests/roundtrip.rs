//! Write a matrix, read it back, and compare with the original.

mod common;

use common::{pack, random_any, random_matrix, unpack, Matrix};
use sparsepack::rng::SplitMix64;
use sparsepack::{Error, Shape, Writer};

/// Block sizes used by most tests: tiny ones create many blocks.
const BLOCK_SIZES: [u32; 6] = [1, 2, 3, 7, 64, 1024];

fn assert_round_trip(m: &Matrix, block_rows: u32) {
    let bytes = pack(m, block_rows);
    let back = unpack(&bytes).expect("read back");
    assert_eq!(&back, m, "block_rows {block_rows}");
}

#[test]
fn empty_matrices() {
    for (rows, cols) in [(0, 0), (0, 5), (5, 0), (3000, 7)] {
        for block_rows in BLOCK_SIZES {
            assert_round_trip(&Matrix::new(rows, cols, vec![]), block_rows);
        }
    }
}

#[test]
fn empty_rows_at_start_middle_and_end() {
    let entries = vec![(3, 0, 1), (3, 9, 2), (4, 4, 7), (10, 1, 1), (10, 2, 3)];
    let m = Matrix::new(15, 10, entries);
    for block_rows in BLOCK_SIZES {
        assert_round_trip(&m, block_rows);
    }
}

#[test]
fn single_row_and_single_column() {
    let row = Matrix::new(1, 1000, vec![(0, 0, 5), (0, 1, 1), (0, 999, u32::MAX)]);
    let col = Matrix::new(1000, 1, vec![(0, 0, 1), (500, 0, 2), (999, 0, 3)]);
    let one = Matrix::new(1, 1, vec![(0, 0, 42)]);
    for block_rows in BLOCK_SIZES {
        assert_round_trip(&row, block_rows);
        assert_round_trip(&col, block_rows);
        assert_round_trip(&one, block_rows);
    }
}

#[test]
fn largest_column_indices() {
    let top = u64::MAX - 1;
    let m = Matrix::new(
        3,
        u64::MAX,
        vec![
            (0, 0, 1),
            (0, top, 2),
            (1, top - 1, 3),
            (1, top, 4),
            (2, top, 1),
        ],
    );
    for block_rows in BLOCK_SIZES {
        assert_round_trip(&m, block_rows);
    }
}

#[test]
fn row_count_around_multiples_of_block_size() {
    let mut rng = SplitMix64::new(11);
    for block_rows in [1u32, 2, 3, 8, 100] {
        for blocks in [1u64, 2, 5] {
            let exact = u64::from(block_rows) * blocks;
            for rows in [exact - 1, exact, exact + 1] {
                let m = random_matrix(&mut rng, rows, 50, 5, 0.2);
                let bytes = pack(&m, block_rows);
                let back = unpack(&bytes).unwrap();
                assert_eq!(back, m, "rows {rows}, block_rows {block_rows}");

                let index =
                    sparsepack::Index::read_from(&mut std::io::Cursor::new(&bytes)).unwrap();
                let expected_blocks = rows.div_ceil(u64::from(block_rows));
                assert_eq!(index.blocks().len() as u64, expected_blocks);
                let last = index.blocks().last();
                if let Some(last) = last {
                    assert_eq!(last.first_row + u64::from(last.row_count), rows);
                }
            }
        }
    }
}

#[test]
fn many_random_matrices() {
    let mut rng = SplitMix64::new(2024);
    for _ in 0..300 {
        let m = random_any(&mut rng);
        let block_rows = BLOCK_SIZES[rng.below(BLOCK_SIZES.len() as u64) as usize];
        assert_round_trip(&m, block_rows);
    }
}

#[test]
fn same_input_gives_same_bytes() {
    let mut rng = SplitMix64::new(5);
    let m = random_matrix(&mut rng, 500, 300, 20, 0.1);
    assert_eq!(pack(&m, 16), pack(&m, 16));
}

#[test]
fn inferred_shape_is_smallest_that_fits() {
    let mut bytes = Vec::new();
    let mut w = Writer::new(&mut bytes, 4).unwrap();
    w.push(2, 7, 1).unwrap();
    w.push(5, 3, 1).unwrap();
    let index = w.finish(None).unwrap();
    assert_eq!(index.shape(), Shape { rows: 6, cols: 8 });
    assert_eq!(unpack(&bytes).unwrap().shape, Shape { rows: 6, cols: 8 });
}

fn push_all(entries: &[(u64, u64, u32)]) -> Result<(), Error> {
    let mut w = Writer::new(Vec::new(), 4)?;
    for &(r, c, v) in entries {
        w.push(r, c, v)?;
    }
    w.finish(None).map(|_| ())
}

#[test]
fn writer_rejects_bad_entries() {
    let unsorted_rows = [(5, 0, 1), (4, 0, 1)];
    let unsorted_cols = [(5, 3, 1), (5, 2, 1)];
    let duplicate = [(5, 3, 1), (5, 3, 2)];
    let zero = [(0, 0, 0)];
    let too_big = [(0, u64::MAX, 1)];
    for entries in [
        &unsorted_rows[..],
        &unsorted_cols,
        &duplicate,
        &zero,
        &too_big,
    ] {
        assert!(
            matches!(push_all(entries), Err(Error::InvalidInput(_))),
            "{entries:?}"
        );
    }
    assert!(push_all(&[(0, 1, 1), (1, 0, 1), (1, 5, 1)]).is_ok());
}

#[test]
fn writer_rejects_entries_outside_the_shape() {
    for shape in [Shape { rows: 2, cols: 10 }, Shape { rows: 10, cols: 3 }] {
        let mut w = Writer::new(Vec::new(), 4).unwrap();
        w.push(2, 3, 1).unwrap();
        assert!(matches!(w.finish(Some(shape)), Err(Error::InvalidInput(_))));
    }
}

#[test]
fn zero_rows_per_block_is_rejected() {
    assert!(matches!(
        Writer::new(Vec::new(), 0),
        Err(Error::InvalidInput(_))
    ));
}
