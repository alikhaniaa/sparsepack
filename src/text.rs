//! Text input and output: TSV triplets.
//!
//! A TSV triplet file has one non-zero per line, `row<TAB>col<TAB>value`,
//! with 0-based indices. Fields may also be separated by spaces. Lines that
//! start with `#` are comments, and empty lines are skipped. A comment of the
//! form `# shape ROWS COLS` before the first entry declares the shape of the
//! matrix; without it, the shape is the smallest one that holds all entries.
//!
//! `unpack` writes exactly this form: the shape line, then one line per
//! non-zero in row-major order. Packing such a file and unpacking it again
//! gives the same bytes.

use std::io::BufRead;

use crate::error::{Error, Result};
use crate::format::Shape;

/// One non-zero: `(row, col, value)`.
pub type Triplet = (u64, u64, u32);

/// The `# shape ROWS COLS` line (with newline) for `shape`.
pub fn shape_line(shape: Shape) -> String {
    format!("# shape {} {}\n", shape.rows, shape.cols)
}

/// Append the decimal digits of `value` to `out`.
///
/// Faster than `write!` because it skips the formatting machinery; output
/// speed matters when unpacking hundreds of millions of entries.
pub fn push_u64(out: &mut Vec<u8>, mut value: u64) {
    let mut digits = [0u8; 20]; // u64::MAX has 20 digits
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[i..]);
}

/// Append `row<TAB>col<TAB>value<NEWLINE>` to `out`.
pub fn push_triplet(out: &mut Vec<u8>, row: u64, col: u64, value: u32) {
    push_u64(out, row);
    out.push(b'\t');
    push_u64(out, col);
    out.push(b'\t');
    push_u64(out, u64::from(value));
    out.push(b'\n');
}

/// Parse a non-negative decimal integer made of ASCII digits only.
fn parse_u64(field: &[u8]) -> Option<u64> {
    if field.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for &b in field {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
    }
    Some(value)
}

/// The whitespace-separated fields of a line.
fn fields(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|b| b.is_ascii_whitespace())
        .filter(|f| !f.is_empty())
}

/// Parse exactly `N` integer fields, or return `None`.
fn parse_numbers<const N: usize>(line: &[u8]) -> Option<[u64; N]> {
    let mut out = [0u64; N];
    let mut it = fields(line);
    for slot in &mut out {
        *slot = parse_u64(it.next()?)?;
    }
    match it.next() {
        Some(_) => None,
        None => Some(out),
    }
}

/// Reads non-zeros from TSV triplet text, one line at a time.
///
/// It is an iterator over `Result<(row, col, value)>`. Each entry is checked
/// on its own (three integers, value in `1..=u32::MAX`, inside the declared
/// shape); the sort order is checked by the [`Writer`](crate::Writer).
pub struct TripletReader<R> {
    input: R,
    line: Vec<u8>,
    line_number: u64,
    shape: Option<Shape>,
    seen_entry: bool,
}

impl<R: BufRead> TripletReader<R> {
    /// Read triplets from `input`.
    pub fn new(input: R) -> Self {
        TripletReader {
            input,
            line: Vec::new(),
            line_number: 0,
            shape: None,
            seen_entry: false,
        }
    }

    /// The shape declared by a `# shape` line, if one was seen so far.
    pub fn shape(&self) -> Option<Shape> {
        self.shape
    }

    /// Number of the line read last (1-based), for error messages.
    pub fn line_number(&self) -> u64 {
        self.line_number
    }

    fn error(&self, what: impl std::fmt::Display) -> Error {
        Error::InvalidInput(format!("line {}: {what}", self.line_number))
    }

