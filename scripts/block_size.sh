#!/usr/bin/env bash
# How the block size changes file size, pack time and the cost of random
# access, on the 100k x 30k benchmark matrix (same generator settings as
# bench.sh). Block sizes: 128, 1024 (default) and 8192 rows.
# Slices: 1% of the rows (1000 rows from the middle) and a single row.
# 3 runs per timing, median reported.
#
# Writes results/block_size_raw.tsv, results/block_size.tsv and
# results/block_size_times.tsv.
set -euo pipefail
cd "$(dirname "$0")/.."

bin=target/release/sparsepack
work=data/block_size
results=results
mkdir -p "$work" "$results"
cargo build --release --quiet
threads="$(nproc)"

rows=100000
tsv="$work/medium.tsv"
"$bin" gen --rows "$rows" --cols 30000 --mean-nnz 300 --zipf 1.0 --seed 42 -o "$tsv" 2> /dev/null

raw="$results/block_size_raw.tsv"
echo -e "block_rows\toperation\trun\twall_s\tpeak_rss_mb" > "$raw"
out="$results/block_size.tsv"
echo -e "block_rows\tblocks\tspk_bytes\toverhead_bytes\tslice\tblocks_read\tblock_bytes_read\tentries" > "$out"

for b in 128 1024 8192; do
    spk="$work/b$b.spk"
    for rep in 1 2 3; do
        t="$(python3 scripts/time_cmd.py /dev/null "$bin" -t "$threads" pack "$tsv" -o "$spk" --block-rows "$b" 2> /dev/null)"
        echo -e "${b}\tpack\t${rep}\t${t}" | tee -a "$raw"
        t="$(python3 scripts/time_cmd.py "$work/s1.tsv" "$bin" slice "$spk" --rows 50000:51000 2> /dev/null)"
        echo -e "${b}\tslice_1000_rows\t${rep}\t${t}" | tee -a "$raw"
        t="$(python3 scripts/time_cmd.py "$work/s2.tsv" "$bin" slice "$spk" --rows 50000:50001 2> /dev/null)"
        echo -e "${b}\tslice_1_row\t${rep}\t${t}" | tee -a "$raw"
        t="$(python3 scripts/time_cmd.py "$work/cs.tsv" "$bin" -t "$threads" colsums "$spk")"
        echo -e "${b}\tcolsums_${threads}_threads\t${rep}\t${t}" | tee -a "$raw"
    done
    info="$("$bin" info "$spk")"
    blocks="$(awk '$1 == "blocks" { print $2 }' <<< "$info")"
    overhead="$(awk '$1 == "overhead_bytes" { print $2 }' <<< "$info")"
    for range in 50000:51000 50000:50001; do
        report="$("$bin" slice "$spk" --rows "$range" 2>&1 > /dev/null)"
        # "...: N non-zeros; read K of M blocks (X of Y block bytes)"
        entries="$(sed -E 's/.*: ([0-9]+) non-zeros.*/\1/' <<< "$report")"
        read_blocks="$(sed -E 's/.*read ([0-9]+) of.*/\1/' <<< "$report")"
        read_bytes="$(sed -E 's/.*blocks \(([0-9]+) of.*/\1/' <<< "$report")"
        echo -e "${b}\t${blocks}\t$(stat -c %s "$spk")\t${overhead}\t${range}\t${read_blocks}\t${read_bytes}\t${entries}" >> "$out"
    done
done

python3 scripts/summarize.py "$raw" block_rows operation > "$results/block_size_times.tsv"
cat "$out" "$results/block_size_times.tsv"
