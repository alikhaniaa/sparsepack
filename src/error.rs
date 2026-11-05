//! The error type of the library.
//!
//! Every function that reads untrusted bytes returns one of these errors
//! instead of panicking. The binary wraps them with `anyhow` to add context
//! such as file names.

use std::fmt;
use std::io;

/// Everything that can go wrong while writing or reading a sparsepack file.
#[derive(Debug)]
pub enum Error {
    /// Reading or writing failed at the operating system level.
    Io(io::Error),
    /// The file does not start with the sparsepack magic bytes.
    NotSparsepack,
    /// The file was written by a format version this code does not read.
    UnsupportedVersion(u16),
    /// The file is shorter than its own structure says it should be.
    Truncated(String),
    /// A stored checksum does not match the checksum of the stored bytes.
    ChecksumMismatch {
        /// Which part of the file: "header", "footer" or "block 12".
        part: String,
        /// The checksum stored in the file.
        stored: u32,
        /// The checksum computed from the bytes that were read.
        computed: u32,
    },
    /// The bytes are not a valid encoding (wrong counts, bad varints,
    /// indices out of range, inconsistent index entries).
    Corrupt(String),
    /// The caller passed data that cannot be stored (unsorted entries, zero
    /// values, indices outside the declared shape, bad options).
    InvalidInput(String),
}

/// Shorthand for results with the library [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "I/O error: {err}"),
            Error::NotSparsepack => f.write_str("not a sparsepack file (wrong magic bytes)"),
            Error::UnsupportedVersion(v) => write!(f, "unsupported format version {v}"),
            Error::Truncated(what) => write!(f, "truncated file: {what}"),
            Error::ChecksumMismatch {
                part,
                stored,
                computed,
            } => write!(
                f,
                "checksum mismatch in {part}: stored {stored:08x}, computed {computed:08x}"
            ),
            Error::Corrupt(what) => write!(f, "corrupt file: {what}"),
            Error::InvalidInput(what) => write!(f, "invalid input: {what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}
