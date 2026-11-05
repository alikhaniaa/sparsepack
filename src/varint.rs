//! Unsigned LEB128 variable-length integers ("varints").
//!
//! A value is cut into groups of 7 bits, least significant group first. Each
//! group is stored in the low 7 bits of one byte; the high bit of the byte is
//! 1 when more bytes follow and 0 on the last byte. Values 0..=127 take one
//! byte, values below 2^14 take two, and a full `u64` takes at most
//! [`MAX_LEN`] = 10 bytes.
//!
//! Example: 300 = 0b10_0101100 is stored as `[0xAC, 0x02]`
//! (0x2C with the continuation bit set, then 0x02).
//!
//! The decoder is strict. It accepts only the shortest encoding of a value,
//! so every value has exactly one valid byte string, and it never reads past
//! the end of its input.

use std::fmt;

/// Largest number of bytes one `u64` needs: ceil(64 / 7) = 10.
pub const MAX_LEN: usize = 10;

/// Why a byte string is not a valid varint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarintError {
    /// The input ended before a byte with the high bit 0 was seen.
    UnexpectedEnd,
    /// The value does not fit in 64 bits (more than 10 bytes, or a 10th byte
    /// with bits above bit 63).
    Overflow,
    /// The encoding is longer than necessary: it ends with a zero group,
    /// as in `[0x80, 0x00]` for 0.
    NonCanonical,
}

impl fmt::Display for VarintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            VarintError::UnexpectedEnd => "varint is cut off by the end of the data",
            VarintError::Overflow => "varint does not fit in 64 bits",
            VarintError::NonCanonical => "varint is longer than necessary",
        };
        f.write_str(text)
    }
}

impl std::error::Error for VarintError {}

/// Append the varint encoding of `value` to `out`.
pub fn encode(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        // Low 7 bits plus the continuation bit. `as u8` keeps the low 8 bits.
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Decode one varint from the start of `input`.
///
/// Returns the value and the number of bytes it used. Bytes after the varint
/// are ignored.
pub fn decode(input: &[u8]) -> Result<(u64, usize), VarintError> {
    let mut value = 0u64;
    for (i, &byte) in input.iter().enumerate().take(MAX_LEN) {
        // The 10th byte holds bit 63 only, so it must be 0 or 1 (and, being
        // the last possible byte, it must not have the continuation bit).
        if i == MAX_LEN - 1 && byte > 1 {
            return Err(VarintError::Overflow);
        }
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            // A final zero group after other groups adds nothing: the
            // shorter encoding without it is the canonical one.
            if byte == 0 && i > 0 {
                return Err(VarintError::NonCanonical);
            }
            return Ok((value, i + 1));
        }
    }
    // Fewer than 10 bytes, and all of them had the continuation bit.
    Err(VarintError::UnexpectedEnd)
}

