//! Per-column tables whose size comes from the file.
//!
//! The number of columns is read from the footer, so a valid file of 72
//! bytes can declare 2^30 columns. Allocating a table for it with
//! `vec![0; n]` would abort the process when the memory is not available.
//! Here the length is checked against a fixed limit first, and the memory
//! is reserved with `try_reserve_exact`, so a request that is too large
//! becomes an error message instead.

use crate::error::{Error, Result};

/// Largest number of entries in the column tables of one operation, all
/// tables together: 2^27 entries, which is 1 GiB of `u64` counters.
pub const MAX_TABLE_ENTRIES: usize = 1 << 27;

/// Table length for `cols` columns, or an error if it is above
/// [`MAX_TABLE_ENTRIES`] (or does not fit in `usize` on this platform).
pub fn table_len(cols: u64, what: &str) -> Result<usize> {
    usize::try_from(cols)
        .ok()
        .filter(|&n| n <= MAX_TABLE_ENTRIES)
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "{what} keeps one entry per column in memory; {cols} columns is above \
                 the limit of {MAX_TABLE_ENTRIES}"
            ))
        })
}

/// A vector of `len` copies of `value`, or an error if the memory cannot be
/// reserved.
pub fn filled<T: Clone>(len: usize, value: T) -> Result<Vec<T>> {
    let mut table = Vec::new();
    table
        .try_reserve_exact(len)
        .map_err(|_| Error::InvalidInput(format!("cannot allocate a table of {len} entries")))?;
    table.resize(len, value);
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_limit() {
        assert_eq!(table_len(0, "test").unwrap(), 0);
        assert_eq!(table_len(1 << 27, "test").unwrap(), 1 << 27);
        for cols in [(1u64 << 27) + 1, 1 << 30, 1 << 32, u64::MAX] {
            assert!(matches!(
                table_len(cols, "test"),
                Err(Error::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn filled_or_error() {
        assert_eq!(filled(3, 7u64).unwrap(), [7, 7, 7]);
        assert!(filled(usize::MAX / 8, 0u64).is_err());
    }
}
