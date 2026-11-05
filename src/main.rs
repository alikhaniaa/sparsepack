//! Command-line interface of sparsepack.
//!
//! Argument parsing (clap) and output files live here; the work itself is
//! done by library functions in `sparsepack::ops`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use sparsepack::format::{ENTRY_LEN, FOOTER_CRC_LEN, FOOTER_FIXED_LEN, HEADER_LEN, TRAILER_LEN};
use sparsepack::gen::{self, GenParams};
use sparsepack::text::{self, InputFormat};
use sparsepack::{ops, Reader, Writer, DEFAULT_BLOCK_ROWS};

#[derive(Parser)]
#[command(
    name = "sparsepack",
    version,
    about = "Compressed, block-indexed files for sparse integer matrices"
)]
struct Cli {
    /// Worker threads (default: one per logical CPU).
    #[arg(short = 't', long, global = true)]
    threads: Option<usize>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a seeded synthetic matrix (Zipf-skewed columns) as TSV or .spk.
    Gen(GenArgs),
    /// Pack TSV triplets or a MatrixMarket file (sorted by row, then column) into a .spk file.
    Pack(PackArgs),
    /// Write a .spk file as TSV triplets.
    Unpack(UnpackArgs),
    /// Print the non-zeros of a row range (and column range), reading only the blocks needed.
    Slice(SliceArgs),
    /// Drop rows with a small sum and columns with few non-zeros; renumber the rest.
    Filter(FilterArgs),
    /// Print the sum of each column (`col<TAB>sum`), in parallel over blocks.
    Colsums(SumsArgs),
    /// Print the sum of each row (`row<TAB>sum`), in parallel over blocks.
    Rowsums(SumsArgs),
    /// Show shape, size and compression of a .spk file.
    Info(FileArg),
    /// Check the checksum of the header, the footer and every block.
    Verify(FileArg),
}

#[derive(Args)]
struct GenArgs {
    /// Number of rows.
    #[arg(long)]
    rows: u64,
    /// Number of columns.
    #[arg(long)]
    cols: u64,
    /// Mean non-zeros per row (each row draws its count from 0..=2*mean).
    #[arg(long, default_value_t = 100)]
    mean_nnz: u64,
    /// Zipf exponent of column popularity (0 = uniform).
    #[arg(long, default_value_t = 1.0)]
    zipf: f64,
    /// Seed of the random number generator.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Largest value; values are geometric (half of them are 1).
    #[arg(long, default_value_t = 1000)]
    max_value: u32,
    /// Output file: `.spk` writes the packed format, anything else TSV.
    #[arg(short, long)]
    output: PathBuf,
    /// Rows per block (for `.spk` output).
    #[arg(long, default_value_t = DEFAULT_BLOCK_ROWS,
          value_parser = clap::value_parser!(u32).range(1..))]
    block_rows: u32,
}

#[derive(Args)]
struct PackArgs {
    /// Input text file, or `-` for standard input.
    input: PathBuf,
    /// Output .spk file.
    #[arg(short, long)]
    output: PathBuf,
    /// Rows per block.
    #[arg(long, default_value_t = DEFAULT_BLOCK_ROWS,
          value_parser = clap::value_parser!(u32).range(1..))]
    block_rows: u32,
    /// Input format; `auto` picks `mtx` for files ending in `.mtx`, else `tsv`.
    #[arg(long, value_enum, default_value_t = FormatArg::Auto)]
    format: FormatArg,
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Auto,
    /// TSV triplets, 0-based indices.
    Tsv,
    /// MatrixMarket coordinate integer general, 1-based indices.
    Mtx,
}

impl FormatArg {
    fn resolve(self, path: &Path) -> InputFormat {
        match self {
            FormatArg::Tsv => InputFormat::Tsv,
            FormatArg::Mtx => InputFormat::MatrixMarket,
            FormatArg::Auto if path.extension().is_some_and(|e| e == "mtx") => {
                InputFormat::MatrixMarket
            }
            FormatArg::Auto => InputFormat::Tsv,
        }
    }
}

