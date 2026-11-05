//! Damaged files must give an `Err`, never a panic and never wrong data.

mod common;

use common::{pack, random_matrix, unpack, Matrix};
use sparsepack::crc32::crc32;
use sparsepack::format::{ENTRY_LEN, FOOTER_FIXED_LEN, HEADER_LEN, TRAILER_LEN};
use sparsepack::rng::SplitMix64;
use sparsepack::Error;

/// A few small files with several blocks each.
fn sample_files() -> Vec<Vec<u8>> {
    let mut rng = SplitMix64::new(99);
    let mut files = vec![pack(&Matrix::new(0, 0, vec![]), 4)];
    for (rows, cols, block_rows) in [(10, 8, 4), (40, 300, 8), (7, 1, 1), (33, 5000, 16)] {
        files.push(pack(
            &random_matrix(&mut rng, rows, cols, 6, 0.2),
            block_rows,
        ));
    }
    files
}

/// Byte offset of the footer, read from the trailer.
fn footer_offset(bytes: &[u8]) -> usize {
    let at = bytes.len() - TRAILER_LEN;
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap()) as usize
}

/// Recompute the footer checksum after the footer was edited, so that only
/// the structural checks can catch the edit.
fn fix_footer_crc(bytes: &mut [u8]) {
    let start = footer_offset(bytes);
    let crc_at = bytes.len() - TRAILER_LEN - 4;
    let crc = crc32(&bytes[start..crc_at]);
    bytes[crc_at..crc_at + 4].copy_from_slice(&crc.to_le_bytes());
}

/// Overwrite a little-endian field inside the footer and fix the checksum.
fn patch_footer(bytes: &[u8], field: usize, value: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let at = footer_offset(&out) + field;
    out[at..at + value.len()].copy_from_slice(value);
    fix_footer_crc(&mut out);
    out
}

#[test]
fn every_flipped_byte_is_detected() {
    for bytes in sample_files() {
        for i in 0..bytes.len() {
            for mask in [0x01u8, 0x80, 0xff] {
                let mut bad = bytes.clone();
                bad[i] ^= mask;
                assert!(
                    unpack(&bad).is_err(),
                    "byte {i} of {} ^ {mask:#x}",
                    bytes.len()
                );
            }
        }
    }
}

#[test]
fn every_truncation_is_detected() {
    for bytes in sample_files() {
        for len in 0..bytes.len() {
            assert!(
                unpack(&bytes[..len]).is_err(),
                "cut at {len} of {}",
                bytes.len()
            );
        }
    }
}

#[test]
fn appended_bytes_are_detected() {
    for bytes in sample_files() {
        for extra in [&[0u8][..], &[1, 2, 3], &[0; 64]] {
            let mut longer = bytes.clone();
            longer.extend_from_slice(extra);
            assert!(unpack(&longer).is_err());
        }
    }
}

#[test]
fn errors_name_the_problem() {
    let mut rng = SplitMix64::new(1);
    let bytes = pack(&random_matrix(&mut rng, 20, 50, 5, 0.0), 8);

    let mut bad = bytes.clone();
    bad[0] = b'X';
    assert!(matches!(unpack(&bad), Err(Error::NotSparsepack)));

    let mut bad = bytes.clone();
    bad[HEADER_LEN] ^= 1; // first byte of block 0
    match unpack(&bad) {
        Err(Error::ChecksumMismatch { part, .. }) => assert_eq!(part, "block 0"),
        other => panic!("expected a block checksum error, got {other:?}"),
    }

    let mut bad = bytes.clone();
    bad[footer_offset(&bytes) + 1] ^= 1;
    match unpack(&bad) {
        Err(Error::ChecksumMismatch { part, .. }) => assert_eq!(part, "footer"),
        other => panic!("expected a footer checksum error, got {other:?}"),
    }

    assert!(matches!(
        unpack(&bytes[..bytes.len() - 1]),
        Err(Error::Truncated(_))
    ));
    assert!(matches!(unpack(&bytes[..10]), Err(Error::Truncated(_))));

    // Version 2 with a correct header checksum.
    let mut bad = bytes.clone();
    bad[8] = 2;
    let crc = crc32(&bad[..16]);
    bad[16..20].copy_from_slice(&crc.to_le_bytes());
    assert!(matches!(unpack(&bad), Err(Error::UnsupportedVersion(2))));
}

#[test]
fn inconsistent_index_with_valid_checksum_is_rejected() {
    let mut rng = SplitMix64::new(2);
    let bytes = pack(&random_matrix(&mut rng, 30, 40, 5, 0.0), 8);
    assert!(unpack(&bytes).is_ok());

    let entry = |i: usize, field: usize| FOOTER_FIXED_LEN + i * ENTRY_LEN + field;
    let cases: Vec<(&str, usize, Vec<u8>)> = vec![
        ("more rows", 0, 31u64.to_le_bytes().to_vec()),
        ("fewer rows", 0, 20u64.to_le_bytes().to_vec()),
        ("fewer columns", 8, 3u64.to_le_bytes().to_vec()),
        ("total nnz", 16, 1u64.to_le_bytes().to_vec()),
        ("huge block count", 24, u64::MAX.to_le_bytes().to_vec()),
        ("block offset", entry(1, 0), 21u64.to_le_bytes().to_vec()),
        ("first row", entry(1, 8), 9u64.to_le_bytes().to_vec()),
        (
            "huge first row",
            entry(3, 8),
            u64::MAX.to_le_bytes().to_vec(),
        ),
        ("block nnz", entry(2, 16), u64::MAX.to_le_bytes().to_vec()),
        (
            "block length",
            entry(0, 24),
            u32::MAX.to_le_bytes().to_vec(),
        ),
        ("row count", entry(3, 28), 7u32.to_le_bytes().to_vec()),
        ("block checksum", entry(0, 32), 0u32.to_le_bytes().to_vec()),
    ];
    for (name, field, value) in cases {
        let bad = patch_footer(&bytes, field, &value);
        assert!(unpack(&bad).is_err(), "{name}");
    }
}

#[test]
fn random_damage_with_fixed_checksums_never_panics() {
    // Change block bytes and then repair every checksum, so that the block
    // decoder sees garbage. It may decode something or fail, but it must
    // not panic or allocate without bound.
    let mut rng = SplitMix64::new(3);
    let m = random_matrix(&mut rng, 64, 1000, 20, 0.1);
    let bytes = pack(&m, 16);
    let footer = footer_offset(&bytes);
    for _ in 0..3000 {
        let mut bad = bytes.clone();
        for _ in 0..1 + rng.below(4) {
            let i = HEADER_LEN + rng.below((footer - HEADER_LEN) as u64) as usize;
            bad[i] = rng.next_u64() as u8;
        }
        // Recompute each block checksum in the footer.
        for b in 0..4 {
            let at = footer + FOOTER_FIXED_LEN + b * ENTRY_LEN;
            let offset = u64::from_le_bytes(bad[at..at + 8].try_into().unwrap()) as usize;
            let len = u32::from_le_bytes(bad[at + 24..at + 28].try_into().unwrap()) as usize;
            let crc = crc32(&bad[offset..offset + len]);
            bad[at + 32..at + 36].copy_from_slice(&crc.to_le_bytes());
        }
        fix_footer_crc(&mut bad);
        // Only the outcome type matters; decoding must simply return.
        let _ = unpack(&bad);
    }
}
