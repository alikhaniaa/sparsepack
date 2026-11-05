//! The file layout: header, blocks, footer (the block index) and trailer.
//!
//! ```text
//! +--------+---------+---------+-----+---------+--------+---------+
//! | header | block 0 | block 1 | ... | block n | footer | trailer |
//! +--------+---------+---------+-----+---------+--------+---------+
//!   20 B      variable length, compressed        36+36n B   16 B
//! ```
//!
//! All fixed-size integers are little-endian. `docs/FORMAT.md` has the
//! byte-level tables. A reader first reads the header and the trailer, then
//! the footer that the trailer points to, and after that only the blocks it
//! needs.

use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::ops::Range;

use crate::block::Block;
use crate::crc32::crc32;
use crate::error::{Error, Result};

/// First 8 bytes of every sparsepack file.
pub const MAGIC: [u8; 8] = *b"SPARSEPK";
/// Last 8 bytes of every complete sparsepack file.
pub const TRAILER_MAGIC: [u8; 8] = *b"SPKTRAIL";
/// Format version written by this code (and the only one it reads).
pub const VERSION: u16 = 1;
/// Rows per block when the caller does not choose.
pub const DEFAULT_BLOCK_ROWS: u32 = 1024;

/// Bytes in the header: magic (8), version (2), reserved (2), block rows
/// (4), CRC-32 of the first 16 bytes (4).
pub const HEADER_LEN: usize = 20;
/// Bytes in the fixed part of the footer: rows, columns, nnz, block count.
pub const FOOTER_FIXED_LEN: usize = 32;
/// Bytes per block entry in the footer.
pub const ENTRY_LEN: usize = 36;
/// Bytes of the footer checksum.
pub const FOOTER_CRC_LEN: usize = 4;
/// Bytes in the trailer: footer offset (8) and the trailer magic (8).
pub const TRAILER_LEN: usize = 16;

/// Number of rows and columns of a matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// Number of rows; valid row indices are `0..rows`.
    pub rows: u64,
    /// Number of columns; valid column indices are `0..cols`.
    pub cols: u64,
}

/// Where one block is in the file and what it contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEntry {
    /// Byte offset of the block from the start of the file.
    pub offset: u64,
    /// Length of the compressed block in bytes.
    pub len: u32,
    /// Matrix index of the block's first row.
    pub first_row: u64,
    /// Number of rows in the block, empty rows included.
    pub row_count: u32,
    /// Number of non-zeros in the block.
    pub nnz: u64,
    /// CRC-32 of the block's bytes.
    pub crc: u32,
}

impl BlockEntry {
    /// The matrix rows this block holds.
    pub fn rows(&self) -> Range<u64> {
        // Saturating, so that even an unchecked entry cannot overflow.
        self.first_row..self.first_row.saturating_add(u64::from(self.row_count))
    }
}

/// Everything a reader knows after reading header, footer and trailer: the
/// matrix shape and the location of every block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    block_rows: u32,
    shape: Shape,
    nnz: u64,
    blocks: Vec<BlockEntry>,
}

/// Reads little-endian integers from the front of a byte slice, failing
/// (instead of panicking) when the slice is too short.
struct LeReader<'a> {
    bytes: &'a [u8],
    part: &'static str,
}

impl<'a> LeReader<'a> {
    fn new(bytes: &'a [u8], part: &'static str) -> Self {
        Self { bytes, part }
    }

    /// The next `N` bytes. `N` is a const generic: the compiler makes one
    /// version of this function for each array size that is used.
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        match self.bytes.split_first_chunk::<N>() {
            Some((head, rest)) => {
                self.bytes = rest;
                Ok(*head)
            }
            None => Err(Error::Corrupt(format!("{} ends early", self.part))),
        }
    }

    fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64> {
        self.array().map(u64::from_le_bytes)
    }
}

/// Header bytes for a file with `block_rows` rows per block.
pub(crate) fn header_bytes(block_rows: u32) -> [u8; HEADER_LEN] {
    let mut out = [0u8; HEADER_LEN];
    out[0..8].copy_from_slice(&MAGIC);
    out[8..10].copy_from_slice(&VERSION.to_le_bytes());
    // Bytes 10..12 are reserved and stay zero.
    out[12..16].copy_from_slice(&block_rows.to_le_bytes());
    let crc = crc32(&out[..16]);
    out[16..20].copy_from_slice(&crc.to_le_bytes());
    out
}

/// Check a header and return its block size in rows.
fn parse_header(bytes: &[u8; HEADER_LEN]) -> Result<u32> {
    let mut r = LeReader::new(bytes, "header");
    if r.array::<8>()? != MAGIC {
        return Err(Error::NotSparsepack);
    }
    let version = r.u16()?;
    let reserved = r.u16()?;
    let block_rows = r.u32()?;
    let stored = r.u32()?;
    let computed = crc32(&bytes[..16]);
    if stored != computed {
        return Err(Error::ChecksumMismatch {
            part: "header".to_string(),
            stored,
            computed,
        });
    }
    if version != VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    if reserved != 0 {
        return Err(Error::Corrupt(format!(
            "reserved header field is {reserved}, not 0"
        )));
    }
    if block_rows == 0 {
        return Err(Error::Corrupt("block size of 0 rows".to_string()));
    }
    Ok(block_rows)
}