#[derive(Args)]
struct SliceArgs {
    /// Input .spk file.
    input: PathBuf,
    /// Rows `A:B` (0-based, B excluded); `A:` and `:B` leave one end open.
    #[arg(long, value_parser = parse_range)]
    rows: Range<u64>,
    /// Columns `C:D`, same syntax (default: all columns).
    #[arg(long, value_parser = parse_range)]
    cols: Option<Range<u64>>,
    /// Output file (default: standard output).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Parse `A:B`, `A:`, `:B` or `:` into a half-open range.
fn parse_range(text: &str) -> std::result::Result<Range<u64>, String> {
    let (start, end) = text
        .split_once(':')
        .ok_or_else(|| format!("expected START:END, got `{text}`"))?;
    let number = |s: &str, default: u64| {
        if s.is_empty() {
            Ok(default)
        } else {
            s.parse::<u64>().map_err(|e| format!("`{s}`: {e}"))
        }
    };
    let range = number(start, 0)?..number(end, u64::MAX)?;
    if range.start > range.end {
        return Err(format!("start {} is after end {}", range.start, range.end));
    }
    Ok(range)
}

#[derive(Args)]
struct FilterArgs {
    /// Input .spk file.
    input: PathBuf,
    /// Keep rows whose values sum to at least N in the input.
    #[arg(long, value_name = "N", default_value_t = 0)]
    min_row_sum: u64,
    /// Keep columns with at least M non-zeros in the input.
    #[arg(long, value_name = "M", default_value_t = 0)]
    min_col_nnz: u64,
    /// Output .spk file.
    #[arg(short, long)]
    output: PathBuf,
    /// Renumbering table (default: output name with `.map.tsv`).
    #[arg(long)]
    map: Option<PathBuf>,
}

#[derive(Args)]
struct SumsArgs {
    /// Input .spk file.
    input: PathBuf,
    /// Output file (default: standard output).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct FileArg {
    /// Input .spk file.
    input: PathBuf,
}

#[derive(Args)]
struct UnpackArgs {
    /// Input .spk file.
    input: PathBuf,
    /// Output file (default: standard output).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// An output file that is written under a temporary name (`NAME.partial`)
/// and renamed to its real name only by [`AtomicFile::commit`], after its
/// data has been flushed to disk. If the command returns an error first,
/// `Drop` deletes the temporary file. So `NAME` is either absent or complete;
/// only a process killed from outside can leave a `NAME.partial` behind.
struct AtomicFile {
    path: PathBuf,
    tmp: PathBuf,
    /// `None` once `commit` has closed the file.
    file: Option<BufWriter<File>>,
    committed: bool,
}

impl AtomicFile {
    fn create(path: &Path) -> Result<Self> {
        let mut tmp = OsString::from(path.as_os_str());
        tmp.push(".partial");
        let tmp = PathBuf::from(tmp);
        let file =
            File::create(&tmp).with_context(|| format!("cannot create {}", tmp.display()))?;
        Ok(AtomicFile {
            path: path.to_path_buf(),
            tmp,
            file: Some(BufWriter::new(file)),
            committed: false,
        })
    }

    fn writer(&mut self) -> &mut BufWriter<File> {
        self.file.as_mut().expect("file stays open until commit")
    }

    fn commit(mut self) -> Result<()> {
        if let Some(file) = self.file.take() {
            let file = file.into_inner().map_err(|err| err.into_error())?;
            // Data on disk before the new name appears, so a crash right
            // after the rename cannot leave a complete name with lost data.
            file.sync_all()?;
            // `file` is closed here, before the rename (needed on Windows).
        }
        fs::rename(&self.tmp, &self.path).with_context(|| {
            format!(
                "cannot rename {} to {}",
                self.tmp.display(),
                self.path.display()
            )
        })?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.tmp);
        }
    }
}

/// Run `f` on the output file `path` (written atomically) or on standard
/// output when no path is given.
fn with_output(path: Option<&Path>, f: impl FnOnce(&mut dyn Write) -> Result<()>) -> Result<()> {
    match path {
        Some(path) => {
            let mut file = AtomicFile::create(path)?;
            f(file.writer())?;
            file.commit()
        }
        None => {
            let mut out = BufWriter::new(io::stdout().lock());
            f(&mut out)?;
            out.flush()?;
            Ok(())
        }
    }
}

fn open_input(path: &Path) -> Result<Box<dyn io::BufRead>> {
    if path.as_os_str() == "-" {
        return Ok(Box::new(io::stdin().lock()));
    }
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    Ok(Box::new(BufReader::with_capacity(1 << 20, file)))
}

