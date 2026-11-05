#!/usr/bin/env python3
"""Median, min and max wall time per setting of a raw timing table.

Usage: summarize.py RAW.tsv KEY_COLUMN [KEY_COLUMN ...]
RAW.tsv needs the columns `wall_s` and `peak_rss_mb` plus the key columns.
Rows with the same key values form one setting; settings keep the order in
which they first appear.
"""
import csv
import statistics
import sys


def main():
    path, keys = sys.argv[1], sys.argv[2:]
    with open(path) as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    groups = {}
    for r in rows:
        groups.setdefault(tuple(r[k] for k in keys), []).append(r)
    header = keys + ["runs", "median_wall_s", "min_wall_s", "max_wall_s", "median_peak_rss_mb"]
    print("\t".join(header))
    for key, rs in groups.items():
        walls = [float(r["wall_s"]) for r in rs]
        rss = [float(r["peak_rss_mb"]) for r in rs]
        print("\t".join(list(key) + [
            str(len(rs)),
            f"{statistics.median(walls):.3f}",
            f"{min(walls):.3f}",
            f"{max(walls):.3f}",
            f"{statistics.median(rss):.0f}",
        ]))


if __name__ == "__main__":
    main()
