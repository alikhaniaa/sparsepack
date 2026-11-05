//! Run the `sparsepack` binary the way a user would.

mod common;

use std::fs;
use std::ops::Range;
use std::path::Path;
use std::process::{Command, Output};

use common::{random_matrix, to_tsv, Matrix, TempDir};
use sparsepack::rng::SplitMix64;

fn sparsepack(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sparsepack"))
        .args(args)
        .output()
        .expect("run sparsepack")
}

/// Run and require success; return standard output.
fn ok(args: &[&str]) -> String {
    let out = sparsepack(args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 output")
}

/// Run and require failure; return standard error.
fn fails(args: &[&str]) -> String {
    let out = sparsepack(args);
    assert!(!out.status.success(), "{args:?} should fail");
    String::from_utf8(out.stderr).expect("utf-8 output")
}

fn s(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

#[test]
fn pack_then_unpack_gives_the_same_text() {
    let dir = TempDir::new("roundtrip");
    let mut rng = SplitMix64::new(8);
    let matrices = [
        random_matrix(&mut rng, 500, 300, 30, 0.2),
        Matrix::new(0, 0, vec![]),
        Matrix::new(10, 10, vec![]),
        Matrix::new(3, 4, vec![(2, 3, 9)]),
    ];
    for (k, m) in matrices.iter().enumerate() {
        let tsv = dir.join(&format!("m{k}.tsv"));
        let spk = dir.join(&format!("m{k}.spk"));
        let text = to_tsv(m);
        fs::write(&tsv, &text).unwrap();
        for block_rows in ["1", "7", "1024"] {
            ok(&["pack", s(&tsv), "-o", s(&spk), "--block-rows", block_rows]);
            assert_eq!(ok(&["unpack", s(&spk)]), text);
            assert_eq!(ok(&["-t", "1", "unpack", s(&spk)]), text);
        }
        let out = dir.join(&format!("m{k}.out.tsv"));
        ok(&["unpack", s(&spk), "-o", s(&out)]);
        assert_eq!(fs::read_to_string(&out).unwrap(), text);
    }
}

#[test]
fn thread_count_does_not_change_the_file() {
    let dir = TempDir::new("threads");
    let mut rng = SplitMix64::new(9);
    let tsv = dir.join("m.tsv");
    fs::write(&tsv, to_tsv(&random_matrix(&mut rng, 3000, 500, 40, 0.1))).unwrap();
    let mut files = Vec::new();
    for threads in ["1", "2", "8"] {
        let spk = dir.join(&format!("t{threads}.spk"));
        ok(&[
            "-t",
            threads,
            "pack",
            s(&tsv),
            "-o",
            s(&spk),
            "--block-rows",
            "16",
        ]);
        files.push(fs::read(&spk).unwrap());
    }
    assert_eq!(files[0], files[1]);
    assert_eq!(files[0], files[2]);
}

#[test]
fn pack_reads_standard_input() {
    let dir = TempDir::new("stdin");
    let spk = dir.join("m.spk");
    let text = "# shape 2 3\n0\t2\t1\n1\t0\t5\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_sparsepack"))
        .args(["pack", "-", "-o", s(&spk)])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(ok(&["unpack", s(&spk)]), text);
}

#[test]
fn pack_reads_matrix_market_by_extension() {
    let dir = TempDir::new("mtx");
    let mtx = dir.join("m.mtx");
    let spk = dir.join("m.spk");
    fs::write(
        &mtx,
        "%%MatrixMarket matrix coordinate integer general
% made by hand
3 4 2
1 2 5
3 4 1
",
    )
    .unwrap();
    ok(&["pack", s(&mtx), "-o", s(&spk)]);
    assert_eq!(
        ok(&["unpack", s(&spk)]),
        "# shape 3 4
0	1	5
2	3	1
"
    );
    let err = fails(&["pack", s(&mtx), "-o", s(&spk), "--format", "tsv"]);
    assert!(err.contains("line 1"), "{err}");
}

#[test]
fn shape_is_inferred_without_a_shape_line() {
    let dir = TempDir::new("infer");
    let tsv = dir.join("m.tsv");
    let spk = dir.join("m.spk");
    fs::write(&tsv, "1\t4\t2\n3\t0\t1\n").unwrap();
    ok(&["pack", s(&tsv), "-o", s(&spk)]);
    assert_eq!(ok(&["unpack", s(&spk)]), "# shape 4 5\n1\t4\t2\n3\t0\t1\n");
}

