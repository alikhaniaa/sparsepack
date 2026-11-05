//! Text input and output: TSV triplets and MatrixMarket files.
//!
//! **TSV triplets.** One non-zero per line, `row<TAB>col<TAB>value`, with
//! 0-based indices. Fields may also be separated by spaces. Lines that start
//! with `#` are comments, and empty lines are skipped. A comment of the form
//! `# shape ROWS COLS` before the first entry declares the shape of the
//! matrix; without it, the shape is the smallest one that holds all entries.
//!
//! `unpack` writes exactly this form: the shape line, then one line per
//! non-zero in row-major order. Packing such a file and unpacking it again
//! gives the same bytes.
//!
//! **MatrixMarket** (input only). The banner
//! `%%MatrixMarket matrix coordinate integer general`, optional `%` comment
//! lines, a size line `ROWS COLS ENTRIES`, then `row col value` lines with
//! 1-based indices. The number of entry lines must match the size line.
//! Real, complex, pattern and symmetric files are rejected.

use std::io::BufRead;

use crate::error::{Error, Result};
use crate::format::Shape;

/// One non-zero: `(row, col, value)`.
pub type Triplet = (u64, u64, u32);

/// Which text format an input uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    /// TSV triplets with 0-based indices.
    Tsv,
    /// MatrixMarket coordinate integer general, 1-based indices.
    MatrixMarket,
}

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

/// Reads non-zeros from TSV or MatrixMarket text, one line at a time.
///
/// It is an iterator over `Result<(row, col, value)>` with 0-based indices.
/// Each entry is checked on its own (three integers, value in
/// `1..=u32::MAX`, inside the declared shape); the sort order is checked by
/// the [`Writer`](crate::Writer). After the first error the iterator ends.
pub struct TripletReader<R> {
    input: R,
    format: InputFormat,
    line: Vec<u8>,
    line_number: u64,
    shape: Option<Shape>,
    /// Entries read so far.
    entries: u64,
    /// Entry count from a MatrixMarket size line.
    declared_entries: Option<u64>,
    done: bool,
}

impl<R: BufRead> TripletReader<R> {
    /// Read triplets in `format` from `input`.
    pub fn new(input: R, format: InputFormat) -> Self {
        TripletReader {
            input,
            format,
            line: Vec::new(),
            line_number: 0,
            shape: None,
            entries: 0,
            declared_entries: None,
            done: false,
        }
    }

    /// The shape declared by the input (a `# shape` line or a MatrixMarket
    /// size line), if one was seen so far.
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

    /// Handle the current line; returns the entry if the line holds one.
    fn handle_line(&mut self) -> Result<Option<Triplet>> {
        let line = self.line.trim_ascii();
        match self.format {
            InputFormat::Tsv => {
                if line.is_empty() {
                    Ok(None)
                } else if line[0] == b'#' {
                    self.shape_comment().map(|()| None)
                } else {
                    self.entry(0).map(Some)
                }
            }
            InputFormat::MatrixMarket => {
                if self.line_number == 1 {
                    self.banner().map(|()| None)
                } else if line.is_empty() || line[0] == b'%' {
                    Ok(None)
                } else if self.shape.is_none() {
                    self.size_line().map(|()| None)
                } else {
                    self.entry(1).map(Some)
                }
            }
        }
    }

    /// Handle a TSV comment line; only `# shape` comments mean anything.
    fn shape_comment(&mut self) -> Result<()> {
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
        if self.entries > 0 || self.shape.is_some() {
            return Err(self.error("`# shape` must come once, before the first entry"));
        }
        self.shape = Some(Shape { rows, cols });
        Ok(())
    }

