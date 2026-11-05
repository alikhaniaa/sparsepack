# Reading the code

The code is about 2,600 lines (comments included, tests not), of which
about 570 are the command-line front end. Read it bottom-up: each module
only uses the ones above it in this list.

1. `src/varint.rs`: LEB128 varints, `encode`, `decode`, and `Decoder<'a>`, a
   cursor that borrows a byte slice. Start here: it is small and shows the
   strict-decoding style used everywhere (return an error, never panic, never
   read past the end).
2. `src/crc32.rs`: the CRC-32 table built by a `const fn` at compile time,
   and a bit-by-bit reference used in the tests.
3. `src/rng.rs`: SplitMix64, used by the generator and the tests.
4. `src/error.rs`: the library error enum, written by hand (`Display`,
   `std::error::Error`, `From<io::Error>`).
5. `src/block.rs`: `Block` (CSR layout in memory), `Row<'a>` (a borrowed
   view of one row), and the block encoding and decoding. `Block::decode` is
   the most defensive function in the project.
6. `src/format.rs`: header, footer, trailer and `Index`. `Index::read_from`
   and `Index::validate` decide what a valid file is.
7. `src/writer.rs`: the streaming `Writer`: sort-order checks, closing
   blocks, parallel compression of a batch, the footer.
8. `src/reader.rs`: `Reader<R>`, random access to row ranges, counts of the
   blocks and bytes read.
9. `src/text.rs`: TSV and MatrixMarket parsing (an `Iterator` over
   `Result`s) and fast integer formatting.
10. `src/ops.rs`: the commands as library functions; rayon patterns
    (`map_init`, `try_fold` / `try_reduce`, ordered `collect`).
11. `src/gen.rs`: the Zipf generator.
12. `src/main.rs`: clap definitions, `AtomicFile` (write to `NAME.partial`,
    rename on success, delete on drop), error printing.

Then the tests: `tests/roundtrip.rs` and `tests/corruption.rs` show what the
format promises; `tests/common/mod.rs` has the random matrix helpers.

`docs/FORMAT.md` describes the bytes; keep it open next to `format.rs` and
`block.rs`.

## Five questions to be able to answer

1. **Untrusted counts.** `Block::decode` checks `nnz > room / 2` before
   `Vec::with_capacity(nnz)`, and checks each row's `n` against the
   remaining bytes before the loop. What would a 40-byte file with a
   corrupted (but checksum-correct) `nnz` of 2^60 do without these checks?
   Why is `room / 2` a valid bound (what is the smallest number of bytes one
   non-zero can take)? And why does `Index::validate` make every block
   length safe to allocate?

2. **Borrowing.** `Row<'a>` holds two slices that point into a `Block`, and
   `Decoder<'a>` holds a slice of the bytes it reads. Why does the compiler
   refuse a function that decodes a block into a local variable and returns
   one of its `Row`s? What would you change to return owned data instead,
   and what would it cost? Why does `Writer::new` accept `&mut Vec<u8>` (look
   for `impl Write for &mut W` in the standard library)?

3. **Same bytes at any thread count.** `pack -t 1` and `pack -t 8` write
   identical files (there is a test for it). Follow `Writer::flush_pending`:
   which part runs in parallel, why does `par_iter().map().collect()` keep
   the block order, and why can the block offsets only be assigned after the
   parallel step? What would break if each thread wrote its block to the
   file itself?

4. **Parallel sums without locks.** In `ops::column_totals`, what do
   `map_init`, `try_fold` and `try_reduce` each do? How many column tables
   can exist at once, and what does that mean for memory with 30,000
   columns versus 10 million? Why can `row_sums` simply concatenate per-block
   results instead?

5. **What "every byte is checked" means.** Walk through what happens when a
   single byte is changed in each region: header, a block, the footer, the
   `footer_offset` in the trailer, and the end marker. Which errors come
   out, and which case is only caught with high probability rather than for
   certain? Why does the varint decoder reject `80 00`, even though it would
   decode to a valid 0?
