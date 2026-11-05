//! Seeded generator of synthetic sparse count matrices.
//!
//! Think of documents x terms or users x items: a few columns are very
//! popular and most are rare. The model is deliberately simple:
//!
//! - Column popularity follows a Zipf law: the column of popularity rank `k`
//!   (0-based) has weight `1 / (k + 1)^s`. `s = 0` gives uniform columns;
//!   `s = 1` is the classic word-frequency shape. The ranks are assigned to
//!   column indices by a seeded random permutation, so popular columns are
//!   spread over the index range instead of sitting at the start (which
//!   would make the column gaps unrealistically small).
//! - Each row draws its number of non-zeros uniformly from `0..=2 * mean`
//!   (capped at the number of columns), so the average is `mean` whenever
//!   `2 * mean <= cols`.
//! - The columns of a row are drawn from the Zipf law without repetition: if
//!   a drawn rank is already used in this row, the next unused rank is taken.
//! - Values are geometric: 1 with probability 1/2, 2 with 1/4, 3 with 1/8 and
//!   so on, capped at `max_value`.
//!
//! The random numbers come from [`SplitMix64`], so the output depends only on
//! the parameters. The Zipf table uses floating-point `powf`, whose last bit
//! may in principle differ between platforms' math libraries; on one machine
//! the output is always the same.

use crate::error::{Error, Result};
use crate::format::Shape;
use crate::rng::SplitMix64;
use crate::table;

/// Parameters of a generated matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct GenParams {
    /// Number of rows.
    pub rows: u64,
    /// Number of columns. The generator keeps three 8-byte tables with one
    /// entry per column, so together they stay within
    /// [`MAX_TABLE_ENTRIES`](crate::table::MAX_TABLE_ENTRIES) entries: at
    /// most 44,739,242 columns.
    pub cols: u64,
    /// Mean number of non-zeros per row.
    pub mean_nnz: u64,
    /// Zipf exponent of column popularity (0 = uniform).
    pub zipf: f64,
    /// Seed of the random number generator.
    pub seed: u64,
    /// Largest value (at least 1).
    pub max_value: u32,
}

impl GenParams {
    /// Shape of the generated matrix.
    pub fn shape(&self) -> Shape {
        Shape {
            rows: self.rows,
            cols: self.cols,
        }
    }
}

/// Generate the matrix and call `emit(row, col, value)` for each non-zero,
/// in row-major order (the order the [`Writer`](crate::Writer) needs).
pub fn generate<F>(params: &GenParams, mut emit: F) -> Result<()>
where
    F: FnMut(u64, u64, u32) -> Result<()>,
{
    if params.max_value == 0 {
        return Err(Error::InvalidInput(
            "max value must be at least 1".to_string(),
        ));
    }
    if !(params.zipf.is_finite() && params.zipf >= 0.0) {
        return Err(Error::InvalidInput(format!(
            "Zipf exponent {} must be a finite number >= 0",
            params.zipf
        )));
    }
    let max_cols = table::MAX_TABLE_ENTRIES / 3;
    let cols = usize::try_from(params.cols)
        .ok()
        .filter(|&c| c <= max_cols)
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "the generator keeps three tables with one entry per column; \
                 {} columns is above the limit of {max_cols}",
                params.cols
            ))
        })?;
    if cols == 0 {
        return Ok(()); // every row is empty
    }

    let mut rng = SplitMix64::new(params.seed);
    let cdf = zipf_cdf(cols, params.zipf)?;
    // rank_to_col[k] is the column index of the column with popularity rank k.
    let mut rank_to_col = table::filled(cols, 0u64)?;
    for (rank, col) in rank_to_col.iter_mut().enumerate() {
        *col = rank as u64;
    }
    for i in (1..cols).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        rank_to_col.swap(i, j);
    }
    // used[k] == row + 1 marks rank k as taken in the current row, so the
    // table never needs clearing between rows.
    let mut used = table::filled(cols, 0u64)?;
    let max_draws = params.mean_nnz.saturating_mul(2).saturating_add(1);
    let mut row_cols: Vec<u64> = Vec::new();

    for row in 0..params.rows {
        let mark = row + 1;
        let n = rng.below(max_draws).min(params.cols);
        row_cols.clear();
        for _ in 0..n {
            let u = rng.next_f64();
            // First rank whose cumulative probability exceeds u.
            let mut rank = cdf.partition_point(|&p| p <= u).min(cols - 1);
            while used[rank] == mark {
                rank = (rank + 1) % cols;
            }
            used[rank] = mark;
            row_cols.push(rank_to_col[rank]);
        }
        row_cols.sort_unstable();
        for &col in &row_cols {
            let value = (1 + rng.next_u64().trailing_zeros()).min(params.max_value);
            emit(row, col, value)?;
        }
    }
    Ok(())
}

