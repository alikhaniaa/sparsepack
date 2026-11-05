//! The operations behind the command-line tool, as library functions.
//!
//! Operations that touch every block decode the blocks in parallel with
//! rayon. Each worker opens its own handle to the file, so no file handle
//! (and no seek position) is shared between threads.

use std::fs::File;
use std::io::{self, BufRead, Write};
use std::ops::Range;
use std::path::Path;

use rayon::prelude::*;

use crate::block::Block;
use crate::error::{Error, Result};
use crate::format::{Index, Shape};
use crate::text::{self, InputFormat, TripletReader};
use crate::writer::Writer;

/// Blocks handed to the thread pool at once by streaming operations, per
/// worker thread. Larger batches use more memory, smaller ones leave
/// threads idle at the end of each batch.
const BATCH_BLOCKS_PER_THREAD: usize = 4;

/// Read block `i` through a worker's own file handle (or report why the
/// handle could not be opened).
fn read_with(file: &mut io::Result<File>, index: &Index, i: usize) -> Result<Block> {
    match file {
        Ok(file) => index.read_block(file, i),
        // io::Error is not Clone, so copy its kind and message.
        Err(err) => Err(Error::Io(io::Error::new(err.kind(), err.to_string()))),
    }
}

/// Read and decode blocks `range` of the file at `path` in parallel and
/// return `f(i, block_or_error)` for each block `i`, in block order.
fn par_blocks<T, F>(path: &Path, index: &Index, range: Range<usize>, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(usize, Result<Block>) -> T + Sync,
{
    range
        .into_par_iter()
        // `map_init` runs the first closure once per rayon job and passes
        // its result to every call of the second closure in that job.
        .map_init(
            || File::open(path),
            |file, i| f(i, read_with(file, index, i)),
        )
        .collect()
}

/// Number of columns as a table length, refusing shapes whose per-column
/// table would not fit in memory.
fn column_table_len(shape: Shape) -> Result<usize> {
    const LIMIT: u64 = 1 << 32;
    if shape.cols > LIMIT {
        return Err(Error::InvalidInput(format!(
            "{} columns: per-column tables are limited to {LIMIT} columns",
            shape.cols
        )));
    }
    Ok(shape.cols as usize)
}

fn overflow() -> Error {
    Error::InvalidInput("a total does not fit in 64 bits".to_string())
}

/// One total per column over the whole matrix, computed in parallel.
///
/// Every rayon job folds its blocks into its own table with `add`, so no
/// locking is needed; the tables are then added up pairwise (`try_reduce`).
fn column_totals<F>(path: &Path, index: &Index, add: F) -> Result<Vec<u64>>
where
    F: Fn(&mut [u64], &Block) -> Result<()> + Sync,
{
    let cols = column_table_len(index.shape())?;
    (0..index.blocks().len())
        .into_par_iter()
        .map_init(|| File::open(path), |file, i| read_with(file, index, i))
        .try_fold(
            || vec![0u64; cols],
            |mut table, block| {
                add(&mut table, &block?)?;
                Ok(table)
            },
        )
        .try_reduce(
            || vec![0u64; cols],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x = x.checked_add(y).ok_or_else(overflow)?;
                }
                Ok(a)
            },
        )
}

/// Number of non-zeros in each column.
pub fn col_nnz(path: &Path, index: &Index) -> Result<Vec<u64>> {
    column_totals(path, index, |table, block| {
        for (_, col, _) in block.entries() {
            table[col as usize] += 1;
        }
        Ok(())
    })
}

/// Read blocks `range` of the file at `path` in parallel, apply `f` to each
/// decoded block and return the results in block order, or the first error.
pub fn map_blocks<T, F>(path: &Path, index: &Index, range: Range<usize>, f: F) -> Result<Vec<T>>
where
    T: Send,
    F: Fn(Block) -> Result<T> + Sync,
{
    par_blocks(path, index, range, |_, block| block.and_then(&f))
        .into_iter()
        .collect()
}

/// Check every block: read it, compare its CRC-32 and decode it. Returns
/// one error per damaged block (empty when the file is intact). The header
/// and footer were already checked when `index` was read.
pub fn verify(path: &Path, index: &Index) -> Vec<Error> {
    par_blocks(path, index, 0..index.blocks().len(), |_, block| block.err())
        .into_iter()
        .flatten()
        .collect()
}

/// Number of decimal digits of `n`.
fn digits(n: u64) -> u64 {
    u64::from(n.checked_ilog10().unwrap_or(0)) + 1
}

/// Size in bytes of the TSV text that [`unpack`] writes for this file,
/// computed by decoding every block (in parallel) without formatting it.
pub fn tsv_size(path: &Path, index: &Index) -> Result<u64> {
    let per_block = map_blocks(path, index, 0..index.blocks().len(), |block| {
        Ok(block
            .entries()
            .map(|(r, c, v)| digits(r) + digits(c) + digits(u64::from(v)) + 3)
            .sum::<u64>())
    })?;
    let header = text::shape_line(index.shape()).len() as u64;
    Ok(header + per_block.iter().sum::<u64>())
}

/// Add a line number to input errors from the writer (sort order, zeros).
fn at_line(err: Error, line: u64) -> Error {
    match err {
        Error::InvalidInput(msg) => Error::InvalidInput(format!("line {line}: {msg}")),
        other => other,
    }
}

