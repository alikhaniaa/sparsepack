# sparsepack

A small Rust library and command-line tool that stores sparse matrices of
small non-negative integers in a compressed, block-indexed binary file
(`.spk`), written as a learning project. Think of a count matrix with
100,000 rows and 30,000 columns where each row has a few hundred non-zeros:
documents x terms, or users x items.

The file format, the varint compression and the CRC-32 checksums are
implemented by hand. Rows are grouped into blocks that are compressed
independently, and a footer index lists every block, so a reader can
decode only the rows it needs. Packing, unpacking and the whole-matrix
operations run in parallel with rayon.

All data used in the tests and benchmarks is synthetic, made by the tool's
own seeded generator (`sparsepack gen`).

## Build and run

Developed with rustc 1.99.1; older toolchains were not tested.
Dependencies: `clap`, `rayon` and `anyhow`; the tests need nothing extra.

```
cargo build --release
cargo test
./target/release/sparsepack --help
```

| command    | what it does |
|------------|--------------|
| `gen`      | seeded synthetic matrix: `--rows`, `--cols`, `--mean-nnz` per row, `--zipf` exponent of column popularity, `--seed`, `--max-value`; writes TSV, or `.spk` if the output name ends in `.spk`; same seed, same bytes |
| `pack`     | TSV triplets (`row col value`, 0-based) or MatrixMarket coordinate integer (1-based) to `.spk`; streaming, so the input is never held in memory; entries must be sorted by row, then column |
| `unpack`   | `.spk` to TSV triplets; `pack` then `unpack` gives back the same bytes |
| `info`     | shape, nnz, density, blocks, file size, bytes per non-zero, and the size of the same data as TSV, as u32 triplets and as CSR |
| `slice`    | non-zeros of `--rows A:B` (and optionally `--cols C:D`), reading only the blocks that hold those rows; reports how many blocks and bytes were read |
| `filter`   | drop rows whose sum is below `--min-row-sum N` and columns with fewer than `--min-col-nnz M` non-zeros, renumber the rest, write a renumbering table |
| `colsums`, `rowsums` | sum of each column or row, in parallel over blocks |
| `verify`   | check the header, footer and every block checksum; exit status 1 if anything is damaged |

`-t/--threads N` sets the number of worker threads (default: all logical
CPUs). Output files are written as `NAME.partial` and renamed when complete,
so a failed command does not leave a half-written file.

**Text formats.** TSV triplets have one non-zero per line,
`row<TAB>col<TAB>value`. A first line `# shape ROWS COLS` gives the shape;
without it the shape is the smallest that holds all entries. Other `#`
lines are comments. Values must be in 1..=4294967295 (zeros are not stored,
so an explicit 0 is an error). `unpack` writes the shape line and then one
line per non-zero.

**Filter, precisely.** Both thresholds are evaluated on the input matrix,
independently: a column is kept if it has at least M non-zeros over all
input rows, and a row is kept if its values sum to at least N over all
input columns, including columns that are dropped. Kept rows and columns
keep their order and are numbered from 0. The filter is applied once, not
repeated until stable, so the output can contain rows that now sum to less
than N, columns with fewer than M non-zeros, and even empty rows. The map
file (default: output name with `.map.tsv`) has a `# kind new old` header,
one `col NEW OLD` line per kept column and one `row NEW OLD` line per kept
row; `row 5 17` means row 5 of the output is row 17 of the input.

## Example

Copied from `results/examples.txt` (made by `scripts/examples.sh`; the
outputs of `slice` and `info` are shortened here).

```
$ sparsepack gen --rows 5000 --cols 2000 --mean-nnz 40 --zipf 1.0 --seed 7 -o m.tsv
generated 5000 x 2000 matrix, 201136 non-zeros (40.23 per row) -> m.tsv

$ head -n 4 m.tsv
# shape 5000 2000
0	527	1
0	659	2
0	697	1

$ sparsepack pack m.tsv -o m.spk
packed 5000 x 2000 matrix, 201136 non-zeros, 5 blocks -> m.spk

$ sparsepack info m.spk
shape              5000 x 2000
nnz                201136
density            0.020114
block_rows         1024
blocks             5
file_bytes         421167
overhead_bytes     252 (header, footer, trailer)
bytes_per_nnz      2.094
tsv_bytes          2252194 (what unpack writes)
u32_triplet_bytes  2413632 (12 bytes per non-zero)
csr_bytes          1649096 (u32 columns and values, u64 row offsets)
spk_vs_tsv         18.70%

$ sparsepack slice m.spk --rows 2500:2502 --cols 0:400
2500	28	1
2500	68	2
...
2501	333	1
rows 2500..2502, cols 0..400: 16 non-zeros; read 1 of 5 blocks (90021 of 420915 block bytes)

$ sparsepack unpack m.spk | cmp - m.tsv && echo identical
identical

$ sparsepack filter m.spk --min-row-sum 60 --min-col-nnz 20 -o small.spk
kept 3106 of 5000 rows, 1274 of 2000 columns, 163754 of 201136 non-zeros -> small.spk (map: small.map.tsv)

$ sparsepack pack unsorted.tsv -o unsorted.spk || echo "exit status $?"
sparsepack: cannot pack unsorted.tsv: invalid input: line 2: column 3 comes after column 5 in row 0: columns must be strictly increasing within a row
exit status 1

# flip one bit of byte 5000 of m.spk (inside a block)
$ sparsepack verify m.spk || echo "exit status $?"
m.spk: checksum mismatch in block 0: stored 7b4ae2ff, computed 563a6aff
sparsepack: 1 of 5 blocks damaged
exit status 1

$ head -c 60000 m.spk > cut.spk
$ sparsepack info cut.spk || echo "exit status $?"
sparsepack: cannot read cut.spk: truncated file: end marker missing (the file was cut off or is not a sparsepack file)
exit status 1
```

