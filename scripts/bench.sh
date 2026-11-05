#!/usr/bin/env bash
# Size and speed benchmark on three synthetic matrices made by `sparsepack gen`.
#
# Sizes: plain TSV, TSV + gzip -6 (system gzip), plain u32 triplets, CSR and
# .spk. Times (3 runs each, median reported): pack, unpack (to a file, and to
# /dev/null to leave out the cost of writing to disk), slice of 1% of the
# rows, colsums at 1 and N threads, and a single-threaded pure-Python
# colsums on the TSV. gzip -6 is timed once per matrix.
# Input files are on local disk and in the page cache (warm runs).
#
# Writes results/machine.txt, results/bench_raw.tsv, results/bench_summary.tsv,
# results/sizes.tsv, results/checks.tsv and results/slice_reports.txt.
set -euo pipefail
cd "$(dirname "$0")/.."

bin=target/release/sparsepack
work=data/bench
results=results
mkdir -p "$work" "$results"
cargo build --release --quiet
threads="$(nproc)"

{
    echo "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | sed 's/^ //')"
    echo "physical_cores: $(grep -m1 'cpu cores' /proc/cpuinfo | cut -d: -f2 | sed 's/^ //')"
    echo "logical_cpus: $(nproc)"
    echo "mem_total: $(grep MemTotal /proc/meminfo | awk '{printf "%.1f GiB", $2/1048576}')"
    echo "kernel: $(uname -r)"
    echo "rustc: $(rustc --version)"
    echo "python: $(python3 --version)"
    echo "gzip: $(gzip --version | head -n 1)"
} > "$results/machine.txt"

# name rows cols mean_nnz_per_row
matrices=(
    "small 10000 5000 100"
    "medium 100000 30000 300"
    "large 300000 50000 300"
)

raw="$results/bench_raw.tsv"
echo -e "matrix\toperation\tthreads\trun\twall_s\tpeak_rss_mb" > "$raw"

# timed MATRIX OPERATION THREADS RUN STDOUT_FILE command...
timed() {
    local name="$1" op="$2" t="$3" rep="$4" out="$5"
    shift 5
    local timing
    timing="$(python3 scripts/time_cmd.py "$out" "$@")"
    echo -e "${name}\t${op}\t${t}\t${rep}\t${timing}" | tee -a "$raw"
}

info_value() { awk -v k="$2" '$1 == k { print $2 }' <<< "$1"; }

sizes="$results/sizes.tsv"
echo -e "matrix\trows\tcols\tnnz\tblocks\ttsv_bytes\tgzip6_bytes\tu32_triplet_bytes\tcsr_bytes\tspk_bytes\tspk_bytes_per_nnz\tgzip6_bytes_per_nnz" > "$sizes"
checks="$results/checks.tsv"
echo -e "matrix\tcheck\tresult" > "$checks"
: > "$results/slice_reports.txt"

for spec in "${matrices[@]}"; do
    read -r name rows cols mean <<< "$spec"
    tsv="$work/$name.tsv"
    spk="$work/$name.spk"
    "$bin" gen --rows "$rows" --cols "$cols" --mean-nnz "$mean" --zipf 1.0 --seed 42 -o "$tsv"

    start=$((rows / 2))
    end=$((start + rows / 100))
    for rep in 1 2 3; do
        timed "$name" pack 1 "$rep" /dev/null "$bin" -t 1 pack "$tsv" -o "$spk"
        timed "$name" pack "$threads" "$rep" /dev/null "$bin" -t "$threads" pack "$tsv" -o "$spk"
        timed "$name" unpack 1 "$rep" /dev/null "$bin" -t 1 unpack "$spk" -o "$work/$name.unpacked.tsv"
        timed "$name" unpack "$threads" "$rep" /dev/null "$bin" -t "$threads" unpack "$spk" -o "$work/$name.unpacked.tsv"
        timed "$name" unpack_devnull 1 "$rep" /dev/null "$bin" -t 1 unpack "$spk"
        timed "$name" unpack_devnull "$threads" "$rep" /dev/null "$bin" -t "$threads" unpack "$spk"
        timed "$name" slice_1pct_rows 1 "$rep" "$work/$name.slice.tsv" "$bin" -t 1 slice "$spk" --rows "$start:$end"
        timed "$name" colsums 1 "$rep" "$work/$name.colsums.t1.tsv" "$bin" -t 1 colsums "$spk"
        timed "$name" colsums "$threads" "$rep" "$work/$name.colsums.tN.tsv" "$bin" -t "$threads" colsums "$spk"
        timed "$name" python_colsums 1 "$rep" "$work/$name.colsums.py.tsv" python3 scripts/py_colsums.py "$tsv"
    done
    timed "$name" gzip6 1 1 "$tsv.gz" gzip -6 -c "$tsv"

    {
        echo "# $name: slice --rows $start:$end"
        "$bin" slice "$spk" --rows "$start:$end" 2>&1 > /dev/null
    } >> "$results/slice_reports.txt"

    info="$("$bin" info "$spk")"
    nnz="$(info_value "$info" nnz)"
    spk_bytes="$(stat -c %s "$spk")"
    gz_bytes="$(stat -c %s "$tsv.gz")"
    echo -e "${name}\t${rows}\t${cols}\t${nnz}\t$(info_value "$info" blocks)\t$(stat -c %s "$tsv")\t${gz_bytes}\t$(info_value "$info" u32_triplet_bytes)\t$(info_value "$info" csr_bytes)\t${spk_bytes}\t$(awk -v a="$spk_bytes" -v n="$nnz" 'BEGIN { printf "%.3f", a / n }')\t$(awk -v a="$gz_bytes" -v n="$nnz" 'BEGIN { printf "%.3f", a / n }')" >> "$sizes"

    same() { if cmp -s "$1" "$2"; then echo yes; else echo NO; fi; }
    echo -e "${name}\tunpack output identical to generated TSV\t$(same "$tsv" "$work/$name.unpacked.tsv")" >> "$checks"
    echo -e "${name}\tcolsums 1 thread = ${threads} threads\t$(same "$work/$name.colsums.t1.tsv" "$work/$name.colsums.tN.tsv")" >> "$checks"
    echo -e "${name}\tcolsums = py_colsums.py\t$(same "$work/$name.colsums.t1.tsv" "$work/$name.colsums.py.tsv")" >> "$checks"
    echo -e "${name}\tinfo tsv_bytes = TSV file size\t$([[ "$(info_value "$info" tsv_bytes)" == "$(stat -c %s "$tsv")" ]] && echo yes || echo NO)" >> "$checks"
    echo -e "${name}\tverify\t$("$bin" verify "$spk")" >> "$checks"
    rm -f "$work/$name.unpacked.tsv"
done

python3 scripts/summarize.py "$raw" matrix operation threads > "$results/bench_summary.tsv"
cat "$results/bench_summary.tsv" "$sizes" "$checks"