/// Trailer bytes pointing at a footer that starts at `footer_offset`.
pub(crate) fn trailer_bytes(footer_offset: u64) -> [u8; TRAILER_LEN] {
    let mut out = [0u8; TRAILER_LEN];
    out[0..8].copy_from_slice(&footer_offset.to_le_bytes());
    out[8..16].copy_from_slice(&TRAILER_MAGIC);
    out
}

/// Seek to `offset` and fill `buf` from there. Running out of data is
/// reported as a truncated file rather than as a bare I/O error.
fn read_at<R: Read + Seek>(src: &mut R, offset: u64, buf: &mut [u8]) -> Result<()> {
    src.seek(SeekFrom::Start(offset))?;
    src.read_exact(buf).map_err(|err| {
        if err.kind() == ErrorKind::UnexpectedEof {
            Error::Truncated(format!(
                "{} bytes at offset {offset} are missing",
                buf.len()
            ))
        } else {
            Error::Io(err)
        }
    })
}

impl Index {
    /// Assemble an index; used by the writer once all blocks are written.
    pub(crate) fn new(block_rows: u32, shape: Shape, nnz: u64, blocks: Vec<BlockEntry>) -> Self {
        Index {
            block_rows,
            shape,
            nnz,
            blocks,
        }
    }

    /// Rows per block. Every block has this many rows except the last one,
    /// which may have fewer.
    pub fn block_rows(&self) -> u32 {
        self.block_rows
    }

    /// Shape of the matrix.
    pub fn shape(&self) -> Shape {
        self.shape
    }

    /// Number of non-zeros in the whole matrix.
    pub fn nnz(&self) -> u64 {
        self.nnz
    }

    /// Entries of all blocks, in row order.
    pub fn blocks(&self) -> &[BlockEntry] {
        &self.blocks
    }

    /// Indices of the blocks that hold at least one row of `rows`.
    ///
    /// The entries are sorted by row, so two binary searches over the index
    /// find the first and the last block; no block data is read.
    pub fn blocks_for_rows(&self, rows: Range<u64>) -> Range<usize> {
        if rows.is_empty() {
            return 0..0;
        }
        let start = self.blocks.partition_point(|b| b.rows().end <= rows.start);
        let end = self.blocks.partition_point(|b| b.first_row < rows.end);
        start..end.max(start)
    }

