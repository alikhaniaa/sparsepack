//! Streaming writer for sparsepack files.
//!
//! Entries are pushed one at a time, sorted by row and then by column. Rows
//! are collected into the current block; when a row index beyond the block
//! arrives, the block is closed and queued. Queued blocks are compressed and
//! written in batches, so memory use depends on the batch size, not on the
//! size of the matrix. The block index stays in memory (36 bytes per block)
//! and is written as the footer by [`Writer::finish`].
//!
//! Blocks are independent, so a batch is compressed in parallel on the rayon
//! thread pool. The output does not depend on the number of threads: the
//! compressed blocks are collected in order and written one after another.

use std::io::Write;

use rayon::prelude::*;

use crate::block::Block;
use crate::crc32::crc32;
use crate::error::{Error, Result};
use crate::format::{self, BlockEntry, Index, Shape, HEADER_LEN};

/// Flush the queue when it holds this many blocks per worker thread...
const PENDING_BLOCKS_PER_THREAD: usize = 2;
/// ...or this many non-zeros (about 48 MiB in memory at 12 bytes each).
const MAX_PENDING_NNZ: usize = 4 << 20;

/// Writes a sparsepack file to any `Write` destination (a file, a
/// `Vec<u8>`, ...). The destination does not need to support seeking.
pub struct Writer<W: Write> {
    out: W,
    block_rows: u32,
    /// Bytes written so far, which is the offset of the next block.
    written: u64,
    /// The block that receives new rows.
    current: Block,
    /// Full blocks waiting to be compressed and written.
    pending: Vec<Block>,
    pending_nnz: usize,
    max_pending_blocks: usize,
    /// Index entries of the blocks written so far.
    entries: Vec<BlockEntry>,
    nnz: u64,
    /// Position of the last pushed entry, to check the sort order.
    last: Option<(u64, u64)>,
    max_col: Option<u64>,
}

impl<W: Write> Writer<W> {
    /// Start a file with `block_rows` rows per block and write its header.
    pub fn new(mut out: W, block_rows: u32) -> Result<Self> {
        if block_rows == 0 {
            return Err(Error::InvalidInput(
                "a block must have at least 1 row".to_string(),
            ));
        }
        out.write_all(&format::header_bytes(block_rows))?;
        Ok(Writer {
            out,
            block_rows,
            written: HEADER_LEN as u64,
            current: Block::new(0),
            pending: Vec::new(),
            pending_nnz: 0,
            max_pending_blocks: PENDING_BLOCKS_PER_THREAD * rayon::current_num_threads(),
            entries: Vec::new(),
            nnz: 0,
            last: None,
            max_col: None,
        })
    }

    /// Add the non-zero `value` at (`row`, `col`).
    ///
    /// Entries must arrive sorted by row, then by column, with no position
    /// repeated. A zero value is rejected because only non-zeros are stored.
    pub fn push(&mut self, row: u64, col: u64, value: u32) -> Result<()> {
        if value == 0 {
            return Err(Error::InvalidInput(format!(
                "value 0 at row {row}, column {col}: only non-zero values are stored"
            )));
        }
        if row == u64::MAX || col == u64::MAX {
            return Err(Error::InvalidInput(format!(
                "index ({row}, {col}) is too large (the largest index is 2^64 - 2)"
            )));
        }
        if let Some((last_row, last_col)) = self.last {
            if row < last_row {
                return Err(Error::InvalidInput(format!(
                    "row {row} comes after row {last_row}: rows must be sorted"
                )));
            }
            if row == last_row && col <= last_col {
                let problem = if col == last_col {
                    format!("duplicate entry at row {row}, column {col}")
                } else {
                    format!("column {col} comes after column {last_col} in row {row}")
                };
                return Err(Error::InvalidInput(format!(
                    "{problem}: columns must be strictly increasing within a row"
                )));
            }
        }
        self.move_to_row(row)?;
        self.current.push(col, value);
        self.last = Some((row, col));
        self.max_col = Some(self.max_col.map_or(col, |m| m.max(col)));
        self.nnz += 1;
        Ok(())
    }

