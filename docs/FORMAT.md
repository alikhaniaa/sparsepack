# The sparsepack file format, version 1

A `.spk` file stores one sparse matrix of non-negative integers: `rows x
cols` positions, of which `nnz` are non-zero. Only the non-zeros are stored.

```
+--------+---------+---------+-----+-------------+--------+---------+
| header | block 0 | block 1 | ... | block n - 1 | footer | trailer |
+--------+---------+---------+-----+-------------+--------+---------+
  20 B     variable length, compressed             36+36n B   16 B
```

All fixed-size integers are little-endian. "varint" means the unsigned
LEB128 encoding described below. Offsets are counted from the start of the
file.

## Header (20 bytes, offset 0)

| offset | size | type   | field                                         |
|-------:|-----:|--------|-----------------------------------------------|
| 0      | 8    | bytes  | magic `SPARSEPK` (`53 50 41 52 53 45 50 4B`)  |
| 8      | 2    | u16    | format version, `1`                           |
| 10     | 2    | u16    | reserved, must be `0`                         |
| 12     | 4    | u32    | `block_rows`: rows per block, at least 1      |
| 16     | 4    | u32    | CRC-32 of bytes 0..16                         |

## Blocks

Rows are grouped into blocks of `block_rows` consecutive rows. Block `i`
holds rows `i * block_rows` up to (not including)
`min((i + 1) * block_rows, rows)`, so every block except the last has exactly
`block_rows` rows, and there are `ceil(rows / block_rows)` blocks. Empty rows
are stored (one byte each). Block 0 starts right after the header and the
blocks follow each other without gaps.

Each block is a sequence of its rows. Each row is:

| field            | encoding | meaning                                           |
|------------------|----------|---------------------------------------------------|
| `n`              | varint   | number of non-zeros in the row                    |
| `d_1 ... d_n`    | varints  | column gaps: `d_1 = c_1`, `d_k = c_k - c_(k-1) - 1` |
| `v_1 ... v_n`    | varints  | values, each in `1..=4294967295`                  |

Here `c_1 < c_2 < ... < c_n` are the column indices of the row. Because the
columns are strictly increasing, the gaps are never negative, and the `- 1`
makes neighbouring columns cost a gap of 0. The decoder rebuilds
`c_1 = d_1` and `c_k = c_(k-1) + 1 + d_k`.

A block has no header of its own: its row count, non-zero count, length and
checksum are in the footer.

## Footer (36 + 36 x block_count bytes)

| offset          | size | type | field                                      |
|----------------:|-----:|------|--------------------------------------------|
| 0               | 8    | u64  | `rows`                                     |
| 8               | 8    | u64  | `cols`                                     |
| 16              | 8    | u64  | `nnz`, total non-zeros                     |
| 24              | 8    | u64  | `block_count`                              |
| 32 + 36 i       | 36   |      | entry of block `i` (table below)           |
| 32 + 36 n       | 4    | u32  | CRC-32 of footer bytes 0 .. 32 + 36 n      |

Block entry (36 bytes):

| offset | size | type | field                                         |
|-------:|-----:|------|-----------------------------------------------|
| 0      | 8    | u64  | `offset` of the block in the file             |
| 8      | 8    | u64  | `first_row`, matrix index of the block's first row |
| 16     | 8    | u64  | `nnz` of the block                            |
| 24     | 4    | u32  | `len`, length of the block in bytes           |
| 28     | 4    | u32  | `row_count`, rows in the block                |
| 32     | 4    | u32  | CRC-32 of the block's `len` bytes             |

## Trailer (16 bytes, the last bytes of the file)

| offset | size | type  | field                                      |
|-------:|-----:|-------|--------------------------------------------|
| 0      | 8    | u64   | `footer_offset`                            |
| 8      | 8    | bytes | end marker `SPKTRAIL` (`53 50 4B 54 52 41 49 4C`) |

## Varints (unsigned LEB128)

A value is split into groups of 7 bits, least significant group first. Each
group goes into one byte; the high bit (`0x80`) is set on every byte except
the last. 0..127 take 1 byte, 128..16383 take 2, and `u64::MAX` takes 10.
Examples: `0` is `00`, `300` is `AC 02`, `u64::MAX` is `FF` x 9 then `01`.

Only the shortest encoding is valid: a last byte of `00` after other bytes
(for example `80 00` for 0) is rejected, and so is a 10th byte other than
`00`..`01`.

## CRC-32

CRC-32/ISO-HDLC, the checksum of zip, gzip and PNG: reflected polynomial
`0xEDB88320`, initial value `0xFFFFFFFF`, final XOR `0xFFFFFFFF`. The check
value of the ASCII bytes `123456789` is `0xCBF43926`.