#[test]
fn bad_input_fails_with_a_clear_message_and_no_output() {
    let dir = TempDir::new("bad");
    let cases = [
        ("0\t0\t1\n0\t5\t1\n0\t3\t1\n", "line 3"),
        ("2\t0\t1\n1\t0\t1\n", "rows must be sorted"),
        ("0\t0\t1\n0\t0\t2\n", "duplicate"),
        ("0\t0\t0\n", "value 0"),
        ("# shape 2 2\n0\t2\t1\n", "outside the declared"),
        ("0 zero 1\n", "line 1"),
    ];
    for (k, (text, expected)) in cases.iter().enumerate() {
        let tsv = dir.join(&format!("bad{k}.tsv"));
        let spk = dir.join(&format!("bad{k}.spk"));
        fs::write(&tsv, text).unwrap();
        let err = fails(&["pack", s(&tsv), "-o", s(&spk)]);
        assert!(err.contains(expected), "{text:?}: {err}");
        assert!(!spk.exists(), "no output file after an error");
        assert!(!dir.join(&format!("bad{k}.spk.partial")).exists());
    }
}

#[test]
fn damaged_file_fails_cleanly() {
    let dir = TempDir::new("damaged");
    let tsv = dir.join("m.tsv");
    let spk = dir.join("m.spk");
    fs::write(&tsv, to_tsv(&Matrix::new(5, 5, vec![(0, 1, 2), (4, 4, 4)]))).unwrap();
    ok(&["pack", s(&tsv), "-o", s(&spk)]);
    let mut bytes = fs::read(&spk).unwrap();
    bytes[21] ^= 0x40;
    fs::write(&spk, &bytes).unwrap();
    let err = fails(&["unpack", s(&spk)]);
    assert!(err.contains("checksum mismatch in block 0"), "{err}");
    fs::write(&spk, &bytes[..bytes.len() - 3]).unwrap();
    let err = fails(&["unpack", s(&spk)]);
    assert!(err.contains("truncated"), "{err}");
    let err = fails(&["unpack", s(&dir.join("missing.spk"))]);
    assert!(err.contains("cannot read"), "{err}");
}

#[test]
fn gen_is_deterministic() {
    let dir = TempDir::new("gen");
    let args = |out: &Path, seed: &str| -> Vec<String> {
        [
            "gen",
            "--rows",
            "700",
            "--cols",
            "300",
            "--mean-nnz",
            "20",
            "--zipf",
            "1.1",
            "--seed",
            seed,
            "--block-rows",
            "64",
            "-o",
            s(out),
        ]
        .map(String::from)
        .to_vec()
    };
    let run = |a: Vec<String>| ok(&a.iter().map(String::as_str).collect::<Vec<_>>());
    let (a, b, c) = (dir.join("a.tsv"), dir.join("b.tsv"), dir.join("c.tsv"));
    run(args(&a, "5"));
    run(args(&b, "5"));
    run(args(&c, "6"));
    let text = fs::read(&a).unwrap();
    assert_eq!(text, fs::read(&b).unwrap(), "same seed, same bytes");
    assert_ne!(text, fs::read(&c).unwrap(), "different seed");

    // Generating .spk directly equals generating TSV and packing it,
    // at any thread count.
    let direct = dir.join("direct.spk");
    let packed = dir.join("packed.spk");
    run(args(&direct, "5"));
    ok(&[
        "-t",
        "3",
        "pack",
        s(&a),
        "-o",
        s(&packed),
        "--block-rows",
        "64",
    ]);
    assert_eq!(fs::read(&direct).unwrap(), fs::read(&packed).unwrap());
    assert_eq!(ok(&["unpack", s(&direct)]).into_bytes(), text);
}

/// First word of the value of `key` in the `info` output.
fn info_value(info: &str, key: &str) -> String {
    info.lines()
        .find_map(|line| {
            let (k, v) = line.split_once(' ')?;
            (k == key).then(|| v.split_whitespace().next().unwrap_or("").to_string())
        })
        .unwrap_or_else(|| panic!("no {key} in {info}"))
}