/// Reads consecutive varints from a byte slice.
///
/// The decoder borrows the slice (lifetime `'a`) instead of copying it, so it
/// cannot outlive the buffer it reads from.
#[derive(Debug, Clone)]
pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    /// Start decoding at the beginning of `input`.
    pub fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    /// Decode the next varint and move past it.
    pub fn read(&mut self) -> Result<u64, VarintError> {
        let (value, used) = decode(&self.input[self.pos..])?;
        self.pos += used;
        Ok(value)
    }

    /// Number of bytes consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Number of bytes not consumed yet.
    pub fn remaining(&self) -> usize {
        self.input.len() - self.pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;

    fn encoded(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        encode(value, &mut out);
        out
    }

    fn round_trip(value: u64) {
        let bytes = encoded(value);
        assert!(bytes.len() <= MAX_LEN);
        assert_eq!(decode(&bytes), Ok((value, bytes.len())), "value {value}");
    }

    #[test]
    fn known_encodings() {
        assert_eq!(encoded(0), [0x00]);
        assert_eq!(encoded(1), [0x01]);
        assert_eq!(encoded(127), [0x7f]);
        assert_eq!(encoded(128), [0x80, 0x01]);
        assert_eq!(encoded(300), [0xac, 0x02]);
        assert_eq!(encoded(16_383), [0xff, 0x7f]);
        assert_eq!(encoded(16_384), [0x80, 0x80, 0x01]);
        let mut max = vec![0xff; 9];
        max.push(0x01);
        assert_eq!(encoded(u64::MAX), max);
    }

    #[test]
    fn round_trips_edge_values() {
        for shift in 0..64 {
            let p = 1u64 << shift;
            round_trip(p - 1);
            round_trip(p);
            round_trip(p + 1);
        }
        for value in [u64::from(u32::MAX), u64::MAX - 1, u64::MAX] {
            round_trip(value);
        }
    }

    #[test]
    fn length_grows_every_seven_bits() {
        assert_eq!(encoded(0).len(), 1);
        for bytes in 2..=MAX_LEN {
            let smallest = 1u64 << (7 * (bytes - 1));
            assert_eq!(encoded(smallest).len(), bytes);
            assert_eq!(encoded(smallest - 1).len(), bytes - 1);
        }
    }

    #[test]
    fn round_trips_random_values() {
        let mut rng = SplitMix64::new(42);
        for _ in 0..100_000 {
            // Shift by a random amount so that all lengths are covered.
            let value = rng.next_u64() >> rng.below(64);
            round_trip(value);
        }
    }

    #[test]
    fn decoder_reads_a_sequence() {
        let values = [0u64, 5, 300, u64::MAX, 127, 128];
        let mut bytes = Vec::new();
        for &v in &values {
            encode(v, &mut bytes);
        }
        let mut dec = Decoder::new(&bytes);
        for &v in &values {
            assert_eq!(dec.read(), Ok(v));
        }
        assert_eq!(dec.remaining(), 0);
        assert_eq!(dec.position(), bytes.len());
        assert_eq!(dec.read(), Err(VarintError::UnexpectedEnd));
    }

    #[test]
    fn ignores_bytes_after_the_varint() {
        assert_eq!(decode(&[0x05, 0xff, 0xff]), Ok((5, 1)));
        assert_eq!(decode(&[0xac, 0x02, 0x80]), Ok((300, 2)));
    }

    #[test]
    fn rejects_cut_off_input() {
        assert_eq!(decode(&[]), Err(VarintError::UnexpectedEnd));
        assert_eq!(decode(&[0x80]), Err(VarintError::UnexpectedEnd));
        assert_eq!(decode(&[0xff; 9]), Err(VarintError::UnexpectedEnd));
        // Every proper prefix of a multi-byte encoding is cut off.
        let bytes = encoded(u64::MAX);
        for len in 0..bytes.len() {
            assert_eq!(decode(&bytes[..len]), Err(VarintError::UnexpectedEnd));
        }
    }

    #[test]
    fn rejects_values_above_64_bits() {
        // Ten continuation bytes: an 11th byte would be needed.
        assert_eq!(decode(&[0xff; 10]), Err(VarintError::Overflow));
        assert_eq!(decode(&[0xff; 12]), Err(VarintError::Overflow));
        // A 10th byte of 2 would set bit 64.
        let mut too_big = vec![0xff; 9];
        too_big.push(0x02);
        assert_eq!(decode(&too_big), Err(VarintError::Overflow));
    }

    #[test]
    fn rejects_overlong_encodings() {
        assert_eq!(decode(&[0x80, 0x00]), Err(VarintError::NonCanonical));
        assert_eq!(decode(&[0x81, 0x80, 0x00]), Err(VarintError::NonCanonical));
        // 1 padded to ten bytes.
        let mut padded = vec![0x81];
        padded.extend([0x80; 8]);
        padded.push(0x00);
        assert_eq!(decode(&padded), Err(VarintError::NonCanonical));
    }
}