## The format in short

```
| header 20 B | block 0 | block 1 | ... | footer 36 + 36 per block | trailer 16 B |
```

- **Header:** magic `SPARSEPK`, version, rows per block, CRC-32.
- **Blocks:** block `i` holds rows `i * block_rows` up to the next block
  (default 1024 rows; the last block may be shorter). Each row is stored as
  its number of non-zeros, then the column gaps, then the values, all as
  LEB128 varints. The first gap is the first column; each later gap is
  `column - previous column - 1`.
- **Footer:** rows, columns, nnz, and per block its offset, length, first
  row, row count, nnz and CRC-32; then a CRC-32 of the footer.
- **Trailer:** the footer offset and the end marker `SPKTRAIL`.

A reader reads the header and the last 16 bytes, then the footer, and then
only the blocks it needs. The byte-level tables, the checks a reader makes
and a hex dump of a small file are in [`docs/FORMAT.md`](docs/FORMAT.md).

## Results

Machine (`results/machine.txt`): Intel Core i7-6700 @ 3.40 GHz, 4 cores / 8
logical CPUs, 23.5 GiB RAM, Linux under WSL2, rustc 1.99.1, Python 3.14.8,
gzip 1.15. All matrices come from `sparsepack gen` with Zipf exponent 1.0
and seed 42; MB means 10^6 bytes.

**Sizes** (`results/sizes.tsv`, made by `scripts/bench.sh`):

| matrix | shape | non-zeros | TSV | TSV + gzip -6 | u32 triplets | CSR (u32) | .spk | .spk bytes per non-zero |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| small  | 10,000 x 5,000    | 1,011,430  | 11.8 MB   | 3.16 MB  | 12.1 MB   | 8.17 MB  | 2.10 MB  | 2.08 |
| medium | 100,000 x 30,000  | 30,041,687 | 406.0 MB  | 99.5 MB  | 360.5 MB  | 241.1 MB | 66.8 MB  | 2.22 |
| large  | 300,000 x 50,000  | 90,074,194 | 1298.0 MB | 306.3 MB | 1080.9 MB | 723.0 MB | 214.5 MB | 2.38 |

The `.spk` file is about 16-18% of the TSV and 67-70% of the gzipped TSV.
"CSR (u32)" is an uncompressed in-memory layout with 4-byte column indices
and values and 8-byte row offsets, computed by `info`, not written to disk.

**Times** (median of 3 runs in seconds, warm page cache,
`results/bench_summary.tsv`; raw runs in `results/bench_raw.tsv`):

| operation | small | medium | large |
|---|---:|---:|---:|
| pack TSV -> .spk, 1 thread | 0.092 | 2.823 | 8.537 |
| pack TSV -> .spk, 8 threads | 0.086 | 2.417 | 7.480 |
| unpack .spk -> TSV file, 1 thread | 0.074 | 2.261 | 11.915 |
| unpack .spk -> TSV file, 8 threads | 0.047 | 5.596 | 15.439 |
| unpack .spk -> /dev/null, 1 thread | 0.058 | 1.873 | 5.881 |
| unpack .spk -> /dev/null, 8 threads | 0.025 | 0.478 | 1.483 |
| slice of 1% of the rows | 0.006 | 0.031 | 0.070 |
| colsums, 1 thread | 0.020 | 0.523 | 1.772 |
| colsums, 8 threads | 0.015 | 0.150 | 0.462 |
| colsums in pure Python on the TSV (`scripts/py_colsums.py`) | 0.353 | 10.413 | 31.983 |
| gzip -6 of the TSV (one run) | 0.902 | 21.695 | 61.545 |

What these numbers say:

- `colsums` on 8 threads is 3.5x (medium) and 3.8x (large) faster than on
  one thread, which is about what 4 physical cores allow. The single-thread
  Python loop is 20x slower than one Rust thread on the medium matrix; it
  is a weak baseline (no numpy, and it parses text while sparsepack reads a
  compressed binary file).
- `pack` hardly gains from threads (2.82 s to 2.42 s on medium): parsing
  the text is single-threaded and is most of the work; only block
  compression runs in parallel.
- Unpacking to `/dev/null` scales well (1.87 s to 0.48 s on medium), but
  writing the 0.4-1.3 GB of text to a file is limited by the disk and is
  very noisy: the medium 8-thread runs took between 1.0 s and 6.3 s, and
  the median is slower than one thread. These timings measure the write
  path of this WSL2 machine more than the decoder.
- A 1% slice reads 2 of 98 blocks (1.4 MB of 66.8 MB) on medium and 4 of
  293 blocks on large (`results/slice_reports.txt`); the time is mostly
  process start-up.

**Block size** (medium matrix; `scripts/block_size.sh`,
`results/block_size.tsv`, `results/block_size_times.tsv`):

| rows per block | blocks | file size | index overhead | 1,000-row slice: blocks / bytes read / time | 1-row slice: bytes read / time | colsums, 8 threads |
|---:|---:|---:|---:|---|---|---:|
| 128  | 782 | 66,810,515 B | 28,224 B | 9 / 771,735 B / 0.025 s   | 88,168 B / 0.003 s    | 0.169 s |
| 1024 | 98  | 66,785,891 B | 3,600 B  | 2 / 1,373,567 B / 0.031 s | 693,678 B / 0.010 s   | 0.145 s |
| 8192 | 13  | 66,782,831 B | 540 B    | 1 / 5,495,923 B / 0.082 s | 5,495,923 B / 0.073 s | 0.249 s |

Block size barely changes the file size (0.04% between 128 and 8192 rows):
rows are compressed on their own, so a block only adds its 36-byte index
entry. It changes the cost of random access a lot: one row costs a whole
block, 88 KB with 128-row blocks and 5.5 MB with 8192-row blocks. Very
large blocks also hurt the parallel operations: with 13 blocks on 8 threads
the work is uneven, and `colsums` needed 254 MB instead of 47 MB (peak
resident memory): each of the 8 threads holds a decoded block, and a decoded
8192-row block has about 2.5 million non-zeros, roughly 30 MB in memory.

**Where .spk does badly** (`scripts/edge_cases.sh`, `results/edge_sizes.tsv`):

| case | non-zeros | TSV | TSV + gzip -6 | .spk | .spk + gzip -6 |
|---|---:|---:|---:|---:|---:|
| tiny, 10 x 10 | 27 | 176 B | 126 B | 172 B | 142 B |
| 1000 x 5000, 5 per row | 4,850 | 51,996 B | 17,674 B | 14,979 B | 10,864 B |
| 40% dense, 2000 x 1000 | 801,194 | 8,275,932 B | 2,125,165 B | 1,606,330 B | 525,172 B |
| 75% dense, all values 1, 2000 x 100 | 150,177 | 1,403,371 B | 323,468 B | 302,498 B | 17,906 B |
| uniform columns (Zipf 0), 20000 x 30000 | 6,042,934 | 79.0 MB | 21.0 MB | 13.4 MB | 9.1 MB |
| Zipf 1.5, 20000 x 30000 | 6,042,934 | 78.9 MB | 19.3 MB | 13.5 MB | 3.8 MB |

- **Tiny files lose.** 108 of the 172 bytes of the 10 x 10 file are fixed
  header, footer and trailer, and the gzipped TSV is smaller.
- **Dense data shows the 2-byte floor.** Every non-zero costs at least one
  byte for its gap and one for its value, so dense matrices stay at about
  2.0 bytes per non-zero. I expected gzip on the TSV to beat .spk on dense
  data; in these cases it did not (2.65 and 2.15 bytes per non-zero), but
  gzip applied to the .spk file shrinks it 3x (40% dense) to 17x (all ones).
  A bitmap or an entropy coder would do much better than varints here.
- **Column skew is not used.** The .spk size is the same (2.22-2.23 bytes
  per non-zero) for uniform and strongly skewed columns, while gzip of the
  .spk file finds the repeated popular columns (3.8 MB instead of 13.5 MB at
  Zipf 1.5). The generator spreads popular columns over the index range on
  purpose; data whose popular columns have small indices would compress
  better than shown here.

