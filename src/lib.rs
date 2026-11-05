//! sparsepack: a compressed, block-indexed file format for sparse matrices
//! of small non-negative integers.
//!
//! A matrix is stored as blocks of consecutive rows. Each block is
//! compressed on its own (column gaps and values as varints) and has its own
//! CRC-32. A footer at the end of the file lists every block, so a reader can
//! go straight to the rows it needs.
//!
//! Writing: create a [`Writer`], [`Writer::push`] the non-zeros in row-major
//! order and call [`Writer::finish`]. Reading: [`Index::read_from`] reads the
//! footer, then [`Index::read_block`] decodes one [`Block`].

#![warn(missing_docs)]

pub mod block;
pub mod crc32;
pub mod error;
pub mod format;
pub mod gen;
pub mod ops;
pub mod reader;
pub mod rng;
pub mod table;
pub mod text;
pub mod varint;
pub mod writer;

pub use block::{Block, Row};
pub use error::{Error, Result};
pub use format::{BlockEntry, Index, Shape, DEFAULT_BLOCK_ROWS};
pub use reader::Reader;
pub use writer::Writer;
