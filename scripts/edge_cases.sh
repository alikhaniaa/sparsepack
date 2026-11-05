#!/usr/bin/env bash
# Sizes on matrices where .spk is expected to do badly or differently:
# a tiny matrix (fixed overhead), dense matrices, a dense matrix of ones
# (very repetitive text, which gzip compresses well), and different column
# skews. Also gzip -6 of the .spk file itself, to see how much redundancy
# the varint coding leaves.
#
# Writes results/edge_sizes.tsv.
set -euo pipefail
cd "$(dirname "$0")/.."

bin=target/release/sparsepack
work=data/edge
results=results
mkdir -p "$work" "$results"
cargo build --release --quiet

# name rows cols mean_nnz_per_row zipf max_value
cases=(
    "tiny 10 10 2 1.0 1000"
    "small_rows 1000 5000 5 1.0 1000"
    "dense_40pct 2000 1000 400 1.0 1000"
    "dense_70pct 2000 200 150 1.0 1000"
    "dense_ones 2000 100 100 1.0 1"
    "uniform_cols 20000 30000 300 0.0 1000"
    "zipf_1.0 20000 30000 300 1.0 1000"
    "zipf_1.5 20000 30000 300 1.5 1000"
)

out="$results/edge_sizes.tsv"
echo -e "case\trows\tcols\tmean_nnz\tzipf\tmax_value\tnnz\tdensity\ttsv_bytes\tgzip6_bytes\tspk_bytes\tspk_gzip6_bytes\tspk_overhead_bytes\tspk_bytes_per_nnz\tgzip6_bytes_per_nnz\tsmaller" > "$out"

for spec in "${cases[@]}"; do
    read -r name rows cols mean zipf max_value <<< "$spec"
    tsv="$work/$name.tsv"
    spk="$work/$name.spk"
    "$bin" gen --rows "$rows" --cols "$cols" --mean-nnz "$mean" --zipf "$zipf" \
        --max-value "$max_value" --seed 7 -o "$tsv" 2> /dev/null
    "$bin" pack "$tsv" -o "$spk" 2> /dev/null
    gzip -6 -c "$tsv" > "$tsv.gz"
    gzip -6 -c "$spk" > "$spk.gz"
    info="$("$bin" info "$spk")"
    nnz="$(awk '$1 == "nnz" { print $2 }' <<< "$info")"
    density="$(awk '$1 == "density" { print $2 }' <<< "$info")"
    overhead="$(awk '$1 == "overhead_bytes" { print $2 }' <<< "$info")"
    tsv_b="$(stat -c %s "$tsv")"
    gz_b="$(stat -c %s "$tsv.gz")"
    spk_b="$(stat -c %s "$spk")"
    spkgz_b="$(stat -c %s "$spk.gz")"
    smaller=spk
    if (( gz_b < spk_b )); then smaller=tsv.gz; fi
    awk -v OFS='\t' -v name="$name" -v rows="$rows" -v cols="$cols" -v mean="$mean" \
        -v zipf="$zipf" -v max_value="$max_value" -v nnz="$nnz" -v density="$density" \
        -v tsv="$tsv_b" -v gz="$gz_b" -v spk="$spk_b" -v spkgz="$spkgz_b" \
        -v overhead="$overhead" -v smaller="$smaller" \
        'BEGIN { print name, rows, cols, mean, zipf, max_value, nnz, density, tsv, gz, spk,
                 spkgz, overhead, sprintf("%.3f", spk / nnz), sprintf("%.3f", gz / nnz), smaller }' >> "$out"
done
cat "$out"
