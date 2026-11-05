//! Command-line interface of sparsepack.
//!
//! Argument parsing (clap) and output files live here; the work itself is
//! done by library functions in `sparsepack::ops`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sparsepack::{ops, Reader, DEFAULT_BLOCK_ROWS};

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
    /// Pack TSV triplets (row, col, value; sorted by row, then col) into a .spk file.
    Pack(PackArgs),
    /// Write a .spk file as TSV triplets.
    Unpack(UnpackArgs),
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
/// and renamed to its real name only by [`AtomicFile::commit`]. If the
/// command fails first, `Drop` deletes the temporary file, so a failed run
/// never leaves a half-written output behind.
struct AtomicFile {
    path: PathBuf,
    tmp: PathBuf,
    /// `None` after `commit` has closed the file.
    file: Option<BufWriter<File>>,
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
        })
    }

    fn writer(&mut self) -> &mut BufWriter<File> {
        self.file.as_mut().expect("file stays open until commit")
    }

    fn commit(mut self) -> Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
            // The file is closed here, before the rename (needed on Windows).
        }
        fs::rename(&self.tmp, &self.path).with_context(|| {
            format!(
                "cannot rename {} to {}",
                self.tmp.display(),
                self.path.display()
            )
        })
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        // Still open means `commit` was not reached: remove the partial file.
        if self.file.take().is_some() {
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

fn pack(args: &PackArgs) -> Result<()> {
    let input = open_input(&args.input)?;
    let mut out = AtomicFile::create(&args.output)?;
    let index = ops::pack(input, out.writer(), args.block_rows)
        .with_context(|| format!("cannot pack {}", args.input.display()))?;
    out.commit()?;
    let shape = index.shape();
    eprintln!(
        "packed {} x {} matrix, {} non-zeros, {} blocks -> {}",
        shape.rows,
        shape.cols,
        index.nnz(),
        index.blocks().len(),
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
        Command::Pack(args) => pack(args),
        Command::Unpack(args) => unpack(args),
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