    /// Make `row` the last row of the current block, closing full blocks and
    /// adding empty rows on the way.
    fn move_to_row(&mut self, row: u64) -> Result<()> {
        let block_rows = u64::from(self.block_rows);
        while row >= self.current.first_row() + block_rows {
            self.close_block()?;
        }
        while self.current.first_row() + (self.current.row_count() as u64) <= row {
            self.current.start_row();
        }
        Ok(())
    }

    /// Fill the current block up to `block_rows` rows, queue it and start
    /// the next one.
    fn close_block(&mut self) -> Result<()> {
        while self.current.row_count() < self.block_rows as usize {
            self.current.start_row();
        }
        let next = Block::new(self.current.first_row() + u64::from(self.block_rows));
        let full = std::mem::replace(&mut self.current, next);
        self.queue(full)
    }

    fn queue(&mut self, block: Block) -> Result<()> {
        self.pending_nnz += block.nnz();
        self.pending.push(block);
        if self.pending.len() >= self.max_pending_blocks || self.pending_nnz >= MAX_PENDING_NNZ {
            self.flush_pending()?;
        }
        Ok(())
    }

    /// Compress all queued blocks in parallel, then write them in row order.
    fn flush_pending(&mut self) -> Result<()> {
        // The collected results are in block order, so the file does not
        // depend on the number of threads.
        let compressed: Vec<(Vec<u8>, u32)> = self
            .pending
            .par_iter()
            .map(|block| {
                let bytes = block.encode();
                let crc = crc32(&bytes);
                (bytes, crc)
            })
            .collect();
        for (block, (bytes, crc)) in self.pending.drain(..).zip(compressed) {
            let len = u32::try_from(bytes.len()).map_err(|_| {
                Error::InvalidInput(format!(
                    "block at row {} is larger than 4 GiB; use fewer rows per block",
                    block.first_row()
                ))
            })?;
            self.out.write_all(&bytes)?;
            self.entries.push(BlockEntry {
                offset: self.written,
                len,
                first_row: block.first_row(),
                row_count: block.row_count() as u32,
                nnz: block.nnz() as u64,
                crc,
            });
            self.written += u64::from(len);
        }
        self.pending_nnz = 0;
        Ok(())
    }

    /// Write the remaining blocks, the footer and the trailer.
    ///
    /// With `Some(shape)` the matrix gets exactly that shape (trailing empty
    /// rows are written as empty rows), and an error is returned if a pushed
    /// entry lies outside it. With `None` the shape is the smallest that
    /// holds every entry: last row + 1 by largest column + 1.
    pub fn finish(mut self, shape: Option<Shape>) -> Result<Index> {
        let inferred = Shape {
            rows: self.last.map_or(0, |(row, _)| row + 1),
            cols: self.max_col.map_or(0, |col| col + 1),
        };
        let shape = match shape {
            Some(s) if s.rows < inferred.rows || s.cols < inferred.cols => {
                return Err(Error::InvalidInput(format!(
                    "entries need a {} x {} matrix, but the shape is {} x {}",
                    inferred.rows, inferred.cols, s.rows, s.cols
                )));
            }
            Some(s) => s,
            None => inferred,
        };
        if shape.rows > 0 {
            self.move_to_row(shape.rows - 1)?;
            let last = std::mem::replace(&mut self.current, Block::new(shape.rows));
            self.queue(last)?;
        }
        self.flush_pending()?;

        let footer_offset = self.written;
        let index = Index::new(self.block_rows, shape, self.nnz, self.entries);
        self.out.write_all(&index.footer_bytes())?;
        self.out.write_all(&format::trailer_bytes(footer_offset))?;
        self.out.flush()?;
        Ok(index)
    }
}
