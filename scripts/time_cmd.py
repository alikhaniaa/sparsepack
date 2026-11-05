#!/usr/bin/env python3
"""Run a command, send its stdout to a file, and print wall time and peak memory.

Usage: time_cmd.py OUTPUT_FILE command [args...]
Prints one line: <wall seconds><TAB><peak RSS in MB of the child process>
Standard error of the command is passed through.
"""
import resource
import subprocess
import sys
import time


def main():
    output_path = sys.argv[1]
    command = sys.argv[2:]
    with open(output_path, "wb") as out:
        start = time.perf_counter()
        subprocess.run(command, stdout=out, check=True)
        wall = time.perf_counter() - start
    # ru_maxrss is in kilobytes on Linux; it is the largest child so far,
    # which is the only child because this script runs one command.
    peak_mb = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024
    print(f"{wall:.3f}\t{peak_mb:.0f}")


if __name__ == "__main__":
    main()