    /// Handle a comment line; only `# shape` comments mean anything.
    fn comment(&mut self) -> Result<()> {
        // The caller saw `#` as the first non-blank byte.
        let rest = &self.line.trim_ascii()[1..];
        let mut words = fields(rest);
        if words.next() != Some(b"shape".as_slice()) {
            return Ok(());
        }
        let numbers: Vec<&[u8]> = words.collect();
        let shape = match numbers.as_slice() {
            [rows, cols] => parse_u64(rows).zip(parse_u64(cols)),
            _ => None,
        };
        let Some((rows, cols)) = shape else {
            return Err(self.error("expected `# shape ROWS COLS`"));
        };
        if self.seen_entry || self.shape.is_some() {
            return Err(self.error("`# shape` must come once, before the first entry"));
        }
        self.shape = Some(Shape { rows, cols });
        Ok(())
    }

    /// Parse the current line as an entry.
    fn entry(&mut self) -> Result<Triplet> {
        let Some([row, col, value]) = parse_numbers::<3>(&self.line) else {
            return Err(self.error("expected `row<TAB>col<TAB>value` with non-negative integers"));
        };
        let value = match u32::try_from(value) {
            Ok(v) if v > 0 => v,
            _ => return Err(self.error(format!("value {value} is not in 1..=4294967295"))),
        };
        if let Some(shape) = self.shape {
            if row >= shape.rows || col >= shape.cols {
                return Err(self.error(format!(
                    "entry ({row}, {col}) is outside the declared {} x {} shape",
                    shape.rows, shape.cols
                )));
            }
        }
        self.seen_entry = true;
        Ok((row, col, value))
    }
}

impl<R: BufRead> Iterator for TripletReader<R> {
    type Item = Result<Triplet>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.line.clear();
            match self.input.read_until(b'\n', &mut self.line) {
                Ok(0) => return None,
                Ok(_) => {}
                Err(err) => return Some(Err(err.into())),
            }
            self.line_number += 1;
            let trimmed = self.line.trim_ascii();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed[0] == b'#' {
                if let Err(err) = self.comment() {
                    return Some(Err(err));
                }
                continue;
            }
            return Some(self.entry());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(text: &str) -> (Result<Vec<Triplet>>, Option<Shape>) {
        let mut reader = TripletReader::new(text.as_bytes());
        let entries = reader.by_ref().collect();
        (entries, reader.shape())
    }

    #[test]
    fn push_u64_writes_decimal() {
        for value in [0u64, 7, 10, 99, 12345, u64::MAX] {
            let mut out = Vec::new();
            push_u64(&mut out, value);
            assert_eq!(out, value.to_string().into_bytes());
        }
    }

    #[test]
    fn parses_entries_comments_and_shape() {
        let text = "# made by hand\n# shape 4 10\n0\t1\t5\n\n2 3  7\r\n# note\n3\t9\t4294967295\n";
        let (entries, shape) = read_all(text);
        assert_eq!(entries.unwrap(), [(0, 1, 5), (2, 3, 7), (3, 9, u32::MAX)]);
        assert_eq!(shape, Some(Shape { rows: 4, cols: 10 }));
    }

    #[test]
    fn rejects_bad_lines() {
        for text in [
            "0\t1\n",
            "0\t1\t2\t3\n",
            "0\t-1\t2\n",
            "0\tx\t2\n",
            "0\t1\t0\n",
            "0\t1\t4294967296\n",
            "0\t1\t99999999999999999999999\n",
            "# shape 2\n",
            "# shape 2 2\n5\t0\t1\n",
            "# shape 2 2\n0\t2\t1\n",
            "0\t0\t1\n# shape 2 2\n",
            "# shape 2 2\n# shape 2 2\n",
        ] {
            let (entries, _) = read_all(text);
            assert!(matches!(entries, Err(Error::InvalidInput(_))), "{text:?}");
        }
    }

    #[test]
    fn error_names_the_line() {
        let (entries, _) = read_all("0\t0\t1\n\n0\t1\tx\n");
        let message = entries.unwrap_err().to_string();
        assert!(message.contains("line 3"), "{message}");
    }
}