    /// Footer bytes: shape, nnz, block entries and a CRC-32 over all of them.
    pub(crate) fn footer_bytes(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(FOOTER_FIXED_LEN + ENTRY_LEN * self.blocks.len() + FOOTER_CRC_LEN);
        out.extend_from_slice(&self.shape.rows.to_le_bytes());
        out.extend_from_slice(&self.shape.cols.to_le_bytes());
        out.extend_from_slice(&self.nnz.to_le_bytes());
        out.extend_from_slice(&(self.blocks.len() as u64).to_le_bytes());
        for b in &self.blocks {
            out.extend_from_slice(&b.offset.to_le_bytes());
            out.extend_from_slice(&b.first_row.to_le_bytes());
            out.extend_from_slice(&b.nnz.to_le_bytes());
            out.extend_from_slice(&b.len.to_le_bytes());
            out.extend_from_slice(&b.row_count.to_le_bytes());
            out.extend_from_slice(&b.crc.to_le_bytes());
        }
        let crc = crc32(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Read header, trailer and footer of a file. No block is read.
    pub fn read_from<R: Read + Seek>(src: &mut R) -> Result<Index> {
        let file_len = src.seek(SeekFrom::End(0))?;
        let min_len = (HEADER_LEN + FOOTER_FIXED_LEN + FOOTER_CRC_LEN + TRAILER_LEN) as u64;
        if file_len < min_len {
            return Err(Error::Truncated(format!(
                "{file_len} bytes, the smallest valid file has {min_len}"
            )));
        }

        let mut header = [0u8; HEADER_LEN];
        read_at(src, 0, &mut header)?;
        let block_rows = parse_header(&header)?;

        let mut trailer = [0u8; TRAILER_LEN];
        let footer_end = file_len - TRAILER_LEN as u64;
        read_at(src, footer_end, &mut trailer)?;
        let mut t = LeReader::new(&trailer, "trailer");
        let footer_offset = t.u64()?;
        if t.array::<8>()? != TRAILER_MAGIC {
            return Err(Error::Truncated(
                "end marker missing (the file was cut off or is not a sparsepack file)".to_string(),
            ));
        }
        let smallest_footer = (FOOTER_FIXED_LEN + FOOTER_CRC_LEN) as u64;
        if footer_offset < HEADER_LEN as u64 || footer_offset > footer_end - smallest_footer {
            return Err(Error::Corrupt(format!(
                "footer offset {footer_offset} is outside the file ({file_len} bytes)"
            )));
        }
        // At most the file length, so the allocation is bounded by real data.
        let mut footer = vec![0u8; (footer_end - footer_offset) as usize];
        read_at(src, footer_offset, &mut footer)?;
        let index = Self::parse_footer(&footer, block_rows)?;
        index.validate(footer_offset)?;
        Ok(index)
    }

    /// Check that the index describes a possible file. The footer checksum
    /// already passed, so a failure here means the writer was faulty or the
    /// file was built by hand; either way, nothing below may trust it.
    ///
    /// Blocks must cover rows `0..rows` in order, `block_rows` rows each
    /// (the last one may have fewer), lie back to back from the end of the
    /// header to the start of the footer, and add up to the total nnz. This
    /// also bounds every block length by the file size, so reading a block
    /// never allocates more than the file holds.
    fn validate(&self, footer_offset: u64) -> Result<()> {
        let block_rows = u64::from(self.block_rows);
        let expected_blocks = self.shape.rows.div_ceil(block_rows);
        if self.blocks.len() as u64 != expected_blocks {
            return Err(Error::Corrupt(format!(
                "{} rows in blocks of {block_rows} need {expected_blocks} blocks, the index lists {}",
                self.shape.rows,
                self.blocks.len()
            )));
        }
        let mut offset = HEADER_LEN as u64;
        let mut nnz = 0u64;
        for (i, b) in self.blocks.iter().enumerate() {
            let first_row = i as u64 * block_rows;
            let row_count = block_rows.min(self.shape.rows - first_row);
            if b.first_row != first_row || u64::from(b.row_count) != row_count {
                return Err(Error::Corrupt(format!(
                    "block {i} should hold {row_count} rows from row {first_row}, \
                     the index says {} rows from row {}",
                    b.row_count, b.first_row
                )));
            }
            if b.offset != offset {
                return Err(Error::Corrupt(format!(
                    "block {i} starts at byte {}, expected {offset}",
                    b.offset
                )));
            }
            let overflow = || Error::Corrupt(format!("block {i}: sizes overflow"));
            offset = offset.checked_add(u64::from(b.len)).ok_or_else(overflow)?;
            nnz = nnz.checked_add(b.nnz).ok_or_else(overflow)?;
        }
        if offset != footer_offset {
            return Err(Error::Corrupt(format!(
                "blocks end at byte {offset}, but the footer starts at byte {footer_offset}"
            )));
        }
        if nnz != self.nnz {
            return Err(Error::Corrupt(format!(
                "blocks hold {nnz} non-zeros, the footer says {}",
                self.nnz
            )));
        }
        Ok(())
    }

    /// Check the footer checksum and decode the footer.
    fn parse_footer(footer: &[u8], block_rows: u32) -> Result<Index> {
        let (body, crc_bytes) = footer.split_at(footer.len() - FOOTER_CRC_LEN);
        let stored = LeReader::new(crc_bytes, "footer checksum").u32()?;
        let computed = crc32(body);
        if stored != computed {
            return Err(Error::ChecksumMismatch {
                part: "footer".to_string(),
                stored,
                computed,
            });
        }
        let mut r = LeReader::new(body, "footer");
        let shape = Shape {
            rows: r.u64()?,
            cols: r.u64()?,
        };
        let nnz = r.u64()?;
        let count = r.u64()?;
        let expected = count
            .checked_mul(ENTRY_LEN as u64)
            .and_then(|n| n.checked_add(FOOTER_FIXED_LEN as u64));
        if expected != Some(body.len() as u64) {
            return Err(Error::Corrupt(format!(
                "footer has {} bytes, which does not fit {count} block entries",
                body.len()
            )));
        }
        let mut blocks = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let offset = r.u64()?;
            let first_row = r.u64()?;
            let nnz = r.u64()?;
            let len = r.u32()?;
            let row_count = r.u32()?;
            let crc = r.u32()?;
            blocks.push(BlockEntry {
                offset,
                len,
                first_row,
                row_count,
                nnz,
                crc,
            });
        }
        Ok(Index::new(block_rows, shape, nnz, blocks))
    }

    /// Read block `i` from `src`, check its checksum and decode it.
    pub fn read_block<R: Read + Seek>(&self, src: &mut R, i: usize) -> Result<Block> {
        let entry = self.blocks.get(i).ok_or_else(|| {
            Error::InvalidInput(format!(
                "block {i} does not exist (the file has {} blocks)",
                self.blocks.len()
            ))
        })?;
        let mut bytes = vec![0u8; entry.len as usize];
        read_at(src, entry.offset, &mut bytes)?;
        let computed = crc32(&bytes);
        if computed != entry.crc {
            return Err(Error::ChecksumMismatch {
                part: format!("block {i}"),
                stored: entry.crc,
                computed,
            });
        }
        Block::decode(
            &bytes,
            entry.first_row,
            entry.row_count,
            self.shape.cols,
            entry.nnz,
        )
        .map_err(|msg| Error::Corrupt(format!("block {i}: {msg}")))
    }
}