**Correctness** (`results/checks.tsv`, `results/quality.txt`). For all
three benchmark matrices, `unpack` gives back the generated TSV byte for
byte, `colsums` at 1 and 8 threads and the Python script give identical
output, `info` predicts the TSV size exactly and `verify` passes. `cargo fmt
--check` and `cargo clippy --all-targets -- -D warnings` are clean and the
76 tests pass (`cargo test --release`). The tests include varint edge and
malformed cases, the CRC-32 check value, round trips of 300 random matrices
and of edge shapes, every single-byte flip and every truncation of several
small files, a random-damage test with repaired checksums, slice and filter
against naive in-memory versions, MatrixMarket against TSV, and the
command-line tool run in temporary directories.

## Design notes

- **Delta + varint.** Inside a row the columns are sorted, so storing the
  gap to the previous column instead of the column itself gives small
  numbers: with 300 non-zeros spread over 30,000 columns the typical gap is
  around 100. A varint stores numbers below 128 in one byte, and most
  values of count data are small too. This is simple, byte-aligned and fast
  to decode, at the price of the 2-byte floor shown above.
- **Independent blocks.** A block can be decoded without any other block.
  That makes random access possible (decode only the blocks that hold the
  requested rows), lets compression and decompression run in parallel, and
  keeps damage local: `verify` names the damaged block and the other blocks
  stay readable. Blocks have a fixed number of rows, so the block of row
  `r` is `r / block_rows`; the reader still finds blocks by binary search
  over the index.
- **Footer index.** The writer streams: it writes each block as soon as it
  is full and keeps only the 36-byte index entries in memory. The total nnz
  (and the shape, when the input does not declare it) is known only at the
  end, so it goes into the footer, and the writer never seeks back; it could
  write to a pipe. A fixed-size trailer at the end points to the footer, so
  a reader starts by reading the last 16 bytes. ZIP and Parquet put their
  directories at the end for the same reason.
- **A CRC-32 per block.** A slice checks only the blocks it reads, and a
  damaged file says which block is bad. The header and the footer have
  their own CRCs. CRC-32 detects accidental damage (every error burst of up
  to 32 bits), not deliberate tampering.
- **Strict reading.** Every count read from a file is checked against the
  bytes that are actually there before it is used to allocate memory, and
  varints must use their shortest encoding. Damaged or cut-off files give
  an error message, never a panic; the tests check this byte by byte.
- **Same output at any thread count.** Blocks are compressed in parallel,
  collected in order and written by one thread, so `pack -t 1` and
  `pack -t 8` write identical files (tested).

## Limitations

- Values are `u32` integers from 1 to 4294967295. No zeros as values, no
  negative numbers, no floats.
- Only row-wise access: there is no column index, so reading one column
  means reading every block.
- Input rows must be sorted (and columns within a row); `pack` stops with an
  error otherwise. There is no external sort.
- Single machine, single file. No appends or in-place updates: changing the
  matrix means writing a new file.
- `colsums`, `col_nnz` and `filter` keep one 8-byte counter per column for
  each worker thread (fewer threads when needed), at most 2^27 counters
  (1 GiB) in total; a matrix with more than 2^27 columns is refused with an
  error before anything is allocated. `gen` keeps three such tables and
  accepts at most 44,739,242 columns.
- The declared shape is trusted: a `# shape` line with 10^12 rows makes the
  writer emit every empty row (one byte each).
- `gen` uses floating-point `powf` for its Zipf table, so the output is
  byte-identical for a seed on one machine, but the last bit of `powf` could
  in principle differ on another platform's math library.
- All benchmark data is synthetic, from one simple model (Zipf columns,
  uniform row lengths, geometric values). Real data can compress better or
  worse.
- Tested on Linux only (CI runs on Ubuntu).
- This is a learning project. It is not a replacement for established
  formats and libraries such as CSR in HDF5, Parquet, Zarr or the
  MatrixMarket tools, which are faster, more general and far better tested.

## Reproducing the results

```
bash scripts/bench.sh        # sizes and times, 3 matrices -> results/bench_*.tsv, sizes.tsv, checks.tsv, machine.txt
bash scripts/block_size.sh   # block sizes 128, 1024, 8192 -> results/block_size*.tsv
bash scripts/edge_cases.sh   # tiny, dense, skewed         -> results/edge_sizes.tsv
bash scripts/examples.sh     # the example above           -> results/examples.txt
bash scripts/quality.sh      # fmt, clippy, tests          -> results/quality.txt
```

The scripts need bash, Python 3 (standard library only) and gzip. Data goes
to `data/` (not committed); `bench.sh` leaves about 2.3 GB there (more
while it runs) and took about 8 minutes on the machine above.

## Reading the code

Start at `src/varint.rs` and go up through `crc32.rs`, `block.rs`,
`format.rs`, `writer.rs` and `reader.rs`; the full reading order and five
questions to check your understanding are in
[`docs/READING.md`](docs/READING.md).

## License

MIT, see `LICENSE`.