#[test]
fn info_reports_sizes() {
    let dir = TempDir::new("info");
    let mut rng = SplitMix64::new(10);
    let m = random_matrix(&mut rng, 2000, 700, 30, 0.1);
    let tsv = dir.join("m.tsv");
    let spk = dir.join("m.spk");
    fs::write(&tsv, to_tsv(&m)).unwrap();
    ok(&["pack", s(&tsv), "-o", s(&spk), "--block-rows", "100"]);
    let info = ok(&["info", s(&spk)]);
    let nnz = m.entries.len() as u64;
    assert_eq!(info_value(&info, "shape"), "2000");
    assert_eq!(info_value(&info, "nnz"), nnz.to_string());
    assert_eq!(info_value(&info, "blocks"), "20");
    assert_eq!(info_value(&info, "block_rows"), "100");
    let file_bytes = fs::metadata(&spk).unwrap().len();
    assert_eq!(info_value(&info, "file_bytes"), file_bytes.to_string());
    let tsv_bytes = fs::metadata(&tsv).unwrap().len();
    assert_eq!(info_value(&info, "tsv_bytes"), tsv_bytes.to_string());
    assert_eq!(
        info_value(&info, "u32_triplet_bytes"),
        (12 * nnz).to_string()
    );
    let overhead = 20 + 36 + 36 * 20 + 16;
    assert_eq!(info_value(&info, "overhead_bytes"), overhead.to_string());

    // An empty matrix has no bytes per non-zero.
    let empty = dir.join("empty.spk");
    fs::write(&tsv, "# shape 0 0\n").unwrap();
    ok(&["pack", s(&tsv), "-o", s(&empty)]);
    let info = ok(&["info", s(&empty)]);
    assert_eq!(info_value(&info, "bytes_per_nnz"), "-");
    assert_eq!(info_value(&info, "tsv_bytes"), "12");
}

#[test]
fn verify_finds_damaged_blocks() {
    let dir = TempDir::new("verify");
    let mut rng = SplitMix64::new(12);
    let tsv = dir.join("m.tsv");
    let spk = dir.join("m.spk");
    fs::write(&tsv, to_tsv(&random_matrix(&mut rng, 300, 100, 10, 0.0))).unwrap();
    ok(&["pack", s(&tsv), "-o", s(&spk), "--block-rows", "50"]);
    assert!(ok(&["verify", s(&spk)]).contains("6 blocks verified"));

    // Block 0 starts right after the 20-byte header.
    let mut bytes = fs::read(&spk).unwrap();
    bytes[20] ^= 1;
    fs::write(&spk, &bytes).unwrap();
    let err = fails(&["verify", s(&spk)]);
    assert!(err.contains("block 0"), "{err}");
    assert!(err.contains("1 of 6 blocks are damaged"), "{err}");

    // Damage the footer checksum: the file cannot even be opened.
    let footer_crc = bytes.len() - 16 - 1;
    bytes[footer_crc] ^= 1;
    fs::write(&spk, &bytes).unwrap();
    let err = fails(&["verify", s(&spk)]);
    assert!(err.contains("checksum mismatch in footer"), "{err}");
}

#[test]
fn slice_matches_full_unpack() {
    let dir = TempDir::new("slice");
    let mut rng = SplitMix64::new(14);
    let tsv = dir.join("m.tsv");
    let spk = dir.join("m.spk");
    fs::write(&tsv, to_tsv(&random_matrix(&mut rng, 1000, 300, 20, 0.1))).unwrap();
    ok(&["pack", s(&tsv), "-o", s(&spk), "--block-rows", "100"]);
    let full = ok(&["unpack", s(&spk)]);
    let pick = |rows: Range<u64>, cols: Range<u64>| -> String {
        full.lines()
            .skip(1)
            .filter(|line| {
                let f: Vec<u64> = line.split('\t').map(|x| x.parse().unwrap()).collect();
                rows.contains(&f[0]) && cols.contains(&f[1])
            })
            .map(|line| format!("{line}\n"))
            .collect()
    };
    assert_eq!(
        ok(&["slice", s(&spk), "--rows", "250:260"]),
        pick(250..260, 0..300)
    );
    assert_eq!(
        ok(&["slice", s(&spk), "--rows", "90:420", "--cols", "10:50"]),
        pick(90..420, 10..50)
    );
    assert_eq!(
        ok(&["slice", s(&spk), "--rows", "950:"]),
        pick(950..1000, 0..300)
    );
    assert_eq!(
        ok(&["slice", s(&spk), "--rows", ":", "--cols", ":5"]),
        pick(0..1000, 0..5)
    );

    let out = sparsepack(&["slice", s(&spk), "--rows", "250:260"]);
    let report = String::from_utf8(out.stderr).unwrap();
    assert!(report.contains("read 1 of 10 blocks"), "{report}");
    let out = sparsepack(&["slice", s(&spk), "--rows", "199:201"]);
    let report = String::from_utf8(out.stderr).unwrap();
    assert!(report.contains("read 2 of 10 blocks"), "{report}");

    for bad in ["5", "9:3", "a:b", "-1:2"] {
        fails(&["slice", s(&spk), "--rows", bad]);
    }
}
