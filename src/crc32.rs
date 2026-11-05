//! CRC-32 checksums (the IEEE 802.3 variant used by zip, gzip and PNG).
//!
//! A CRC is the remainder of a polynomial division of the data by a fixed
//! 33-bit polynomial. It detects every error burst of up to 32 bits, so any
//! single changed byte in a checked region is always detected.
//!
//! This is the "reflected" form: bits are processed least significant first,
//! the register starts at all ones and the result is inverted. A 256-entry
//! table holds the effect of one input byte, so the main loop costs one table
//! lookup per byte. The table is computed at compile time by a `const fn`.

/// The polynomial 0x04C11DB7 with its bits reversed (reflected form).
const POLY: u32 = 0xEDB8_8320;

/// `TABLE[b]` is the CRC register after shifting byte `b` through 8 rounds.
const TABLE: [u32; 256] = make_table();

const fn make_table() -> [u32; 256] {
    // `for` loops are not allowed in a `const fn`, hence the `while` loops.
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        let index = (crc ^ u32::from(byte)) & 0xff;
        crc = TABLE[index as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;

    /// Bit-by-bit CRC without the table, as an independent reference.
    fn crc32_bitwise(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (POLY & mask);
            }
        }
        !crc
    }

    #[test]
    fn known_check_value() {
        // The standard check value of every CRC-32/ISO-HDLC implementation.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn known_values() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn table_matches_bitwise_reference() {
        let mut rng = SplitMix64::new(1);
        for len in [0usize, 1, 2, 3, 7, 8, 9, 100, 1000, 4097] {
            let data: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
            assert_eq!(crc32(&data), crc32_bitwise(&data), "length {len}");
        }
    }

    #[test]
    fn detects_every_single_bit_flip() {
        let mut rng = SplitMix64::new(2);
        let mut data: Vec<u8> = (0..256).map(|_| rng.next_u64() as u8).collect();
        let original = crc32(&data);
        for i in 0..data.len() {
            for bit in 0..8 {
                data[i] ^= 1 << bit;
                assert_ne!(crc32(&data), original, "byte {i} bit {bit}");
                data[i] ^= 1 << bit;
            }
        }
    }
}
