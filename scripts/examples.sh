#!/usr/bin/env bash
# Run the example commands shown in the README on a small generated matrix
# and save the commands with their output to results/examples.txt.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release --quiet
export PATH="$PWD/target/release:$PATH"
work=data/examples
mkdir -p results
rm -rf "$work"
mkdir -p "$work"
cd "$work"

show() {
    echo "\$ $*"
    eval "$@" 2>&1
    echo
}

# Small input files for the MatrixMarket and error examples.
printf '%s\n' '%%MatrixMarket matrix coordinate integer general' '3 4 2' '1 2 5' '3 4 1' > tiny.mtx
printf '0\t5\t1\n0\t3\t1\n' > unsorted.tsv

{
    show "sparsepack gen --rows 5000 --cols 2000 --mean-nnz 40 --zipf 1.0 --seed 7 -o m.tsv"
    show "head -n 4 m.tsv"
    show "sparsepack pack m.tsv -o m.spk"
    show "sparsepack info m.spk"
    show "sparsepack verify m.spk"
    show "sparsepack slice m.spk --rows 2500:2502 --cols 0:400"
    show "sparsepack unpack m.spk | cmp - m.tsv && echo identical"
    show "sparsepack colsums m.spk | sort -k2,2nr | head -n 3"
    show "sparsepack rowsums m.spk | head -n 3"
    show "sparsepack filter m.spk --min-row-sum 60 --min-col-nnz 20 -o small.spk"
    show "head -n 3 small.map.tsv"
    show "grep -c '^row' small.map.tsv"
    show "cat tiny.mtx"
    show "sparsepack pack tiny.mtx -o tiny.spk"
    show "sparsepack unpack tiny.spk"
    show "cat unsorted.tsv"
    show "sparsepack pack unsorted.tsv -o unsorted.spk || echo \"exit status \$?\""
    echo "# flip one bit of byte 5000 of m.spk (inside a block)"
    python3 -c "
import sys
data = bytearray(open('m.spk', 'rb').read())
data[5000] ^= 1
open('m.spk', 'wb').write(data)
"
    show "sparsepack verify m.spk || echo \"exit status \$?\""
    show "head -c 60000 m.spk > cut.spk"
    show "sparsepack info cut.spk || echo \"exit status \$?\""
} > ../../results/examples.txt
cat ../../results/examples.txt
