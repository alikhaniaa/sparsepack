# Reading the code

The code is about 2,700 lines (comments included, tests not), of which
about 590 are the command-line front end. Read it bottom-up: each module
only uses the ones above it in this list.

1. `src/varint.rs`: LEB128 varints, `encode`, `decode`, and `Decoder`, a
   cursor over a byte slice. Start here: it is small and shows the
   strict-decoding style used everywhere (return an error, never panic, never
   read past the end).
2. `src/crc32.rs`: the CRC-32 table, built at compile time, and a
   bit-by-bit reference used in the tests.
3. `src/rng.rs`: SplitMix64, used by the generator and the tests.
4. `src/error.rs`: the library error enum.
5. `src/table.rs`: per-column tables with a size limit and fallible
   allocation, so a file that declares huge dimensions gives an error.
6. `src/block.rs`: `Block` (CSR layout in memory), `Row` (a borrowed view of
   one row), and the block encoding and decoding. `Block::decode` is the most
   defensive function in the project.
7. `src/format.rs`: header, footer, trailer and `Index`. `Index::read_from`
   and `Index::validate` decide what a valid file is.
8. `src/writer.rs`: the streaming `Writer`: sort-order checks, closing
   blocks, parallel compression of a batch, the footer.
9. `src/reader.rs`: `Reader`, random access to row ranges, counts of the
   blocks and bytes read.
10. `src/text.rs`: TSV and MatrixMarket parsing and fast integer formatting.
11. `src/gen.rs`: the Zipf generator.
12. `src/ops.rs`: the commands as library functions, with the parallel
    patterns: one file handle per rayon job, per-thread column tables,
    results collected in block order.
13. `src/main.rs`: clap definitions, `AtomicFile` (write to `NAME.partial`,
    sync, rename on success, delete on error), error printing.

Then the tests: `tests/roundtrip.rs` and `tests/corruption.rs` show what the
format promises; `tests/common/mod.rs` has the random matrix helpers.

`docs/FORMAT.md` describes the bytes; keep it open next to `format.rs` and
`block.rs`.