/// Cumulative Zipf probabilities: `cdf[k]` = P(rank <= k). The last element
/// is exactly 1.
fn zipf_cdf(cols: usize, s: f64) -> Result<Vec<f64>> {
    let mut cdf = table::filled(cols, 0.0)?;
    let mut total = 0.0;
    for (k, p) in cdf.iter_mut().enumerate() {
        total += ((k + 1) as f64).powf(-s);
        *p = total;
    }
    for p in &mut cdf {
        *p /= total;
    }
    if let Some(last) = cdf.last_mut() {
        *last = 1.0;
    }
    Ok(cdf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(rows: u64, cols: u64, mean_nnz: u64, zipf: f64, seed: u64) -> GenParams {
        GenParams {
            rows,
            cols,
            mean_nnz,
            zipf,
            seed,
            max_value: 1000,
        }
    }

    fn entries(p: &GenParams) -> Vec<(u64, u64, u32)> {
        let mut out = Vec::new();
        generate(p, |r, c, v| {
            out.push((r, c, v));
            Ok(())
        })
        .unwrap();
        out
    }

    #[test]
    fn same_seed_same_matrix() {
        let p = params(200, 500, 20, 1.0, 7);
        assert_eq!(entries(&p), entries(&p));
        let q = GenParams {
            seed: 8,
            ..p.clone()
        };
        assert_ne!(entries(&p), entries(&q));
    }

    #[test]
    fn entries_are_sorted_distinct_and_in_range() {
        let p = params(300, 50, 20, 1.2, 1);
        let e = entries(&p);
        assert!(e.windows(2).all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1)));
        assert!(e
            .iter()
            .all(|&(r, c, v)| r < 300 && c < 50 && (1..=1000).contains(&v)));
    }

    #[test]
    fn mean_nnz_per_row_is_close_to_requested() {
        let p = params(2000, 10_000, 50, 1.0, 3);
        let mean = entries(&p).len() as f64 / 2000.0;
        assert!((mean - 50.0).abs() < 2.0, "mean {mean}");
    }

    #[test]
    fn dense_rows_are_capped_at_the_column_count() {
        // 2 * mean > cols: rows can be completely full but never more.
        let e = entries(&params(50, 10, 100, 1.0, 4));
        for row in 0..50 {
            assert!(e.iter().filter(|x| x.0 == row).count() <= 10);
        }
    }

    #[test]
    fn zipf_makes_some_columns_popular() {
        let count_per_col = |zipf: f64| {
            let mut counts = vec![0u32; 1000];
            for (_, c, _) in entries(&params(2000, 1000, 10, zipf, 5)) {
                counts[c as usize] += 1;
            }
            counts.sort_unstable();
            (counts[999], counts[500])
        };
        let (top, median) = count_per_col(1.0);
        assert!(top > 20 * median.max(1), "top {top}, median {median}");
        let (top, median) = count_per_col(0.0);
        assert!(top < 3 * median, "uniform: top {top}, median {median}");
    }

    #[test]
    fn values_are_geometric_and_capped() {
        let e = entries(&params(1000, 1000, 20, 1.0, 6));
        let ones = e.iter().filter(|x| x.2 == 1).count() as f64 / e.len() as f64;
        assert!((ones - 0.5).abs() < 0.02, "fraction of ones {ones}");
        let capped = GenParams {
            max_value: 2,
            ..params(1000, 1000, 20, 1.0, 6)
        };
        assert!(entries(&capped).iter().all(|x| x.2 <= 2));
    }

    #[test]
    fn rejects_bad_parameters() {
        let mut p = params(10, 10, 2, 1.0, 1);
        p.max_value = 0;
        assert!(generate(&p, |_, _, _| Ok(())).is_err());
        let p = params(10, 10, 2, -1.0, 1);
        assert!(generate(&p, |_, _, _| Ok(())).is_err());
        let p = params(10, 10, 2, f64::NAN, 1);
        assert!(generate(&p, |_, _, _| Ok(())).is_err());
        let p = params(10, 1 << 40, 2, 1.0, 1);
        assert!(generate(&p, |_, _, _| Ok(())).is_err());
        let p = params(10, 44_739_243, 2, 1.0, 1);
        assert!(generate(&p, |_, _, _| Ok(())).is_err());
    }

    #[test]
    fn zero_columns_give_empty_rows() {
        assert!(entries(&params(10, 0, 5, 1.0, 1)).is_empty());
    }
}