fn gen(args: &GenArgs) -> Result<()> {
    let params = GenParams {
        rows: args.rows,
        cols: args.cols,
        mean_nnz: args.mean_nnz,
        zipf: args.zipf,
        seed: args.seed,
        max_value: args.max_value,
    };
    let mut out = AtomicFile::create(&args.output)?;
    let nnz = if args.output.extension().is_some_and(|e| e == "spk") {
        let mut writer = Writer::new(out.writer(), args.block_rows)?;
        gen::generate(&params, |r, c, v| writer.push(r, c, v))?;
        writer.finish(Some(params.shape()))?.nnz()
    } else {
        let file = out.writer();
        file.write_all(text::shape_line(params.shape()).as_bytes())?;
        let mut buf = Vec::with_capacity(1 << 17);
        let mut nnz = 0u64;
        gen::generate(&params, |r, c, v| {
            text::push_triplet(&mut buf, r, c, v);
            nnz += 1;
            if buf.len() >= 1 << 16 {
                file.write_all(&buf)?;
                buf.clear();
            }
            Ok(())
        })?;
        file.write_all(&buf)?;
        nnz
    };
    out.commit()?;
    eprintln!(
        "generated {} x {} matrix, {nnz} non-zeros ({:.2} per row) -> {}",
        args.rows,
        args.cols,
        nnz as f64 / args.rows.max(1) as f64,
        args.output.display()
    );
    Ok(())
}

fn pack(args: &PackArgs) -> Result<()> {
    let input = open_input(&args.input)?;
    let mut out = AtomicFile::create(&args.output)?;
    let format = args.format.resolve(&args.input);
    let index = ops::pack(input, format, out.writer(), args.block_rows)
        .with_context(|| format!("cannot pack {}", args.input.display()))?;
    out.commit()?;
    let shape = index.shape();
    eprintln!(
        "packed {} x {} matrix, {} non-zeros, {} -> {}",
        shape.rows,
        shape.cols,
        index.nnz(),
        blocks(index.blocks().len()),
        args.output.display()
    );
    Ok(())
}

fn unpack(args: &UnpackArgs) -> Result<()> {
    let reader = open_spk(&args.input)?;
    with_output(args.output.as_deref(), |out| {
        ops::unpack(&args.input, reader.index(), out)
            .with_context(|| format!("cannot unpack {}", args.input.display()))
    })
}

fn slice(args: &SliceArgs) -> Result<()> {
    let mut reader = open_spk(&args.input)?;
    let cols = args.cols.clone().unwrap_or(0..u64::MAX);
    let mut found = 0u64;
    with_output(args.output.as_deref(), |out| {
        let mut buf = Vec::with_capacity(1 << 17);
        reader.for_each_in(args.rows.clone(), cols.clone(), |r, c, v| {
            text::push_triplet(&mut buf, r, c, v);
            found += 1;
            if buf.len() >= 1 << 16 {
                out.write_all(&buf)?;
                buf.clear();
            }
            Ok(())
        })?;
        out.write_all(&buf)?;
        Ok(())
    })?;
    let index = reader.index();
    let total_bytes: u64 = index.blocks().iter().map(|b| u64::from(b.len)).sum();
    eprintln!(
        "rows {}..{}, cols {}..{}: {found} non-zeros; read {} of {} ({} of {} block bytes)",
        args.rows.start,
        args.rows.end.min(index.shape().rows),
        cols.start,
        cols.end.min(index.shape().cols),
        reader.blocks_read(),
        blocks(index.blocks().len()),
        reader.bytes_read(),
        total_bytes
    );
    Ok(())
}

fn filter(args: &FilterArgs) -> Result<()> {
    let reader = open_spk(&args.input)?;
    let index = reader.index();
    let map_path = args
        .map
        .clone()
        .unwrap_or_else(|| args.output.with_extension("map.tsv"));
    let options = ops::FilterOptions {
        min_row_sum: args.min_row_sum,
        min_col_nnz: args.min_col_nnz,
        block_rows: index.block_rows(),
    };
    let mut out = AtomicFile::create(&args.output)?;
    let mut map = AtomicFile::create(&map_path)?;
    let summary = ops::filter(&args.input, index, options, out.writer(), map.writer())
        .with_context(|| format!("cannot filter {}", args.input.display()))?;
    // The map goes into place first: whenever the filtered matrix exists
    // under its name, its renumbering table is complete next to it.
    map.commit()?;
    out.commit()?;
    let (before, after) = (index.shape(), summary.index.shape());
    eprintln!(
        "kept {} of {} rows, {} of {} columns, {} of {} non-zeros -> {} (map: {})",
        after.rows,
        before.rows,
        after.cols,
        before.cols,
        summary.index.nnz(),
        index.nnz(),
        args.output.display(),
        map_path.display()
    );
    Ok(())
}

/// Which sums `sums` prints.
#[derive(Clone, Copy)]
enum Axis {
    Rows,
    Cols,
}

