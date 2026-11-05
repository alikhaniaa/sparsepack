//! Random access to the rows of a sparsepack file.
//!
//! A [`Reader`] reads the footer once when it is created. After that, every
//! request reads only the blocks it needs, and the reader counts them so
//! that callers (and tests) can check how much of the file was touched.

use std::fs::File;
use std::io::{Read, Seek};
use std::ops::Range;
use std::path::Path;

use crate::block::Block;
use crate::error::Result;
use crate::format::Index;

/// A sparsepack file opened for reading.
///
/// `R` is any seekable byte source: a [`File`], or a `Cursor<Vec<u8>>` for
/// data in memory.
pub struct Reader<R> {
    src: R,
    index: Index,
    blocks_read: usize,
    bytes_read: u64,
}

impl Reader<File> {
    /// Open the file at `path` and read its footer.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(File::open(path)?)
    }
}

impl<R: Read + Seek> Reader<R> {
    /// Read the header, trailer and footer from `src`.
    pub fn new(mut src: R) -> Result<Self> {
        let index = Index::read_from(&mut src)?;
        Ok(Reader {
            src,
            index,
            blocks_read: 0,
            bytes_read: 0,
        })
    }

    /// The block index (shape, nnz and block entries).
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Read and decode block `i`.
    pub fn read_block(&mut self, i: usize) -> Result<Block> {
        let block = self.index.read_block(&mut self.src, i)?;
        self.blocks_read += 1;
        self.bytes_read += u64::from(self.index.blocks()[i].len);
        Ok(block)
    }

    /// The underlying byte source.
    pub fn get_ref(&self) -> &R {
        &self.src
    }

    /// Number of blocks read so far.
    pub fn blocks_read(&self) -> usize {
        self.blocks_read
    }

    /// Number of compressed block bytes read so far (footer not counted).
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Call `f(row, col, value)` for every non-zero with a row in `rows` and
    /// a column in `cols`, in row-major order. Only the blocks that overlap
    /// `rows` are read; within a row, the column range is found by binary
    /// search.
    pub fn for_each_in<F>(&mut self, rows: Range<u64>, cols: Range<u64>, mut f: F) -> Result<()>
    where
        F: FnMut(u64, u64, u32) -> Result<()>,
    {
        for i in self.index.blocks_for_rows(rows.clone()) {
            let block = self.read_block(i)?;
            for (r, row) in block.rows() {
                if !rows.contains(&r) {
                    continue;
                }
                let lo = row.cols.partition_point(|&c| c < cols.start);
                let hi = row.cols.partition_point(|&c| c < cols.end).max(lo);
                for k in lo..hi {
                    f(r, row.cols[k], row.vals[k])?;
                }
            }
        }
        Ok(())
    }
}
