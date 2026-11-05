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
use crate::format::Index;
use crate::text::{self, InputFormat, TripletReader};
use crate::writer::Writer;

/// Blocks handed to the thread pool at once by streaming operations, per
/// worker thread. Larger batches use more memory, smaller ones leave
/// threads idle at the end of each batch.
const BATCH_BLOCKS_PER_THREAD: usize = 4;

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
            |file, i| {
                let block = match file {
                    Ok(file) => index.read_block(file, i),
                    // io::Error is not Clone, so copy its kind and message.
                    Err(err) => Err(Error::Io(io::Error::new(err.kind(), err.to_string()))),
                };
                f(i, block)
            },
        )
        .collect()
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