    /// Check the MatrixMarket banner line.
    fn banner(&self) -> Result<()> {
        let words: Vec<String> = fields(&self.line)
            .map(|w| String::from_utf8_lossy(w).to_ascii_lowercase())
            .collect();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["%%matrixmarket", "matrix", "coordinate", "integer", "general"] => Ok(()),
            ["%%matrixmarket", "matrix", "coordinate", "integer", other] => Err(self.error(
                format!("only `general` matrices are supported, not `{other}`"),
            )),
            ["%%matrixmarket", "matrix", "coordinate", other, _] => Err(self.error(format!(
                "only `integer` values are supported, not `{other}`"
            ))),
            ["%%matrixmarket", "matrix", other, ..] => Err(self.error(format!(
                "only the `coordinate` (sparse) layout is supported, not `{other}`"
            ))),
            _ => Err(self.error("expected `%%MatrixMarket matrix coordinate integer general`")),
        }
    }

    /// Parse the MatrixMarket size line `ROWS COLS ENTRIES`.
    fn size_line(&mut self) -> Result<()> {
        let Some([rows, cols, entries]) = parse_numbers::<3>(&self.line) else {
            return Err(self.error("expected the size line `ROWS COLS ENTRIES`"));
        };
        self.shape = Some(Shape { rows, cols });
        self.declared_entries = Some(entries);
        Ok(())
    }

    /// Parse the current line as an entry whose indices start at `base`
    /// (0 for TSV, 1 for MatrixMarket).
    fn entry(&mut self, base: u64) -> Result<Triplet> {
        let Some([row, col, value]) = parse_numbers::<3>(&self.line) else {
            return Err(self.error("expected `row col value` with non-negative integers"));
        };
        let (Some(row), Some(col)) = (row.checked_sub(base), col.checked_sub(base)) else {
            return Err(self.error("indices start at 1 in MatrixMarket files"));
        };
        let value = match u32::try_from(value) {
            Ok(v) if v > 0 => v,
            _ => return Err(self.error(format!("value {value} is not in 1..=4294967295"))),
        };
        if let Some(shape) = self.shape {
            if row >= shape.rows || col >= shape.cols {
                return Err(self.error(format!(
                    "entry ({row}, {col}) (0-based) is outside the declared {} x {} shape",
                    shape.rows, shape.cols
                )));
            }
        }
        self.entries += 1;
        if let Some(declared) = self.declared_entries {
            if self.entries > declared {
                return Err(
                    self.error(format!("more entries than the {declared} on the size line"))
                );
            }
        }
        Ok((row, col, value))
    }

    /// Checks at the end of the input.
    fn at_end(&self) -> Result<()> {
        if self.format == InputFormat::MatrixMarket {
            match self.declared_entries {
                None => {
                    return Err(Error::InvalidInput(
                        "MatrixMarket input ends before its size line".to_string(),
                    ))
                }
                Some(declared) if declared != self.entries => {
                    return Err(Error::InvalidInput(format!(
                        "the size line declares {declared} entries, the file has {}",
                        self.entries
                    )))
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

impl<R: BufRead> Iterator for TripletReader<R> {
    type Item = Result<Triplet>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            self.line.clear();
            match self.input.read_until(b'\n', &mut self.line) {
                Ok(0) => {
                    self.done = true;
                    return self.at_end().err().map(Err);
                }
                Ok(_) => self.line_number += 1,
                Err(err) => {
                    self.done = true;
                    return Some(Err(err.into()));
                }
            }
            match self.handle_line() {
                Ok(Some(entry)) => return Some(Ok(entry)),
                Ok(None) => {}
                Err(err) => {
                    self.done = true;
                    return Some(Err(err));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(text: &str, format: InputFormat) -> (Result<Vec<Triplet>>, Option<Shape>) {
        let mut reader = TripletReader::new(text.as_bytes(), format);
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
    fn parses_tsv_entries_comments_and_shape() {
        let text = "# made by hand\n# shape 4 10\n0\t1\t5\n\n2 3  7\r\n# note\n3\t9\t4294967295\n";
        let (entries, shape) = read_all(text, InputFormat::Tsv);
        assert_eq!(entries.unwrap(), [(0, 1, 5), (2, 3, 7), (3, 9, u32::MAX)]);
        assert_eq!(shape, Some(Shape { rows: 4, cols: 10 }));
    }

    #[test]
    fn rejects_bad_tsv_lines() {
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
            let (entries, _) = read_all(text, InputFormat::Tsv);
            assert!(matches!(entries, Err(Error::InvalidInput(_))), "{text:?}");
        }
    }

    #[test]
    fn error_names_the_line() {
        let (entries, _) = read_all("0\t0\t1\n\n0\t1\tx\n", InputFormat::Tsv);
        let message = entries.unwrap_err().to_string();
        assert!(message.contains("line 3"), "{message}");
    }

    #[test]
    fn parses_matrix_market() {
        let text =
            "%%MatrixMarket matrix coordinate integer general\n% comment\n%\n3 4 2\n1 1 5\n3 4 7\n";
        let (entries, shape) = read_all(text, InputFormat::MatrixMarket);
        assert_eq!(entries.unwrap(), [(0, 0, 5), (2, 3, 7)]);
        assert_eq!(shape, Some(Shape { rows: 3, cols: 4 }));
        // The banner is case-insensitive.
        let upper = "%%MATRIXMARKET MATRIX COORDINATE INTEGER GENERAL\n2 2 0\n";
        let (entries, _) = read_all(upper, InputFormat::MatrixMarket);
        assert!(entries.unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_matrix_market() {
        let banner = "%%MatrixMarket matrix coordinate integer general\n";
        let cases = [
            ("".to_string(), "size line"),
            ("1 1 1\n".to_string(), "expected `%%MatrixMarket"),
            (
                "%%MatrixMarket matrix coordinate real general\n1 1 0\n".to_string(),
                "integer",
            ),
            (
                "%%MatrixMarket matrix coordinate pattern general\n".to_string(),
                "integer",
            ),
            (
                "%%MatrixMarket matrix coordinate integer symmetric\n".to_string(),
                "general",
            ),
            (
                "%%MatrixMarket matrix array integer general\n".to_string(),
                "coordinate",
            ),
            (banner.to_string(), "size line"),
            (format!("{banner}2 2\n"), "size line"),
            (format!("{banner}2 2 1\n0 1 1\n"), "start at 1"),
            (format!("{banner}2 2 1\n3 1 1\n"), "outside"),
            (format!("{banner}2 2 2\n1 1 1\n"), "declares 2 entries"),
            (format!("{banner}2 2 1\n1 1 1\n2 2 1\n"), "more entries"),
            (format!("{banner}2 2 1\n1 1 -3\n"), "non-negative"),
        ];
        for (text, expected) in cases {
            let (entries, _) = read_all(&text, InputFormat::MatrixMarket);
            let message = entries.unwrap_err().to_string();
            assert!(message.contains(expected), "{text:?}: {message}");
        }
    }
}