fn sums(args: &SumsArgs, axis: Axis) -> Result<()> {
    let reader = open_spk(&args.input)?;
    let index = reader.index();
    let totals = match axis {
        Axis::Rows => ops::row_sums(&args.input, index),
        Axis::Cols => ops::col_sums(&args.input, index),
    }
    .with_context(|| format!("cannot read {}", args.input.display()))?;
    with_output(args.output.as_deref(), |out| {
        let mut buf = Vec::with_capacity(1 << 17);
        for (i, &total) in totals.iter().enumerate() {
            text::push_u64(&mut buf, i as u64);
            buf.push(b'\t');
            text::push_u64(&mut buf, total);
            buf.push(b'\n');
            if buf.len() >= 1 << 16 {
                out.write_all(&buf)?;
                buf.clear();
            }
        }
        out.write_all(&buf)?;
        Ok(())
    })
}

fn info(args: &FileArg) -> Result<()> {
    let reader = open_spk(&args.input)?;
    let index = reader.index();
    let shape = index.shape();
    let nnz = index.nnz();
    let blocks = index.blocks().len();
    let file_bytes = fs::metadata(&args.input)?.len();
    let overhead =
        HEADER_LEN + FOOTER_FIXED_LEN + ENTRY_LEN * blocks + FOOTER_CRC_LEN + TRAILER_LEN;
    let tsv_bytes = ops::tsv_size(&args.input, index)?;
    let positions = shape.rows as f64 * shape.cols as f64;
    let density = if positions > 0.0 {
        nnz as f64 / positions
    } else {
        0.0
    };
    let per_nnz = |bytes: u64| {
        if nnz > 0 {
            format!("{:.3}", bytes as f64 / nnz as f64)
        } else {
            "-".to_string()
        }
    };
    let lines = [
        ("shape", format!("{} x {}", shape.rows, shape.cols)),
        ("nnz", nnz.to_string()),
        ("density", format!("{density:.6}")),
        ("block_rows", index.block_rows().to_string()),
        ("blocks", blocks.to_string()),
        ("file_bytes", file_bytes.to_string()),
        (
            "overhead_bytes",
            format!("{overhead} (header, footer, trailer)"),
        ),
        ("bytes_per_nnz", per_nnz(file_bytes)),
        ("tsv_bytes", format!("{tsv_bytes} (what unpack writes)")),
        (
            "u32_triplet_bytes",
            format!("{} (12 bytes per non-zero)", 12 * nnz),
        ),
        (
            "csr_bytes",
            format!(
                "{} (u32 columns and values, u64 row offsets)",
                8 * nnz + 8 * (shape.rows + 1)
            ),
        ),
        (
            "spk_vs_tsv",
            format!("{:.2}%", 100.0 * file_bytes as f64 / tsv_bytes as f64),
        ),
    ];
    for (key, value) in lines {
        println!("{key:<18} {value}");
    }
    Ok(())
}

fn verify(args: &FileArg) -> Result<()> {
    let reader = open_spk(&args.input)?;
    let index = reader.index();
    let errors = ops::verify(&args.input, index);
    for err in &errors {
        eprintln!("{}: {err}", args.input.display());
    }
    let total = blocks(index.blocks().len());
    if !errors.is_empty() {
        bail!("{} of {total} damaged", errors.len());
    }
    println!("ok: header, footer and {total} verified");
    Ok(())
}

/// "1 block", "2 blocks".
fn blocks(n: usize) -> String {
    if n == 1 {
        "1 block".to_string()
    } else {
        format!("{n} blocks")
    }
}

fn open_spk(path: &Path) -> Result<Reader<File>> {
    Reader::open(path).with_context(|| format!("cannot read {}", path.display()))
}

fn run(cli: Cli) -> Result<()> {
    if let Some(threads) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .context("cannot start the thread pool")?;
    }
    match &cli.command {
        Command::Gen(args) => gen(args),
        Command::Pack(args) => pack(args),
        Command::Unpack(args) => unpack(args),
        Command::Slice(args) => slice(args),
        Command::Filter(args) => filter(args),
        Command::Colsums(args) => sums(args, Axis::Cols),
        Command::Rowsums(args) => sums(args, Axis::Rows),
        Command::Info(args) => info(args),
        Command::Verify(args) => verify(args),
    }
}

/// True if the error comes from writing into a closed pipe, as in
/// `sparsepack unpack x.spk | head`. That is not worth an error message.
fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
    })
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if is_broken_pipe(&err) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("sparsepack: {err:#}");
            ExitCode::FAILURE
        }
    }
}
