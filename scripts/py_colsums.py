#!/usr/bin/env python3
"""Column sums of a TSV triplet file: the single-threaded baseline.

Standard library only, one pass over the text, no numpy.
Usage: py_colsums.py MATRIX.tsv
The file must start with a `# shape ROWS COLS` line (as written by
`sparsepack gen` and `sparsepack unpack`). Prints `col<TAB>sum` for every
column, the same output as `sparsepack colsums`.
"""
import sys


def main():
    sums = None
    with open(sys.argv[1], "rb") as f:
        for line in f:
            if line.startswith(b"#"):
                words = line.split()
                if len(words) == 4 and words[1] == b"shape":
                    sums = [0] * int(words[3])
                continue
            _row, col, value = line.split()
            sums[int(col)] += int(value)
    if sums is None:
        sys.exit("py_colsums.py: no `# shape ROWS COLS` line")
    sys.stdout.write("".join(f"{i}\t{s}\n" for i, s in enumerate(sums)))


if __name__ == "__main__":
    main()