/// Pack text in `format` from `input` into a sparsepack file written to
/// `out`, with `block_rows` rows per block. The input is streamed: only the
/// blocks waiting for compression are held in memory.
pub fn pack<R: BufRead, W: Write>(
    input: R,
    format: InputFormat,
    out: W,
    block_rows: u32,
) -> Result<Index> {
    let mut reader = TripletReader::new(input, format);
    let mut writer = Writer::new(out, block_rows)?;
    while let Some(entry) = reader.next() {
        let (row, col, value) = entry?;
        writer
            .push(row, col, value)
            .map_err(|err| at_line(err, reader.line_number()))?;
    }
    writer.finish(reader.shape())
}

/// Write the whole matrix as TSV triplets: the `# shape` line, then one line
/// per non-zero in row-major order.
///
/// Blocks are decoded and formatted in parallel, a batch at a time, and
/// written in order.
pub fn unpack<W: Write>(path: &Path, index: &Index, mut out: W) -> Result<()> {
    out.write_all(text::shape_line(index.shape()).as_bytes())?;
    let n = index.blocks().len();
    let batch = BATCH_BLOCKS_PER_THREAD * rayon::current_num_threads();
    for start in (0..n).step_by(batch) {
        let end = (start + batch).min(n);
        let chunks = map_blocks(path, index, start..end, |block| {
            let mut buf = Vec::with_capacity(16 * block.nnz());
            for (row, col, value) in block.entries() {
                text::push_triplet(&mut buf, row, col, value);
            }
            Ok(buf)
        })?;
        for chunk in chunks {
            out.write_all(&chunk)?;
        }
    }
    out.flush()?;
    Ok(())
}

/// Thresholds and outputs of [`filter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterOptions {
    /// Keep a row if the sum of its values in the input is at least this.
    pub min_row_sum: u64,
    /// Keep a column if it has at least this many non-zeros in the input.
    pub min_col_nnz: u64,
    /// Rows per block of the output file.
    pub block_rows: u32,
}

/// What [`filter`] kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterSummary {
    /// Index of the written file (shape: kept rows x kept columns).
    pub index: Index,
    /// Non-zeros of the input that were dropped.
    pub dropped_nnz: u64,
}

/// Marks a dropped column in the old-to-new column table.
const DROPPED: u64 = u64::MAX;

/// Drop sparse rows and columns, renumber the rest, and write the result.
///
/// Both thresholds are evaluated on the **input** matrix, independently:
///
/// - a column is kept if it has at least `min_col_nnz` non-zeros over all
///   input rows (pass 1, in parallel over blocks);
/// - a row is kept if the sum of its values over all input columns is at
///   least `min_row_sum` (pass 2, while rewriting).
///
/// Kept rows and columns keep their order and are renumbered `0, 1, 2, ...`.
/// The filter runs once and is not repeated until nothing changes, so an
/// output row can sum to less than `min_row_sum` (its values in dropped
/// columns are gone) and an output column can have fewer than `min_col_nnz`
/// non-zeros (some were in dropped rows). A kept row may even be empty.
///
/// `map` receives the renumbering as TSV: a `# kind new old` header, one
/// `col NEW OLD` line per kept column, then one `row NEW OLD` line per kept
/// row. `row 5 17` means: row 5 of the output is row 17 of the input.
pub fn filter<W: Write, M: Write>(
    path: &Path,
    index: &Index,
    options: FilterOptions,
    out: W,
    mut map: M,
) -> Result<FilterSummary> {
    // Pass 1: column counts, then the old-to-new column table.
    let counts = col_nnz(path, index)?;
    let mut col_map = vec![DROPPED; counts.len()];
    let mut kept_cols = 0u64;
    let mut line = Vec::new();
    line.extend_from_slice(b"# kind\tnew\told\n");
    for (old, &n) in counts.iter().enumerate() {
        if n >= options.min_col_nnz {
            col_map[old] = kept_cols;
            line.extend_from_slice(b"col\t");
            text::push_u64(&mut line, kept_cols);
            line.push(b'\t');
            text::push_u64(&mut line, old as u64);
            line.push(b'\n');
            kept_cols += 1;
        }
    }
    map.write_all(&line)?;

    // Pass 2: decode blocks a batch at a time (in parallel), then decide
    // row by row and push the kept entries, renumbered, to the writer.
    let mut writer = Writer::new(out, options.block_rows)?;
    let mut kept_rows = 0u64;
    let mut kept_nnz = 0u64;
    let n = index.blocks().len();
    let batch = BATCH_BLOCKS_PER_THREAD * rayon::current_num_threads();
    for start in (0..n).step_by(batch) {
        let blocks = map_blocks(path, index, start..(start + batch).min(n), Ok)?;
        for block in &blocks {
            for (old_row, row) in block.rows() {
                let sum = row
                    .vals
                    .iter()
                    .try_fold(0u64, |acc, &v| acc.checked_add(u64::from(v)))
                    .ok_or_else(overflow)?;
                if sum < options.min_row_sum {
                    continue;
                }
                for (&col, &value) in row.cols.iter().zip(row.vals) {
                    let new_col = col_map[col as usize];
                    if new_col != DROPPED {
                        writer.push(kept_rows, new_col, value)?;
                        kept_nnz += 1;
                    }
                }
                line.clear();
                line.extend_from_slice(b"row\t");
                text::push_u64(&mut line, kept_rows);
                line.push(b'\t');
                text::push_u64(&mut line, old_row);
                line.push(b'\n');
                map.write_all(&line)?;
                kept_rows += 1;
            }
        }
    }
    map.flush()?;
    let index_out = writer.finish(Some(Shape {
        rows: kept_rows,
        cols: kept_cols,
    }))?;
    Ok(FilterSummary {
        index: index_out,
        dropped_nnz: index.nnz() - kept_nnz,
    })
}