## Reading

1. Read the header; check the magic, the header CRC, the version, the
   reserved field and `block_rows >= 1`.
2. Read the last 16 bytes; check the end marker. A missing marker means the
   file was cut off (or is not a sparsepack file).
3. Read the footer from `footer_offset` up to the trailer; check its CRC and
   that its length is exactly `36 + 36 x block_count`.
4. Check the index: `block_count = ceil(rows / block_rows)`; entry `i` has
   `first_row = i x block_rows` and the row count given above; the blocks
   lie back to back from byte 20 to `footer_offset`; the block `nnz` add up
   to the total `nnz`.
5. To read rows `a..b`, find the blocks that overlap them (binary search over
   `first_row`, or simply `a / block_rows`), read only those, check each
   block CRC, and decode. While decoding, check that every row count fits in
   the remaining bytes, every column is below `cols`, every value is in
   `1..=u32::MAX`, the block ends exactly after its last row and holds the
   `nnz` its entry says.

Every byte of a file is covered by a check: the header and footer by their
CRCs, every block by its CRC in the footer, and the trailer by the end
marker and the footer CRC. A CRC-32 detects every change of up to 32
consecutive bits, so a single damaged byte in the header, a block or the
footer is always reported. A damaged `footer_offset` makes the reader
checksum the wrong range of bytes; that range passing the CRC by chance has
a probability of about 2^-32, so this case is caught in practice but not
guaranteed. The tests flip every byte of several small files and check that
each change is reported.

## Worked example

The 3 x 4 matrix with non-zeros (0, 1) = 5, (0, 3) = 300 and (2, 2) = 1,
packed with `--block-rows 2`, is this 154-byte file (`od -A d -t x1`):

```
0000000 53 50 41 52 53 45 50 4b 01 00 00 00 02 00 00 00
0000016 9a a9 59 59 02 01 01 05 ac 02 00 01 02 01 03 00
0000032 00 00 00 00 00 00 04 00 00 00 00 00 00 00 03 00
0000048 00 00 00 00 00 00 02 00 00 00 00 00 00 00 14 00
0000064 00 00 00 00 00 00 00 00 00 00 00 00 00 00 02 00
0000080 00 00 00 00 00 00 07 00 00 00 02 00 00 00 f6 02
0000096 45 29 1b 00 00 00 00 00 00 00 02 00 00 00 00 00
0000112 00 00 01 00 00 00 00 00 00 00 03 00 00 00 01 00
0000128 00 00 31 e1 b2 bb 09 96 df 34 1e 00 00 00 00 00
0000144 00 00 53 50 4b 54 52 41 49 4c
```

| bytes   | meaning |
|---------|---------|
| 0..8    | magic `SPARSEPK` |
| 8..12   | version 1, reserved 0 |
| 12..16  | `block_rows` = 2 |
| 16..20  | header CRC |
| 20..27  | block 0 (rows 0 and 1): row 0 `02` non-zeros, gaps `01` (column 1) and `01` (3 - 1 - 1), values `05` and `ac 02` (300); row 1 `00` |
| 27..30  | block 1 (row 2): `01` non-zero, gap `02` (column 2), value `01` |
| 30..62  | footer: rows 3, cols 4, nnz 3, 2 blocks |
| 62..98  | entry 0: offset 20, first row 0, nnz 2, length 7, 2 rows, CRC |
| 98..134 | entry 1: offset 27, first row 2, nnz 1, length 3, 1 row, CRC |
| 134..138 | footer CRC |
| 138..154 | trailer: footer offset 30 (`1e`), `SPKTRAIL` |

Of the 154 bytes, 10 hold the matrix and 144 are header, index and trailer; for small
matrices the overhead dominates.

## Why the shape is in the footer

The writer streams: it writes each block as soon as the block is full and
keeps only the index in memory. When the input has no `# shape` line, the
number of rows and columns is only known after the last entry, and the
total `nnz` is never known earlier. Keeping shape, `nnz` and the index in the
footer means the writer never has to seek back, so it can write to a pipe as
well as to a file. The header holds only what is fixed before the first
entry (magic, version and block size).

## Limits

- Values are `u32` in `1..=4294967295`; zero is not stored (it is the
  implicit value), negative numbers and floats cannot be stored.
- Row and column indices are `u64` below `2^64 - 1`.
- A compressed block must fit in 4 GiB (`len` is a `u32`); a block's row
  count is at most `2^32 - 1`.
- The format allows any number of columns, but the tool's per-column
  operations (`colsums`, `filter`) keep one 8-byte counter per column in
  memory and refuse files with more than `2^27` columns. Such a file can
  still be read, sliced, verified and unpacked.
